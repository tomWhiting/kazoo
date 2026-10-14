use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use kazoo_core::analysis::VOICE_MIN_HZ;
use kazoo_core::sanitize_sample;

/// Periods of the tracked pitch the scope shows.
pub const PERIODS: usize = 4;

/// The lowest pitch the mouth's tracker reports: it tracks the voice range.
pub const LOWEST_HZ: f32 = VOICE_MIN_HZ;

/// The rate `Stats::default()` captures at; a live session sizes its own.
const DEFAULT_RATE: u32 = 48_000;

/// Frames the spectrum reads: a power of two near 85 ms (4096 at 48 kHz), so
/// every one of its 32 bands from 50 Hz holds at least one bin at any rate.
#[must_use]
pub fn spectrum_frames(rate: u32) -> usize {
    (rate as usize)
        .saturating_mul(4096)
        .div_ceil(48_000)
        .next_power_of_two()
        .max(2)
}

/// Frames held: four periods of the lowest pitch for the window, one more
/// period to search back for the trigger, and two for interpolation; never
/// fewer than the spectrum reads.
#[must_use]
pub fn frames(rate: u32) -> usize {
    let period = (f64::from(rate) / f64::from(LOWEST_HZ)).ceil() as usize;
    period
        .saturating_mul(PERIODS + 1)
        .saturating_add(2)
        .max(spectrum_frames(rate))
}

/// The newest captured voice and synth samples, oldest first, as the UI
/// reads them. Only `Capture::frames` makes one, so its length always matches
/// the capture it reads from.
#[derive(Debug, Default, Clone)]
pub struct Frames {
    voice: Vec<f32>,
    synth: Vec<f32>,
}

impl Frames {
    #[must_use]
    pub fn voice(&self) -> &[f32] {
        &self.voice
    }

    #[must_use]
    pub fn synth(&self) -> &[f32] {
        &self.synth
    }

    #[cfg(test)]
    pub(crate) fn from_samples(voice: Vec<f32>, synth: Vec<f32>) -> Self {
        assert_eq!(voice.len(), synth.len());
        Self { voice, synth }
    }
}

#[derive(Debug)]
pub struct Capture {
    rate: u32,
    voice: Box<[AtomicU32]>,
    synth: Box<[AtomicU32]>,
    sequence: AtomicU64,
    end: AtomicU64,
}

impl Default for Capture {
    fn default() -> Self {
        Self::new(DEFAULT_RATE)
    }
}

impl Capture {
    /// Allocates both rings once, sized by `frames(rate)`; the audio thread
    /// only stores into them.
    #[must_use]
    pub fn new(rate: u32) -> Self {
        let length = frames(rate);
        Self {
            rate,
            voice: (0..length).map(|_| AtomicU32::new(0)).collect(),
            synth: (0..length).map(|_| AtomicU32::new(0)).collect(),
            sequence: AtomicU64::new(0),
            end: AtomicU64::new(0),
        }
    }

    #[must_use]
    pub const fn rate(&self) -> u32 {
        self.rate
    }

    /// An empty pair of buffers this capture can be read into. Allocates: call
    /// it on the UI side, once.
    #[must_use]
    pub fn frames(&self) -> Frames {
        Frames {
            voice: vec![0.0; self.voice.len()],
            synth: vec![0.0; self.synth.len()],
        }
    }

    pub(super) fn push(&self, voice: &[f32], stereo: &[f32]) {
        let length = self.voice.len() as u64;
        self.sequence.fetch_add(1, Ordering::SeqCst);
        let mut end = self.end.load(Ordering::SeqCst);
        for (input, output) in voice.iter().zip(stereo.chunks_exact(2)) {
            let index = (end % length) as usize;
            self.voice[index].store(sanitize_sample(*input).to_bits(), Ordering::SeqCst);
            let sample = sanitize_sample(output[1].mul_add(0.5, output[0] * 0.5));
            self.synth[index].store(sample.to_bits(), Ordering::SeqCst);
            end = end.wrapping_add(1);
        }
        self.end.store(end, Ordering::SeqCst);
        self.sequence.fetch_add(1, Ordering::SeqCst);
    }

