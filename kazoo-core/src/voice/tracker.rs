use super::gate::{GATE_OFF_DB, GATE_ON_DB, Gate, ONSET_RISE_DB};
use super::judge::JudgeResult;
use super::{LOUDNESS_FLOOR_DB, VOICE_HOP, VoiceFrame, volts_from_hz};
use crate::analysis::{
    EnvelopeFollower, PitchEstimate,
    cycle::{CycleSettings, CycleTracker},
};
use crate::sanitize_sample;

/// Fourteen judge hops is the longest a verdict can still describe the note.
pub const JUDGE_FRESH_MS: u64 = 150;
/// An octave verdict needs pYIN to be more sure than not.
pub const JUDGE_MIN_PROB: f32 = 0.5;
/// Within 300 cents of an octave the judge folds; further away is a different note.
pub const FOLD_WINDOW: f32 = 0.25;
/// Three unvoiced verdicts span 32 ms and veto a periodic non-voice lock.
pub const VETO_RESULTS: u8 = 3;
/// A veto verdict must be very sure the input is unvoiced.
pub const VETO_PROB: f32 = 0.1;
/// Disagreement exceeding a semitone halves pitch confidence.
pub const DISAGREE_CENTS: f32 = 100.0;
/// A 300-cent frame leap exceeds vibrato and stops an old octave verdict folding a new note.
pub const LEAP_CENTS: f32 = 300.0;
/// A hint is tried for four cycles: at most 80 ms at 50 Hz but under 10 ms
/// from 400 Hz up, where a slip costs most. A hint landing within this
/// interval is our correction; a later landing remains a sung change.
pub const HINT_LEAP_MS: f32 = 10.0;
/// A one-millisecond attack lets the fast level see the start of a syllable.
pub const FAST_ATTACK_MS: f32 = 1.0;
/// Ten milliseconds sees a short syllable dip; low notes need longer release.
pub const FAST_RELEASE_MIN_MS: f32 = 10.0;
/// One and a half tracked periods keeps rectified ripple below three dB at 50 Hz.
pub const FAST_RELEASE_PERIODS: f32 = 1.5;
/// The recent valley follows rises slowly so a syllable's recovery remains visible.
pub const VALLEY_RISE_MS: f32 = 80.0;
/// Match the existing input meter's five-millisecond attack.
pub const LOUDNESS_ATTACK_MS: f32 = 5.0;
/// Match the existing input meter's fifty-millisecond release.
pub const LOUDNESS_RELEASE_MS: f32 = 50.0;
/// Breath changes at syllable speed rather than cycle speed.
pub const BREATH_SMOOTH_MS: f32 = 20.0;
/// Past +12 dBFS a sample is a fault, not a voice.
///
/// A converter delivers at most full scale, and a float path with gain
/// before it overshoots by a few dB at most. A fault resets the tracker;
/// anything up to it is a loud note, clipped to full scale as the cycle
/// tracker clips it.
pub const INPUT_FAULT_LEVEL: f32 = 4.0;

/// Instrument-dependent settings for the cycle tracker, judge and gate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackSettings {
    pub cycle: CycleSettings,
    /// The pYIN frame at 48 kHz, scaled with the stream rate.
    pub judge_frame_48k: usize,
    /// Opening level above the source's noise floor.
    pub gate_on_db: f32,
    /// Closing level below the opening level, providing hysteresis.
    pub gate_off_db: f32,
    /// Recovery from a recent valley that counts as another articulation.
    pub onset_rise_db: f32,
    /// `Some(c)`: a voiced move of at least c cents within two frames is an onset.
    pub jump_onset_cents: Option<f32>,
}

impl TrackSettings {
    /// The original voice judge, gate and level onset; sung legato never retriggers.
    pub const VOICE: Self = Self {
        cycle: CycleSettings::VOICE,
        judge_frame_48k: 2048,
        gate_on_db: GATE_ON_DB,
        gate_off_db: GATE_OFF_DB,
        onset_rise_db: ONSET_RISE_DB,
        jump_onset_cents: None,
    };

    pub(crate) const fn validate(self) {
        self.cycle.validate();
        assert!(
            self.judge_frame_48k >= 4
                && self.judge_frame_48k.is_power_of_two()
                && self.judge_frame_48k <= usize::MAX / 8,
            "invalid judge frame"
        );
        assert!(
            self.gate_on_db.is_finite()
                && self.gate_off_db.is_finite()
                && self.gate_off_db < self.gate_on_db,
            "invalid gate thresholds"
        );
        assert!(
            self.onset_rise_db.is_finite() && self.onset_rise_db > 0.0,
            "invalid onset rise"
        );
        if let Some(cents) = self.jump_onset_cents {
            assert!(cents.is_finite() && cents > 0.0, "invalid pitch jump");
        }
    }
}

