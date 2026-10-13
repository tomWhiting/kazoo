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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadReport {
    Priming,
    Audio,
    Underrun,
    Jump,
    Overrun,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LiveStats {
    pub captured: u64,
    pub underruns: u64,
    pub jumps: u64,
    pub overruns: u64,
    pub nonfinite: u64,
    pub raw_clips: u64,
    pub raw_peak: f32,
    pub in_callback_max: usize,
}

#[derive(Debug)]
pub struct LiveWriter {
    ring: Arc<LiveRing>,
    written: u64,
    nonfinite: u64,
    raw_clips: u64,
    raw_peak: f32,
}

#[derive(Debug)]
pub struct LiveInput {
    ring: Arc<LiveRing>,
    pos: u64,
    frames: usize,
    // More than one output buffer of slack would hide an unsustainable buffer size.
    slack: usize,
    reset_pending: bool,
    started: bool,
    fade: usize,
    fade_left: usize,
    fade_total: usize,
    old_pos: Option<u64>,
    drift_frames: u64,
    drift_window: u64,
    underruns: u64,
    jumps: u64,
    overruns: u64,
    last_written: u64,
    last_sample: f32,
    hold_previous: bool,
    held_sample: f32,
}

/// Allocate the real stream's history off the audio thread.
/// Rate must be nonzero and frames must be 32, 64, 128 or 256.
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
        reset_pending: false,
        started: false,
        fade: ((rate as f32 * FADE_MS / 1000.0) as usize).max(1),
        fade_left: 0,
        fade_total: 0,
        old_pos: None,
        drift_frames: 0,
        drift_window: u64::from(rate) * DRIFT_WINDOW_MS / 1000,
        underruns: 0,
        jumps: 0,
        overruns: 0,
        last_written: 0,
        last_sample: 0.0,
        hold_previous: false,
        held_sample: 0.0,
    };
    (writer, reader)
}

impl LiveWriter {
    pub(super) fn observe_callback(&self, frames: usize) {
        self.ring
            .in_callback_max
            .fetch_max(frames, Ordering::Relaxed);
    }

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
        if self.reset_pending {
            self.slack = 0;
            self.reset_pending = false;
        }
        self.pos = written.saturating_sub((frames + self.slack) as u64);
        self.old_pos = old;
        self.hold_previous = false;
        self.held_sample = self.last_sample;
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
            let (old, fade) = if lag == 0 {
                (None, self.fade)
            } else {
                (Some(self.pos), self.fade.min(lag as usize))
            };
            self.jump(written, n, old, fade);
            // At zero lag there is no unread tail. Holding the final played
            // sample keeps the repeat continuous at the callback boundary.
            self.hold_previous = lag == 0;
            return ReadReport::Underrun;
        }
        if lag > target + n as u64 / 2 {
            self.drift_frames += n as u64;
        } else {
            self.drift_frames = 0;
        }
        if lag > target + 4 * n as u64 || self.drift_frames >= self.drift_window {
            self.jumps += 1;
            self.jump(written, n, Some(self.pos), self.fade.min(lag as usize));
            return ReadReport::Jump;
        }
        ReadReport::Audio
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
        let held = self.held_sample;
        for target in out {
            let new = self.sample(self.pos);
            *target = if self.fade_left > 0 {
                let old = if self.hold_previous {
                    held
                } else {
                    self.old_pos.map_or(0.0, |position| self.sample(position))
                };
                let weight = (self.fade_total - self.fade_left + 1) as f32 / self.fade_total as f32;
                self.fade_left -= 1;
                if let Some(position) = self.old_pos.as_mut() {
                    *position += 1;
                }
                new.mul_add(weight, old * (1.0 - weight))
            } else {
                new
            };
            self.last_sample = *target;
            self.pos += 1;
        }
        report
    }

    #[must_use]
    pub fn latency_frames(&self) -> usize {
        self.ring.in_callback_max.load(Ordering::Relaxed) + self.frames + self.slack
    }

    #[must_use]
    pub const fn slack(&self) -> usize {
        self.slack
    }

    /// Remove accumulated slack when the next crossfaded jump occurs.
    pub const fn reset_slack(&mut self) {
        self.reset_pending = true;
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

    #[cfg(test)]
    pub(super) const fn lag(&self) -> u64 {
        self.last_written.saturating_sub(self.pos)
    }
}
