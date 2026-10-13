use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

use super::{CLIP_RAW, DRIFT_WINDOW_MS, FADE_MS, GUARD, LIVE_RING_FRAMES, SLACK_STEP};

/// A history, rather than a consuming queue, permits short repeats at underruns.
/// Positions use u64: 2^64 frames lasts about 12 million years at 48 kHz.
#[derive(Debug)]
struct LiveRing {
    samples: Box<[AtomicU32]>,
    written: AtomicU64,
    nonfinite: AtomicU64,
    raw_clips: AtomicU64,
    raw_peak: AtomicU32,
    in_callback_max: AtomicUsize,
}

/// What a read did to fill its output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadReport {
    /// Not yet one buffer captured: the output is silence.
    Priming,
    /// The next frames in order, or the start with its fade in.
    Audio,
    /// Too few frames: a crossfaded repeat from the past, and more slack.
    Underrun,
    /// Too far behind (drift, a burst, or `reset_slack`): a crossfaded jump
    /// forward.
    Jump,
    /// The reader stalled past the ring's guard: a restart with a fade in.
    Overrun,
}

/// One channel's counts since it opened.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LiveStats {
    /// Frames the input callback has written.
    pub captured: u64,
    /// Reads that found fewer frames than they needed.
    pub underruns: u64,
    /// Drift corrections: crossfaded jumps forward.
    pub jumps: u64,
    /// Restarts after the reader fell past the guard.
    pub overruns: u64,
    /// NaN or infinite samples, written as 0.
    pub nonfinite: u64,
    /// Finite samples at or above `CLIP_RAW` before any gain.
    pub raw_clips: u64,
    /// The largest finite magnitude seen before it was held to 1.0.
    pub raw_peak: f32,
    /// The longest input callback seen, in frames.
    pub in_callback_max: usize,
}

/// The input callback's half of one channel's ring.
#[derive(Debug)]
pub struct LiveWriter {
    ring: Arc<LiveRing>,
    written: u64,
    nonfinite: u64,
    raw_clips: u64,
    raw_peak: f32,
}

/// The output callback's half of one channel's ring; move it into the
/// callback that reads it.
#[derive(Debug)]
pub struct LiveInput {
    ring: Arc<LiveRing>,
    pos: u64,
    frames: usize,
    // More than one output buffer of slack would hide an unsustainable buffer size.
    slack: usize,
    // Set by reset_slack: the next read jumps to the new, smaller target.
    retarget: bool,
    started: bool,
    fade: usize,
    fade_left: usize,
    fade_total: usize,
    // The position faded out of, read while it is below `old_end` (the
    // frames written when the jump was taken) and held at its last sample
    // after, so every jump crossfades for the whole fade. None fades in from
    // silence.
    old_pos: Option<u64>,
    old_end: u64,
    old_value: f32,
    drift_frames: u64,
    drift_window: u64,
    underruns: u64,
    jumps: u64,
    overruns: u64,
    last_written: u64,
    last_sample: f32,
}

/// The real ring's two halves without a device: the stream uses this, and
/// so do the desk's tests. Not a fake. It allocates, so call it off the audio
/// thread. Panics unless `rate` is nonzero and `frames` is 32, 64, 128 or 256.
#[must_use]
pub fn live_pair(rate: u32, frames: usize) -> (LiveWriter, LiveInput) {
    assert!(rate > 0 && matches!(frames, 32 | 64 | 128 | 256));
    let ring = Arc::new(LiveRing {
        samples: (0..LIVE_RING_FRAMES).map(|_| AtomicU32::new(0)).collect(),
        written: AtomicU64::new(0),
        nonfinite: AtomicU64::new(0),
        raw_clips: AtomicU64::new(0),
        raw_peak: AtomicU32::new(0),
        in_callback_max: AtomicUsize::new(0),
    });
    let writer = LiveWriter {
        ring: Arc::clone(&ring),
        written: 0,
        nonfinite: 0,
        raw_clips: 0,
        raw_peak: 0.0,
    };
    let reader = LiveInput {
        ring,
        pos: 0,
        frames,
        slack: 0,
        retarget: false,
        started: false,
        fade: ((rate as f32 * FADE_MS / 1000.0) as usize).max(1),
        fade_left: 0,
        fade_total: 0,
        old_pos: None,
        old_end: 0,
        old_value: 0.0,
        drift_frames: 0,
        drift_window: u64::from(rate) * DRIFT_WINDOW_MS / 1000,
        underruns: 0,
        jumps: 0,
        overruns: 0,
        last_written: 0,
        last_sample: 0.0,
    };
    (writer, reader)
}

impl LiveWriter {
    pub(super) fn observe_callback(&self, frames: usize) {
        self.ring
            .in_callback_max
            .fetch_max(frames, Ordering::Relaxed);
    }

    /// Store one channel's samples: a non-finite sample becomes 0 and is
    /// counted, a raw clip is counted, and every sample is held to ±1.0.
    pub fn write(&mut self, mono: &[f32]) {
        self.observe_callback(mono.len());
        for &raw in mono {
            let sample = if raw.is_finite() {
                let magnitude = raw.abs();
                if magnitude >= CLIP_RAW {
                    self.raw_clips += 1;
                }
                self.raw_peak = self.raw_peak.max(magnitude);
                raw.clamp(-1.0, 1.0)
            } else {
                self.nonfinite += 1;
                0.0
            };
            self.ring.samples[index(self.written)].store(sample.to_bits(), Ordering::Relaxed);
            self.written += 1;
        }
        self.ring.nonfinite.store(self.nonfinite, Ordering::Relaxed);
        self.ring.raw_clips.store(self.raw_clips, Ordering::Relaxed);
        self.ring
            .raw_peak
            .store(self.raw_peak.to_bits(), Ordering::Relaxed);
        self.ring.written.store(self.written, Ordering::Release);
    }
}