/// Callback-safe voice facts from a preallocated cycle history and judge verdicts.
#[derive(Debug)]
pub struct VoiceTracker {
    settings: TrackSettings,
    pub(super) cycle: CycleTracker,
    rate: f32,
    pub(super) samples: u64,
    hop: usize,
    judge: Option<PitchEstimate>,
    judge_at: u64,
    judge_frame: u64,
    fresh_samples: u64,
    veto: u8,
    vetoed: bool,
    previous_cycle: Option<f32>,
    hinted: Option<(u64, f32)>,
    pub(super) last_leap: u64,
    pub(super) fold: f32,
    hz: f32,
    gate: Gate,
    fast: f32,
    attack: f32,
    release: f32,
    valley: f32,
    valley_coefficient: f32,
    loudness: EnvelopeFollower,
    breath: f32,
    breath_coefficient: f32,
    invalid_frame: bool,
}

impl VoiceTracker {
    /// Allocate the cycle history once; processing and resetting never allocate.
    #[must_use]
    pub fn new(rate: u32) -> Self {
        Self::with_settings(rate, TrackSettings::VOICE)
    }

    /// Allocate the selected profile's history once. Invalid settings are rejected here.
    #[must_use]
    pub fn with_settings(rate: u32, settings: TrackSettings) -> Self {
        settings.validate();
        let rate = rate.max(1);
        let sr = rate as f32;
        Self {
            settings,
            cycle: CycleTracker::with_settings(rate, settings.cycle),
            rate: sr,
            samples: 0,
            hop: 0,
            judge: None,
            judge_at: 0,
            judge_frame: settings.judge_config(rate).frame_length as u64,
            fresh_samples: JUDGE_FRESH_MS * u64::from(rate) / 1000,
            veto: 0,
            vetoed: false,
            previous_cycle: None,
            hinted: None,
            last_leap: 0,
            fold: 0.0,
            hz: 0.0,
            gate: Gate::with_settings(rate, settings),
            fast: 0.0,
            attack: coefficient(FAST_ATTACK_MS, sr),
            release: coefficient(FAST_RELEASE_MIN_MS, sr),
            valley: 0.0,
            valley_coefficient: coefficient(VALLEY_RISE_MS, sr),
            loudness: EnvelopeFollower::new(LOUDNESS_ATTACK_MS, LOUDNESS_RELEASE_MS, sr),
            breath: 0.0,
            breath_coefficient: coefficient(BREATH_SMOOTH_MS, sr / VOICE_HOP as f32),
            invalid_frame: false,
        }
    }

    /// Receive a memory-free verdict from the judge, on the callback's sample clock.
    pub fn judge(&mut self, result: JudgeResult) {
        match result {
            JudgeResult::Pitch(mut pitch) => {
                pitch.frequency = pitch.frequency.filter(|hz| hz.is_finite() && *hz > 0.0);
                pitch.voiced_probability =
                    sanitize_sample(pitch.voiced_probability).clamp(0.0, 1.0);
                if pitch.frequency.is_none() && pitch.voiced_probability < VETO_PROB {
                    self.veto = self.veto.saturating_add(1);
                    // Like octave folding, a veto must describe sound after the new lock.
                    if self.veto >= VETO_RESULTS
                        && self
                            .samples
                            .checked_sub(self.judge_frame)
                            .is_some_and(|from| from >= self.last_leap)
                        && self.cycle.estimate().is_some()
                    {
                        self.cycle.unlock();
                        self.vetoed = true;
                    }
                } else {
                    self.veto = 0;
                    self.vetoed = false;
                }
                self.judge = Some(pitch);
                self.judge_at = self.samples;
            }
            JudgeResult::Onset { strength } => {
                if strength.is_finite() && strength > 0.0 {
                    self.gate.flux(self.samples);
                }
            }
        }
    }

    /// Samples still to come before the next frame, 1 to [`VOICE_HOP`].
    #[must_use]
    pub const fn until_frame(&self) -> usize {
        VOICE_HOP - self.hop
    }

    /// Process arbitrary callback sizes, emitting once per fixed 64-sample hop.
    pub fn process(&mut self, mic: &[f32], mut emit: impl FnMut(VoiceFrame)) {
        for &sample in mic {
            let x = if sample.is_finite() && sample.abs() <= INPUT_FAULT_LEVEL {
                sample.clamp(-1.0, 1.0)
            } else {
                self.invalid_frame = true;
                self.cycle.unlock();
                self.gate.reset();
                self.fast = 0.0;
                self.valley = 0.0;
                self.loudness.reset();
                0.0
            };
            self.cycle.push(x);
            let rectified = x.abs();
            self.fast = if rectified > self.fast {
                self.attack
                    .mul_add(self.fast, (1.0 - self.attack) * rectified)
            } else {
                self.fast * self.release
            };
            self.valley = if self.fast < self.valley {
                self.fast
            } else {
                self.valley_coefficient
                    .mul_add(self.valley, (1.0 - self.valley_coefficient) * self.fast)
            };
            self.loudness.process_sample(x);
            self.samples = self.samples.saturating_add(1);
            self.hop += 1;
            if self.hop == VOICE_HOP {
                self.hop = 0;
                emit(self.frame());
                self.invalid_frame = false;
            }
        }
    }

