//! A sample in memory, ready to play: stereo `f32`, its band-limited copies
//! for pitching up, and its slice points.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use kazoo_fx::dsp::hermite;

use crate::resample::Halver;
use crate::{Error, Result, SampleName, onset, rate_is_valid};

/// Octaves above the original a voice can reach. Each needs one
/// half-rate copy of the sample, so a voice pitched up that far still
/// reads audio with nothing above its Nyquist frequency.
pub const MAX_OCTAVES_UP: usize = 6;

/// The number of copies kept: the original and one per octave.
const LEVELS: usize = MAX_OCTAVES_UP + 1;

/// One copy of the audio at `1 / 2^level` of the original rate.
#[derive(Clone)]
struct Level {
    left: Vec<f32>,
    right: Vec<f32>,
}

/// A share of a memory budget, handed back when dropped.
pub(crate) struct Reservation {
    budget: Arc<AtomicUsize>,
    bytes: usize,
}

impl Reservation {
    /// Take `bytes` from `budget` (the bytes in use) if that stays within
    /// `cap`; otherwise say how much is left.
    pub(crate) fn take(
        budget: &Arc<AtomicUsize>,
        bytes: usize,
        cap: usize,
    ) -> std::result::Result<Self, usize> {
        let mut used = budget.load(Ordering::Acquire);
        loop {
            let after = used.saturating_add(bytes);
            if after > cap {
                return Err(cap.saturating_sub(used));
            }
            match budget.compare_exchange_weak(used, after, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => {
                    return Ok(Self {
                        budget: Arc::clone(budget),
                        bytes,
                    });
                }
                Err(now) => used = now,
            }
        }
    }

    /// Give back everything above `bytes`.
    fn shrink_to(&mut self, bytes: usize) {
        if bytes < self.bytes {
            self.budget.fetch_sub(self.bytes - bytes, Ordering::AcqRel);
            self.bytes = bytes;
        }
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.budget.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

/// A playable sample: stereo audio at a known rate, a band-limited copy of
/// it for every octave a voice may be pitched up, and the slice points the
/// [`onset`] analyser found.
///
/// Built off the audio thread (it allocates), then shared with players as
/// an `Arc`. Reading it is real-time safe.
pub struct SampleData {
    name: SampleName,
    rate: u32,
    frames: usize,
    peak: f32,
    levels: Vec<Level>,
    onsets: Vec<usize>,
    reservation: Option<Reservation>,
}

impl fmt::Debug for SampleData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SampleData")
            .field("name", &self.name)
            .field("rate", &self.rate)
            .field("frames", &self.frames)
            .field("peak", &self.peak)
            .field("onsets", &self.onsets.len())
            .finish_non_exhaustive()
    }
}

impl SampleData {
    /// Build a sample from stereo audio at `rate`. Non-finite samples become
    /// silence. Both channels must be the same, non-zero length. Allocates;
    /// never call it on the audio thread.
    pub fn new(name: SampleName, rate: u32, left: Vec<f32>, right: Vec<f32>) -> Result<Self> {
        Self::build(name, rate, left, right, None)
    }

    /// Build a sample whose memory is counted against a store's budget.
    pub(crate) fn build(
        name: SampleName,
        rate: u32,
        mut left: Vec<f32>,
        mut right: Vec<f32>,
        reservation: Option<Reservation>,
    ) -> Result<Self> {
        if !rate_is_valid(rate) {
            return Err(Error::BadAudio {
                reason: format!("sample rate {rate} Hz is out of range"),
            });
        }
        if left.is_empty() || left.len() != right.len() {
            return Err(Error::BadAudio {
                reason: format!(
                    "channels must be the same non-zero length (left {}, right {})",
                    left.len(),
                    right.len()
                ),
            });
        }
        kazoo_fx::dsp::sanitise(&mut left);
        kazoo_fx::dsp::sanitise(&mut right);
        let peak = left
            .iter()
            .chain(&right)
            .fold(0.0f32, |m, x| m.max(x.abs()));
        let onsets = onset::detect(&left, &right, rate);
        let frames = left.len();
        let halver = Halver::new();
        let mut levels = Vec::with_capacity(LEVELS);
        levels.push(Level { left, right });
        while levels.len() < LEVELS {
            let above = &levels[levels.len() - 1];
            let next = Level {
                left: halver.process(&above.left),
                right: halver.process(&above.right),
            };
            levels.push(next);
        }
        let mut data = Self {
            name,
            rate,
            frames,
            peak,
            levels,
            onsets,
            reservation,
        };
        let bytes = data.bytes();
        if let Some(reservation) = data.reservation.as_mut() {
            reservation.shrink_to(bytes);
        }
        Ok(data)
    }