    /// Copies the newest samples into `into`, oldest first, zero-filled before
    /// the first push. False when a callback overlapped every attempt; `into`
    /// may then hold a torn copy and the caller keeps its last good one.
    pub(crate) fn read(&self, into: &mut Frames) -> bool {
        let length = self.voice.len();
        if into.voice.len() != length || into.synth.len() != length {
            return false;
        }
        // A bounded retry leaves drawing to the next UI frame if a callback overlaps.
        for _ in 0..3 {
            let sequence = self.sequence.load(Ordering::SeqCst);
            if sequence % 2 != 0 {
                continue;
            }
            let end = self.end.load(Ordering::SeqCst);
            let count = end.min(length as u64) as usize;
            into.voice[..length - count].fill(0.0);
            into.synth[..length - count].fill(0.0);
            for offset in 0..count {
                let index = (end.wrapping_sub(count as u64).wrapping_add(offset as u64)
                    % length as u64) as usize;
                into.voice[length - count + offset] =
                    f32::from_bits(self.voice[index].load(Ordering::SeqCst));
                into.synth[length - count + offset] =
                    f32::from_bits(self.synth[index].load(Ordering::SeqCst));
            }
            if self.sequence.load(Ordering::SeqCst) == sequence {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rings_keep_the_latest_pair_and_sanitize_faults() {
        let capture = Capture::default();
        let length = frames(DEFAULT_RATE);
        let voice: Vec<_> = (0..length + 128)
            .map(|index| index as f32 / 10_000.0)
            .collect();
        let stereo: Vec<_> = voice
            .iter()
            .flat_map(|sample| [*sample, -*sample])
            .collect();
        capture.push(&voice, &stereo);
        let mut into = capture.frames();
        assert!(capture.read(&mut into));
        assert_eq!(into.voice(), &voice[128..]);
        assert!(into.synth().iter().all(|sample| *sample == 0.0));
        capture.push(
            &[f32::NAN, f32::INFINITY],
            &[f32::NAN, 0.0, f32::INFINITY, f32::NEG_INFINITY],
        );
        assert!(capture.read(&mut into));
        assert!(
            into.voice()
                .iter()
                .chain(into.synth())
                .all(|sample| sample.is_finite())
        );
        assert_eq!(into.voice()[length - 1].to_bits(), 0.0_f32.to_bits());
        assert_eq!(into.synth()[length - 1].to_bits(), 0.0_f32.to_bits());
    }

    #[test]
    fn a_read_into_buffers_of_another_capture_is_refused() {
        let capture = Capture::new(96_000);
        let mut wrong = Capture::new(48_000).frames();
        assert!(!capture.read(&mut wrong));
        let mut right = capture.frames();
        assert!(capture.read(&mut right));
        assert_eq!(right.voice().len(), frames(96_000));
    }

    #[test]
    fn the_capture_holds_five_periods_of_the_lowest_pitch_at_every_rate() {
        for rate in [
            8_000, 22_050, 44_100, 48_000, 88_200, 96_000, 176_400, 192_000, 384_000,
        ] {
            let period = f64::from(rate) / f64::from(LOWEST_HZ);
            let held = frames(rate);
            assert!(
                held as f64 >= period.mul_add((PERIODS + 1) as f64, 2.0),
                "{rate} Hz holds {held} frames, fewer than five {period}-frame periods"
            );
            let fft = spectrum_frames(rate);
            assert!(
                fft.is_power_of_two() && fft <= held,
                "{rate} Hz: {fft} of {held}"
            );
            assert!(
                f64::from(rate) / fft as f64 <= 11.8,
                "{rate} Hz: spectrum bins wider than 11.8 Hz"
            );
        }
    }
}
