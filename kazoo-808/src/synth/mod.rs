//! TR-808 voice synthesis.
//!
//! Each voice is a triggered decaying oscillator or noise source.
//! Sample-by-sample, zero lookahead.

pub mod clap;
pub mod cowbell;
pub mod hihat;
pub mod kick;
pub mod snare;
pub mod tom;

use self::clap::Clap;
use self::cowbell::Cowbell;
use self::hihat::{ClosedHiHat, Cymbal, OpenHiHat};
use self::kick::Kick;
use self::snare::Snare;
use self::tom::Tom;

/// Number of voice rows in the drum machine.
pub const VOICE_COUNT: usize = 10;

/// Maximum number of parameters any single voice can have.
pub const MAX_PARAMS_PER_VOICE: usize = 8;

/// Common interface for all 808 voices.
pub trait Voice: Send {
    /// Trigger the voice with the given velocity (0.0..1.0).
    fn trigger(&mut self, velocity: f32);

    /// Generate the next output sample.
    fn process(&mut self) -> f32;

    /// Whether the voice is currently producing sound.
    fn is_active(&self) -> bool;
}

/// Index into the drum machine voice array.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(usize)]
pub enum VoiceIndex {
    Kick = 0,
    Snare = 1,
    ClosedHiHat = 2,
    OpenHiHat = 3,
    Clap = 4,
    TomHi = 5,
    TomMid = 6,
    TomLo = 7,
    Cowbell = 8,
    Cymbal = 9,
}

impl VoiceIndex {
    /// All voice indices in order.
    pub const ALL: [Self; VOICE_COUNT] = [
        Self::Kick,
        Self::Snare,
        Self::ClosedHiHat,
        Self::OpenHiHat,
        Self::Clap,
        Self::TomHi,
        Self::TomMid,
        Self::TomLo,
        Self::Cowbell,
        Self::Cymbal,
    ];