    /// The upper bound on [`Self::bytes`] for a sample of `frames` frames,
    /// used to reserve memory before building it.
    pub(crate) const fn bytes_for(frames: usize) -> usize {
        // Two channels of f32, and the halving copies add up to less than
        // the original again (plus a little rounding per level).
        let one = frames
            .saturating_add(LEVELS)
            .saturating_mul(2 * size_of::<f32>());
        one.saturating_mul(2)
            .saturating_add(onset::MAX_ONSETS * size_of::<usize>())
    }

    /// The memory the audio, its copies and its slice points take.
    #[must_use]
    pub fn bytes(&self) -> usize {
        let audio: usize = self
            .levels
            .iter()
            .map(|level| (level.left.len() + level.right.len()) * size_of::<f32>())
            .sum();
        audio + self.onsets.len() * size_of::<usize>()
    }

    /// The sample's name.
    #[must_use]
    pub const fn name(&self) -> &SampleName {
        &self.name
    }

    /// Frames per second.
    #[must_use]
    pub const fn rate(&self) -> u32 {
        self.rate
    }

    /// Length in frames.
    #[must_use]
    pub const fn frames(&self) -> usize {
        self.frames
    }

    /// Length in seconds.
    #[must_use]
    pub fn seconds(&self) -> f64 {
        self.frames as f64 / f64::from(self.rate)
    }

    /// The loudest sample, either channel, as a linear magnitude.
    #[must_use]
    pub const fn peak(&self) -> f32 {
        self.peak
    }

    /// The left channel at the original rate.
    #[must_use]
    pub fn left(&self) -> &[f32] {
        &self.levels[0].left
    }

    /// The right channel at the original rate.
    #[must_use]
    pub fn right(&self) -> &[f32] {
        &self.levels[0].right
    }

    /// Where new events start, in frames: sorted, starting with 0.
    #[must_use]
    pub fn onsets(&self) -> &[usize] {
        &self.onsets
    }

    /// The stereo frame at `position` (in original-rate frames, fractional),
    /// read by a voice moving `speed` frames per output frame. Pitched up by
    /// more than an octave, it reads the band-limited copies, blending the
    /// two nearest so a pitch sweep never steps. Four-point Hermite
    /// interpolation; silence outside the sample. Real-time safe.
    #[must_use]
    pub fn read(&self, position: f64, speed: f64) -> (f32, f32) {
        if !position.is_finite() {
            return (0.0, 0.0);
        }
        let speed = speed.abs();
        let octave = if speed > 1.0 && speed.is_finite() {
            speed.log2().min(MAX_OCTAVES_UP as f64)
        } else {
            0.0
        };
        let lower = octave.floor() as usize;
        let blend = (octave - lower as f64) as f32;
        let (l0, r0) = self.read_level(lower, position);
        if blend <= 0.0 || lower + 1 >= LEVELS {
            return (l0, r0);
        }
        let (l1, r1) = self.read_level(lower + 1, position);
        ((l1 - l0).mul_add(blend, l0), (r1 - r0).mul_add(blend, r0))
    }