const fn index(position: u64) -> usize {
    (position & (LIVE_RING_FRAMES as u64 - 1)) as usize
}

impl LiveInput {
    fn sample(&self, position: u64) -> f32 {
        f32::from_bits(self.ring.samples[index(position)].load(Ordering::Relaxed))
    }

    const fn jump(&mut self, written: u64, frames: usize, old: Option<u64>, fade: usize) {
        self.retarget = false;
        self.pos = written.saturating_sub((frames + self.slack) as u64);
        self.old_pos = old;
        self.old_end = written;
        self.old_value = if old.is_some() { self.last_sample } else { 0.0 };
        self.fade_total = fade;
        self.fade_left = fade;
        self.drift_frames = 0;
        self.started = true;
    }

    fn prepare_read(&mut self, written: u64, n: usize) -> ReadReport {
        let target = (n + self.slack) as u64;
        if !self.started {
            if written < target {
                return ReadReport::Priming;
            }
            self.jump(written, n, None, self.fade);
            return ReadReport::Audio;
        }
        let lag = written.saturating_sub(self.pos);
        if lag > (LIVE_RING_FRAMES - GUARD) as u64 {
            self.overruns += 1;
            self.jump(written, n, None, self.fade);
            return ReadReport::Overrun;
        }
        if lag < n as u64 {
            self.slack = (self.slack + SLACK_STEP).min(n);
            self.underruns += 1;
            // The unread tail (none at zero lag) fades out and then holds its
            // last sample, so the repeat is never a step to silence.
            self.jump(written, n, Some(self.pos), self.fade);
            return ReadReport::Underrun;
        }
        if self.retarget && lag > target {
            self.jump(written, n, Some(self.pos), self.fade);
            return ReadReport::Jump;
        }
        self.retarget = false;
        if lag > target + n as u64 / 2 {
            self.drift_frames += n as u64;
        } else {
            self.drift_frames = 0;
        }
        if lag > target + 4 * n as u64 || self.drift_frames >= self.drift_window {
            self.jumps += 1;
            self.jump(written, n, Some(self.pos), self.fade);
            return ReadReport::Jump;
        }
        ReadReport::Audio
    }

    /// The next sample of the position being faded out: its own frames while
    /// they were written before the jump, then its last sample held.
    fn fading_out(&mut self) -> f32 {
        let end = self.old_end;
        if let Some(position) = self.old_pos.filter(|position| *position < end) {
            self.old_value = self.sample(position);
            self.old_pos = Some(position + 1);
        }
        self.old_value
    }

    /// Fill the entire output without consuming the history or allocating.
    pub fn read(&mut self, out: &mut [f32]) -> ReadReport {
        if out.is_empty() {
            return ReadReport::Audio;
        }
        self.frames = out.len();
        let written = self.ring.written.load(Ordering::Acquire);
        self.last_written = written;
        let report = self.prepare_read(written, out.len());
        if report == ReadReport::Priming {
            out.fill(0.0);
            return report;
        }
        for target in out {
            let new = self.sample(self.pos);
            *target = if self.fade_left > 0 {
                let old = self.fading_out();
                let weight = (self.fade_total - self.fade_left + 1) as f32 / self.fade_total as f32;
                self.fade_left -= 1;
                new.mul_add(weight, old * (1.0 - weight))
            } else {
                new
            };
            self.last_sample = *target;
            self.pos += 1;
        }
        report
    }

    /// The software latency in frames: the longest input callback measured,
    /// plus the output buffer, plus the slack.
    #[must_use]
    pub fn latency_frames(&self) -> usize {
        self.ring.in_callback_max.load(Ordering::Relaxed) + self.frames + self.slack
    }

    #[must_use]
    pub const fn slack(&self) -> usize {
        self.slack
    }

    /// Return the slack to 0. The next read takes the latency back with a
    /// crossfaded jump forward (reported as `Jump`, not counted in `jumps`,
    /// which counts drift corrections); a later underrun grows it again.
    pub const fn reset_slack(&mut self) {
        self.slack = 0;
        self.retarget = true;
    }

    #[must_use]
    pub fn stats(&self) -> LiveStats {
        LiveStats {
            captured: self.ring.written.load(Ordering::Acquire),
            underruns: self.underruns,
            jumps: self.jumps,
            overruns: self.overruns,
            nonfinite: self.ring.nonfinite.load(Ordering::Relaxed),
            raw_clips: self.ring.raw_clips.load(Ordering::Relaxed),
            raw_peak: f32::from_bits(self.ring.raw_peak.load(Ordering::Relaxed)),
            in_callback_max: self.ring.in_callback_max.load(Ordering::Relaxed),
        }
    }

    #[cfg(test)]
    pub(super) const fn position(&self) -> u64 {
        self.pos
    }

    /// The lag of the block the last read delivered: frames written when it
    /// read, less the first frame it gave. The policy's own `lag` measure.
    #[cfg(test)]
    pub(super) const fn lag(&self) -> u64 {
        self.last_written
            .saturating_sub(self.pos.saturating_sub(self.frames as u64))
    }
}