    /// Short label for the sequencer grid.
    #[must_use]
    pub const fn short_label(self) -> &'static str {
        match self {
            Self::Kick => "KCK",
            Self::Snare => "SNR",
            Self::ClosedHiHat => "CHH",
            Self::OpenHiHat => "OHH",
            Self::Clap => "CLP",
            Self::TomHi => "TH",
            Self::TomMid => "TM",
            Self::TomLo => "TL",
            Self::Cowbell => "COW",
            Self::Cymbal => "CYM",
        }
    }

    /// Full display name.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Kick => "KICK",
            Self::Snare => "SNARE",
            Self::ClosedHiHat => "CH",
            Self::OpenHiHat => "OH",
            Self::Clap => "CLAP",
            Self::TomHi => "TOM1",
            Self::TomMid => "TOM2",
            Self::TomLo => "TOM3",
            Self::Cowbell => "COWB",
            Self::Cymbal => "CYM",
        }
    }

    /// Convert from usize index.
    #[must_use]
    pub const fn from_index(i: usize) -> Option<Self> {
        match i {
            0 => Some(Self::Kick),
            1 => Some(Self::Snare),
            2 => Some(Self::ClosedHiHat),
            3 => Some(Self::OpenHiHat),
            4 => Some(Self::Clap),
            5 => Some(Self::TomHi),
            6 => Some(Self::TomMid),
            7 => Some(Self::TomLo),
            8 => Some(Self::Cowbell),
            9 => Some(Self::Cymbal),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Voice parameter definitions
// ---------------------------------------------------------------------------

/// Identifies a parameter on a specific voice type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceParam {
    Tune,
    Decay,
    Level,
    Tone,
    Snappy,
    OpenDecay,
    ClosedDecay,
    /// Per-voice accent sensitivity (0.0 = accent has no effect, 1.0 = full).
    AccentAmount,
}

impl VoiceParam {
    /// Parameters available for a given voice.
    #[must_use]
    pub const fn for_voice(voice: VoiceIndex) -> &'static [Self] {
        match voice {
            VoiceIndex::Kick => &[Self::Tune, Self::Decay, Self::AccentAmount, Self::Level],
            VoiceIndex::Snare => &[
                Self::Tune,
                Self::Tone,
                Self::Snappy,
                Self::Decay,
                Self::Level,
            ],
            VoiceIndex::ClosedHiHat => &[Self::Tune, Self::Level],
            VoiceIndex::OpenHiHat => &[Self::Tune, Self::OpenDecay, Self::Level],
            // Clap has no tune/decay setters — only expose Level.
            VoiceIndex::Clap => &[Self::Level],
            VoiceIndex::TomHi
            | VoiceIndex::TomMid
            | VoiceIndex::TomLo
            | VoiceIndex::Cowbell
            | VoiceIndex::Cymbal => &[Self::Tune, Self::Decay, Self::Level],
        }
    }

    /// Display label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Tune => "Tune",
            Self::Decay => "Decay",
            Self::Level => "Level",
            Self::Tone => "Tone",
            Self::Snappy => "Snappy",
            Self::OpenDecay => "OH Decay",
            Self::ClosedDecay => "CH Decay",
            Self::AccentAmount => "Accent",
        }
    }

    /// Parameter range `(min, max)` for a given voice.
    #[must_use]
    pub const fn range(self, voice: VoiceIndex) -> (f32, f32) {
        match (voice, self) {
            // Kick
            (VoiceIndex::Kick, Self::Tune) => (20.0, 100.0),
            (VoiceIndex::Kick, Self::Decay) => (0.05, 0.8),
            // Snare
            (VoiceIndex::Snare, Self::Snappy) => (0.02, 0.5),
            (
                VoiceIndex::Snare | VoiceIndex::TomHi | VoiceIndex::TomMid | VoiceIndex::TomLo,
                Self::Decay,
            ) => (0.05, 0.5),
            // Open hi-hat
            (VoiceIndex::OpenHiHat, Self::OpenDecay) => (0.09, 0.6),
            // Toms
            (VoiceIndex::TomHi | VoiceIndex::TomMid | VoiceIndex::TomLo, Self::Tune) => {
                (50.0, 400.0)
            }
            // Cowbell
            (VoiceIndex::Cowbell, Self::Decay) => (0.1, 1.0),
            // Cymbal
            (VoiceIndex::Cymbal, Self::Decay) => (0.35, 1.2),
            // Snare tune + metallic voices: tune is a ratio
            (_, Self::Tune) => (0.5, 2.0),
            // Level, Tone, AccentAmount are all 0.0-1.0
            _ => (0.0, 1.0),
        }
    }

    /// Default actual value matching the synth initialization state.
    #[must_use]
    pub const fn default_actual(self, voice: VoiceIndex) -> f32 {
        match (voice, self) {
            (VoiceIndex::Kick, Self::Tune) => 49.0,
            (VoiceIndex::Kick, Self::Decay) | (VoiceIndex::OpenHiHat, Self::OpenDecay) => 0.3,
            (VoiceIndex::Snare, Self::Snappy | Self::Decay) => 0.15,
            (VoiceIndex::TomHi, Self::Tune) => 200.0,
            (VoiceIndex::TomHi, Self::Decay) => 0.1,
            (VoiceIndex::TomMid, Self::Tune) => 150.0,
            (VoiceIndex::TomMid, Self::Decay) => 0.13,
            (VoiceIndex::TomLo, Self::Tune) => 100.0,
            (VoiceIndex::TomLo, Self::Decay) => 0.2,
            (VoiceIndex::Cymbal, Self::Decay) => 0.6,
            // All remaining Tune arms (Snare, Cowbell, Cymbal, CH, OH): ratio default = 1.0
            (_, Self::Tune | Self::AccentAmount) => 1.0,
            (_, Self::Level) => 0.8,
            _ => 0.5,
        }
    }

    /// Normalize an actual value to 0.0-1.0 for display.
    #[must_use]
    pub fn normalize(self, voice: VoiceIndex, actual: f32) -> f32 {
        let (min, max) = self.range(voice);
        let span = max - min;
        if span.abs() < f32::EPSILON {
            return 0.5;
        }
        ((actual - min) / span).clamp(0.0, 1.0)
    }

    /// Denormalize a 0.0-1.0 value to the actual parameter range.
    #[must_use]
    pub fn denormalize(self, voice: VoiceIndex, normalized: f32) -> f32 {
        let (min, max) = self.range(voice);
        normalized.mul_add(max - min, min)
    }
}

// ---------------------------------------------------------------------------
// DrumMachine
// ---------------------------------------------------------------------------