    fn frame(&mut self) -> VoiceFrame {
        if db(self.fast) < self.settings.gate_on_db && self.cycle.estimate().is_none() {
            self.vetoed = false;
        }
        let fc = self
            .cycle
            .estimate()
            .filter(|_| !self.vetoed && !self.invalid_frame);
        let current = fc.map(|e| e.hz);
        if let Some(hz) = current {
            let hint_landed = self.hinted.is_some_and(|(at, hinted_hz)| {
                self.samples.saturating_sub(at) as f32 <= HINT_LEAP_MS * self.rate / 1000.0
                    && (1200.0 * (hz / hinted_hz).log2()).abs() <= 50.0
            });
            if hint_landed {
                self.hinted = None;
            }
            if !hint_landed
                && self
                    .previous_cycle
                    .is_none_or(|previous| (1200.0 * (hz / previous).log2()).abs() > LEAP_CENTS)
            {
                self.last_leap = self.samples;
            }
        }
        self.previous_cycle = current;
        let fresh = self.samples.saturating_sub(self.judge_at) <= self.fresh_samples;
        let judge_hz = self.judge.filter(|_| fresh).and_then(|p| p.frequency);
        if let Some(estimate) = fc {
            let mut fold = 0.0;
            if !estimate.provisional
                && self
                    .judge_at
                    .checked_sub(self.judge_frame)
                    .is_some_and(|from| from >= self.last_leap)
                && self
                    .judge
                    .is_some_and(|p| p.voiced_probability >= JUDGE_MIN_PROB)
            {
                if let Some(judge_hz) = judge_hz {
                    let ratio = (judge_hz / estimate.hz).log2();
                    let octave = ratio.round();
                    if octave != 0.0 && (ratio - octave).abs() <= FOLD_WINDOW {
                        fold = octave;
                    }
                }
            }
            self.hz = estimate.hz * fold.exp2();
            if fold != 0.0 && (fold - self.fold).abs() > 0.5 {
                self.cycle.hint(self.rate / self.hz);
                self.hinted = Some((self.samples, self.hz));
            }
            self.fold = fold;
            let release_ms = FAST_RELEASE_MIN_MS
                .max(FAST_RELEASE_PERIODS * estimate.period * 1000.0 / self.rate);
            self.release = coefficient(release_ms, self.rate);
        }
        let voiced = fc.is_some();
        let fast_db = db(self.fast);
        let (gate, onset) =
            self.gate
                .frame_with_pitch(self.samples, voiced, fast_db, db(self.valley), self.hz);
        if onset {
            self.valley = self.fast;
        }
        let raw_breath = if fast_db < self.settings.gate_on_db {
            0.0
        } else if voiced {
            (2.0 * (1.0 - self.cycle.last_clarity())).clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.breath = self
            .breath_coefficient
            .mul_add(self.breath, (1.0 - self.breath_coefficient) * raw_breath);
        let mut confidence = fc.map_or(0.0, |e| e.clarity);
        if fc.is_none_or(|estimate| !estimate.provisional)
            && judge_hz.is_some_and(|hz| (1200.0 * (hz / self.hz).log2()).abs() > DISAGREE_CENTS)
        {
            confidence *= 0.5;
        }
        let valid_hz = self.hz.is_finite();
        let hz = sanitize_sample(self.hz);
        let volts = sanitize_sample(volts_from_hz(hz));
        VoiceFrame {
            at: self.samples,
            hz,
            volts,
            snapped_hz: hz,
            snapped_volts: volts,
            gate: gate && valid_hz && !self.invalid_frame,
            onset: onset && valid_hz && !self.invalid_frame,
            voiced: voiced && valid_hz,
            loudness: sanitize_sample(
                (db(self.loudness.current()) - LOUDNESS_FLOOR_DB) / -LOUDNESS_FLOOR_DB,
            )
            .clamp(0.0, 1.0),
            breath: sanitize_sample(self.breath).clamp(0.0, 1.0),
            confidence: sanitize_sample(confidence).clamp(0.0, 1.0),
        }
    }

    /// Clear history, verdicts and frame clock without allocating.
    pub fn reset(&mut self) {
        self.cycle.reset();
        self.samples = 0;
        self.hop = 0;
        self.judge = None;
        self.judge_at = 0;
        self.veto = 0;
        self.vetoed = false;
        self.previous_cycle = None;
        self.hinted = None;
        self.last_leap = 0;
        self.fold = 0.0;
        self.hz = 0.0;
        self.gate.reset();
        self.fast = 0.0;
        self.valley = 0.0;
        self.loudness.reset();
        self.breath = 0.0;
        self.invalid_frame = false;
        self.release = coefficient(FAST_RELEASE_MIN_MS, self.rate);
    }
}
fn coefficient(ms: f32, rate: f32) -> f32 {
    (-1000.0 / (ms * rate)).exp()
}
fn db(level: f32) -> f32 {
    (20.0 * level.max(1e-6).log10()).max(-120.0)
}