    /// One level read at an original-rate `position`.
    fn read_level(&self, level: usize, position: f64) -> (f32, f32) {
        let Some(copy) = self.levels.get(level) else {
            return (0.0, 0.0);
        };
        let at = position / f64::from(1u32 << level);
        let whole = at.floor();
        let frac = (at - whole) as f32;
        let base = whole as i64;
        let pick = |channel: &[f32], offset: i64| {
            let index = base + offset;
            if index < 0 {
                return 0.0;
            }
            channel.get(index as usize).copied().unwrap_or(0.0)
        };
        let left = hermite(
            pick(&copy.left, -1),
            pick(&copy.left, 0),
            pick(&copy.left, 1),
            pick(&copy.left, 2),
            frac,
        );
        let right = hermite(
            pick(&copy.right, -1),
            pick(&copy.right, 0),
            pick(&copy.right, 1),
            pick(&copy.right, 2),
            frac,
        );
        (left, right)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name() -> SampleName {
        SampleName::new("test").unwrap()
    }

    #[test]
    fn reads_land_on_the_samples() {
        let left: Vec<f32> = (0..64).map(|n| n as f32).collect();
        let right: Vec<f32> = left.iter().map(|x| -x).collect();
        let data = SampleData::new(name(), 48_000, left, right).unwrap();
        assert_eq!(data.read(10.0, 1.0), (10.0, -10.0));
        let (l, r) = data.read(10.5, 1.0);
        assert!((l - 10.5).abs() < 1e-4 && (r + 10.5).abs() < 1e-4);
        assert_eq!(data.read(-5.0, 1.0), (0.0, 0.0));
        assert_eq!(data.read(500.0, 1.0), (0.0, 0.0));
        assert_eq!(data.read(f64::NAN, 1.0), (0.0, 0.0));
        assert!(data.read(10.0, f64::INFINITY).0.is_finite());
    }

    #[test]
    fn pitching_up_reads_band_limited_copies() {
        // A tone at 0.4 of the sample rate folds when read at speed 4
        // unless the reader takes a copy with it filtered out.
        let rate = 48_000;
        let tone: Vec<f32> = (0..rate as usize)
            .map(|n| (std::f32::consts::TAU * 0.4 * n as f32).sin() * 0.5)
            .collect();
        let data = SampleData::new(name(), rate, tone.clone(), tone).unwrap();
        let mut peak = 0.0f32;
        let mut position = 10_000.0;
        for _ in 0..2_000 {
            peak = peak.max(data.read(position, 4.0).0.abs());
            position += 4.0;
        }
        assert!(peak < 0.01, "an above-Nyquist tone came through at {peak}");
    }

    #[test]
    fn poison_and_bad_shapes() {
        let data = SampleData::new(
            name(),
            48_000,
            vec![f32::NAN, 1.0],
            vec![0.5, f32::INFINITY],
        )
        .unwrap();
        assert_eq!(data.left(), &[0.0, 1.0]);
        assert_eq!(data.right(), &[0.5, 0.0]);
        assert!((data.peak() - 1.0).abs() < f32::EPSILON);
        assert!(SampleData::new(name(), 48_000, vec![], vec![]).is_err());
        assert!(SampleData::new(name(), 48_000, vec![0.0], vec![0.0, 0.0]).is_err());
        assert!(SampleData::new(name(), 0, vec![0.0], vec![0.0]).is_err());
    }

    #[test]
    fn reservations_come_back_and_fit_the_estimate() {
        let budget = Arc::new(AtomicUsize::new(0));
        let frames = 10_000;
        let reserved =
            Reservation::take(&budget, SampleData::bytes_for(frames), usize::MAX).unwrap();
        let data = SampleData::build(
            name(),
            48_000,
            vec![0.1; frames],
            vec![0.1; frames],
            Some(reserved),
        )
        .unwrap();
        assert!(data.bytes() <= SampleData::bytes_for(frames));
        assert_eq!(budget.load(Ordering::Acquire), data.bytes());
        drop(data);
        assert_eq!(budget.load(Ordering::Acquire), 0);
        assert_eq!(Reservation::take(&budget, 100, 50).err(), Some(50));
    }
}