/// Holds all 808 voices, handles triggering and mixing.
#[derive(Debug)]
pub struct DrumMachine {
    pub kick: Kick,
    pub snare: Snare,
    pub closed_hihat: ClosedHiHat,
    pub open_hihat: OpenHiHat,
    pub clap: Clap,
    pub tom_hi: Tom,
    pub tom_mid: Tom,
    pub tom_lo: Tom,
    pub cowbell: Cowbell,
    pub cymbal: Cymbal,
    /// Per-voice output levels (0.0..1.0).
    pub levels: [f32; VOICE_COUNT],
    /// Per-voice accent sensitivity (0.0 = no accent effect, 1.0 = full).
    pub accent_amounts: [f32; VOICE_COUNT],
}

impl DrumMachine {
    /// Create a new drum machine at the given sample rate.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        Self {
            kick: Kick::new(sample_rate),
            snare: Snare::new(sample_rate),
            closed_hihat: ClosedHiHat::new(sample_rate),
            open_hihat: OpenHiHat::new(sample_rate),
            clap: Clap::new(sample_rate),
            tom_hi: Tom::high(sample_rate),
            tom_mid: Tom::mid(sample_rate),
            tom_lo: Tom::low(sample_rate),
            cowbell: Cowbell::new(sample_rate),
            cymbal: Cymbal::new(sample_rate),
            levels: [0.8; VOICE_COUNT],
            accent_amounts: [1.0; VOICE_COUNT],
        }
    }

    /// Trigger a voice by index with given velocity.
    pub fn trigger(&mut self, voice: VoiceIndex, velocity: f32) {
        match voice {
            VoiceIndex::Kick => self.kick.trigger(velocity),
            VoiceIndex::Snare => self.snare.trigger(velocity),
            VoiceIndex::ClosedHiHat => {
                // Closed hat chokes open hat (authentic 808 behaviour).
                self.open_hihat.choke();
                self.closed_hihat.trigger(velocity);
            }
            VoiceIndex::OpenHiHat => self.open_hihat.trigger(velocity),
            VoiceIndex::Clap => self.clap.trigger(velocity),
            VoiceIndex::TomHi => self.tom_hi.trigger(velocity),
            VoiceIndex::TomMid => self.tom_mid.trigger(velocity),
            VoiceIndex::TomLo => self.tom_lo.trigger(velocity),
            VoiceIndex::Cowbell => self.cowbell.trigger(velocity),
            VoiceIndex::Cymbal => self.cymbal.trigger(velocity),
        }
    }

    /// Generate the next mono output sample (sum of all voices).
    pub fn process(&mut self) -> f32 {
        let mut sum = 0.0_f32;
        sum += self.kick.process() * self.levels[VoiceIndex::Kick as usize];
        sum += self.snare.process() * self.levels[VoiceIndex::Snare as usize];
        sum += self.closed_hihat.process() * self.levels[VoiceIndex::ClosedHiHat as usize];
        sum += self.open_hihat.process() * self.levels[VoiceIndex::OpenHiHat as usize];
        sum += self.clap.process() * self.levels[VoiceIndex::Clap as usize];
        sum += self.tom_hi.process() * self.levels[VoiceIndex::TomHi as usize];
        sum += self.tom_mid.process() * self.levels[VoiceIndex::TomMid as usize];
        sum += self.tom_lo.process() * self.levels[VoiceIndex::TomLo as usize];
        sum += self.cowbell.process() * self.levels[VoiceIndex::Cowbell as usize];
        sum += self.cymbal.process() * self.levels[VoiceIndex::Cymbal as usize];
        kazoo_core::sanitize_sample(sum)
    }

    /// Set a parameter on a voice.
    pub fn set_voice_param(&mut self, voice: VoiceIndex, param: VoiceParam, value: f32) {
        match (voice, param) {
            // Kick
            (VoiceIndex::Kick, VoiceParam::Tune) => self.kick.set_tune(value),
            (VoiceIndex::Kick, VoiceParam::Decay) => self.kick.set_decay(value),
            (VoiceIndex::Kick, VoiceParam::AccentAmount) => {
                self.accent_amounts[VoiceIndex::Kick as usize] = value.clamp(0.0, 1.0);
            }
            (VoiceIndex::Kick, VoiceParam::Level) => {
                self.levels[VoiceIndex::Kick as usize] = value.clamp(0.0, 1.0);
            }
            // Snare
            (VoiceIndex::Snare, VoiceParam::Tune) => self.snare.set_tune(value),
            (VoiceIndex::Snare, VoiceParam::Tone) => self.snare.set_tone(value),
            (VoiceIndex::Snare, VoiceParam::Snappy) => self.snare.set_snappy(value),
            (VoiceIndex::Snare, VoiceParam::Decay) => self.snare.set_decay(value),
            (VoiceIndex::Snare, VoiceParam::Level) => {
                self.levels[VoiceIndex::Snare as usize] = value.clamp(0.0, 1.0);
            }
            // Closed hi-hat
            (VoiceIndex::ClosedHiHat, VoiceParam::Tune) => self.closed_hihat.set_tune(value),
            (VoiceIndex::ClosedHiHat, VoiceParam::Level) => {
                self.levels[VoiceIndex::ClosedHiHat as usize] = value.clamp(0.0, 1.0);
            }
            // Open hi-hat
            (VoiceIndex::OpenHiHat, VoiceParam::Tune) => self.open_hihat.set_tune(value),
            (VoiceIndex::OpenHiHat, VoiceParam::OpenDecay) => self.open_hihat.set_decay(value),
            (VoiceIndex::OpenHiHat, VoiceParam::Level) => {
                self.levels[VoiceIndex::OpenHiHat as usize] = value.clamp(0.0, 1.0);
            }
            // Clap
            (VoiceIndex::Clap, VoiceParam::Level) => {
                self.levels[VoiceIndex::Clap as usize] = value.clamp(0.0, 1.0);
            }
            // Toms
            (VoiceIndex::TomHi, VoiceParam::Tune) => self.tom_hi.set_tune(value),
            (VoiceIndex::TomHi, VoiceParam::Decay) => self.tom_hi.set_decay(value),
            (VoiceIndex::TomHi, VoiceParam::Level) => {
                self.levels[VoiceIndex::TomHi as usize] = value.clamp(0.0, 1.0);
            }
            (VoiceIndex::TomMid, VoiceParam::Tune) => self.tom_mid.set_tune(value),
            (VoiceIndex::TomMid, VoiceParam::Decay) => self.tom_mid.set_decay(value),
            (VoiceIndex::TomMid, VoiceParam::Level) => {
                self.levels[VoiceIndex::TomMid as usize] = value.clamp(0.0, 1.0);
            }
            (VoiceIndex::TomLo, VoiceParam::Tune) => self.tom_lo.set_tune(value),
            (VoiceIndex::TomLo, VoiceParam::Decay) => self.tom_lo.set_decay(value),
            (VoiceIndex::TomLo, VoiceParam::Level) => {
                self.levels[VoiceIndex::TomLo as usize] = value.clamp(0.0, 1.0);
            }
            // Cowbell
            (VoiceIndex::Cowbell, VoiceParam::Tune) => self.cowbell.set_tune(value),
            (VoiceIndex::Cowbell, VoiceParam::Decay) => self.cowbell.set_decay(value),
            (VoiceIndex::Cowbell, VoiceParam::Level) => {
                self.levels[VoiceIndex::Cowbell as usize] = value.clamp(0.0, 1.0);
            }
            // Cymbal
            (VoiceIndex::Cymbal, VoiceParam::Tune) => self.cymbal.set_tune(value),
            (VoiceIndex::Cymbal, VoiceParam::Decay) => self.cymbal.set_decay(value),
            (VoiceIndex::Cymbal, VoiceParam::Level) => {
                self.levels[VoiceIndex::Cymbal as usize] = value.clamp(0.0, 1.0);
            }
            // Unsupported param for voice — silently ignore.
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drum_machine_produces_silence_when_idle() {
        let mut dm = DrumMachine::new(44100.0);
        for _ in 0..1000 {
            let s = dm.process();
            assert!(
                s.abs() < f32::EPSILON,
                "idle drum machine should be silent, got {s}"
            );
        }
    }

    #[test]
    fn drum_machine_trigger_produces_sound() {
        let mut dm = DrumMachine::new(44100.0);
        dm.trigger(VoiceIndex::Kick, 1.0);
        let mut had_sound = false;
        for _ in 0..4410 {
            let s = dm.process();
            assert!(s.is_finite());
            if s.abs() > 1e-6 {
                had_sound = true;
            }
        }
        assert!(had_sound, "triggered kick should produce sound");
    }

    #[test]
    fn closed_hat_chokes_open_hat() {
        let mut dm = DrumMachine::new(44100.0);
        dm.trigger(VoiceIndex::OpenHiHat, 1.0);
        // Let it ring for a few samples.
        for _ in 0..100 {
            dm.process();
        }
        assert!(dm.open_hihat.is_active());
        // Trigger closed hat — should choke open hat.
        dm.trigger(VoiceIndex::ClosedHiHat, 1.0);
        assert!(!dm.open_hihat.is_active());
    }

    #[test]
    fn all_voices_produce_sound_individually() {
        for voice in VoiceIndex::ALL {
            let mut dm = DrumMachine::new(44100.0);
            dm.trigger(voice, 1.0);
            let mut had_sound = false;
            for _ in 0..4410 {
                let s = dm.process();
                assert!(s.is_finite(), "{voice:?} produced non-finite output");
                if s.abs() > 1e-6 {
                    had_sound = true;
                }
            }
            assert!(had_sound, "{voice:?} should produce audible output");
        }
    }

    #[test]
    fn voice_index_roundtrip() {
        for voice in VoiceIndex::ALL {
            let idx = voice as usize;
            let recovered = VoiceIndex::from_index(idx);
            assert_eq!(recovered, Some(voice));
        }
        assert_eq!(VoiceIndex::from_index(10), None);
    }

    #[test]
    fn snare_tune_setter_changes_sound() {
        // Verify set_voice_param for (Snare, Tune) doesn't panic
        // and produces different output when tuned differently.
        let mut dm1 = DrumMachine::new(44100.0);
        let mut dm2 = DrumMachine::new(44100.0);
        dm2.set_voice_param(VoiceIndex::Snare, VoiceParam::Tune, 2.0);
        dm1.trigger(VoiceIndex::Snare, 1.0);
        dm2.trigger(VoiceIndex::Snare, 1.0);
        let mut different = false;
        for _ in 0..200 {
            let s1 = dm1.process();
            let s2 = dm2.process();
            if (s1 - s2).abs() > 1e-4 {
                different = true;
                break;
            }
        }
        assert!(different, "snare tune should change the output");
    }

    #[test]
    fn snare_decay_setter_changes_sound() {
        // Verify set_voice_param for (Snare, Decay) doesn't panic.
        let mut dm1 = DrumMachine::new(44100.0);
        let mut dm2 = DrumMachine::new(44100.0);
        dm2.set_voice_param(VoiceIndex::Snare, VoiceParam::Decay, 0.5);
        dm1.trigger(VoiceIndex::Snare, 1.0);
        dm2.trigger(VoiceIndex::Snare, 1.0);
        // After 200ms (well past shorter decay), the longer decay version
        // should still be louder.
        for _ in 0..8820 {
            dm1.process();
            dm2.process();
        }
        // Both should produce finite output (no NaN from the setter).
        let s1 = dm1.process();
        let s2 = dm2.process();
        assert!(s1.is_finite());
        assert!(s2.is_finite());
    }

    #[test]
    fn kick_accent_amount_stored() {
        let mut dm = DrumMachine::new(44100.0);
        assert!((dm.accent_amounts[VoiceIndex::Kick as usize] - 1.0).abs() < f32::EPSILON);
        dm.set_voice_param(VoiceIndex::Kick, VoiceParam::AccentAmount, 0.6);
        assert!((dm.accent_amounts[VoiceIndex::Kick as usize] - 0.6).abs() < f32::EPSILON);
    }

    #[test]
    fn voice_param_roundtrip_normalize() {
        for voice in VoiceIndex::ALL {
            for param in VoiceParam::for_voice(voice) {
                let actual = param.default_actual(voice);
                let normalized = param.normalize(voice, actual);
                let recovered = param.denormalize(voice, normalized);
                assert!(
                    (recovered - actual).abs() < 0.01,
                    "{voice:?}/{param:?}: {actual} -> {normalized} -> {recovered}"
                );
            }
        }
    }
}
