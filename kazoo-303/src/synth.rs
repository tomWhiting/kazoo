//! Procedural 303-inspired bass synth voice.
//!
//! The signal path is oscillator -> accent VCA -> resonant low-pass -> soft clip.
//! No sample playback is used anywhere in this module.

use std::f32::consts::TAU;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waveform {
    Saw,
    Square,
}

impl Waveform {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Saw => "saw",
            Self::Square => "square",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcidSynthParam {
    Cutoff,
    Resonance,
    EnvMod,
    Decay,
    Accent,
    SlideTime,
    Drive,
}

impl AcidSynthParam {
    pub const ALL: [Self; 7] = [
        Self::Cutoff,
        Self::Resonance,
        Self::EnvMod,
        Self::Decay,
        Self::Accent,
        Self::SlideTime,
        Self::Drive,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Cutoff => "cutoff",
            Self::Resonance => "resonance",
            Self::EnvMod => "env mod",
            Self::Decay => "decay",
            Self::Accent => "accent",
            Self::SlideTime => "slide",
            Self::Drive => "drive",
        }
    }
}

#[derive(Debug)]
pub struct AcidSynth {
    sample_rate: f32,
    waveform: Waveform,
    phase: f32,
    current_freq: f32,
    target_freq: f32,
    gate: bool,
    amp_env: f32,
    filter_env: f32,
    accent_env: f32,
    cutoff: f32,
    resonance: f32,
    env_mod: f32,
    decay: f32,
    accent_amount: f32,
    slide_time: f32,
    drive: f32,
    filter: DiodeLowPass,
}

impl AcidSynth {
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            waveform: Waveform::Saw,
            phase: 0.0,
            current_freq: midi_to_hz(36),
            target_freq: midi_to_hz(36),
            gate: false,
            amp_env: 0.0,
            filter_env: 0.0,
            accent_env: 0.0,
            cutoff: 0.36,
            resonance: 0.68,
            env_mod: 0.72,
            decay: 0.46,
            accent_amount: 0.74,
            slide_time: 0.32,
            drive: 0.38,
            filter: DiodeLowPass::new(),
        }
    }

    pub const fn waveform(&self) -> Waveform {
        self.waveform
    }

    pub const fn set_waveform(&mut self, waveform: Waveform) {
        self.waveform = waveform;
    }

    pub const fn toggle_waveform(&mut self) -> Waveform {
        self.waveform = match self.waveform {
            Waveform::Saw => Waveform::Square,
            Waveform::Square => Waveform::Saw,
        };
        self.waveform
    }

    #[must_use]
    pub const fn param_value(&self, param: AcidSynthParam) -> f32 {
        match param {
            AcidSynthParam::Cutoff => self.cutoff,
            AcidSynthParam::Resonance => self.resonance,
            AcidSynthParam::EnvMod => self.env_mod,
            AcidSynthParam::Decay => self.decay,
            AcidSynthParam::Accent => self.accent_amount,
            AcidSynthParam::SlideTime => self.slide_time,
            AcidSynthParam::Drive => self.drive,
        }
    }

    pub const fn set_param(&mut self, param: AcidSynthParam, value: f32) {
        let value = value.clamp(0.0, 1.0);
        match param {
            AcidSynthParam::Cutoff => self.cutoff = value,
            AcidSynthParam::Resonance => self.resonance = value,
            AcidSynthParam::EnvMod => self.env_mod = value,
            AcidSynthParam::Decay => self.decay = value,
            AcidSynthParam::Accent => self.accent_amount = value,
            AcidSynthParam::SlideTime => self.slide_time = value,
            AcidSynthParam::Drive => self.drive = value,
        }
    }

    pub fn adjust_param(&mut self, param: AcidSynthParam, delta: f32) -> f32 {
        let value = self.param_value(param) + delta;
        self.set_param(param, value);
        self.param_value(param)
    }

    pub fn note_on(&mut self, note: i8, accent: bool, slide: bool) {
        if note < 0 {
            self.gate = false;
            return;
        }

        self.target_freq = midi_to_hz(note);
        if slide {
            self.amp_env = self.amp_env.max(0.82);
            self.filter_env = self.filter_env.max(0.72);
        } else {
            self.current_freq = self.target_freq;
            self.amp_env = 1.0;
            self.filter_env = 1.0;
        }
        self.accent_env = if accent { 1.0 } else { 0.0 };
        self.gate = true;
    }

    pub const fn release(&mut self) {
        self.gate = false;
    }

    pub fn process(&mut self) -> f32 {
        self.glide_frequency();
        self.advance_envelopes();

        let oscillator = self.oscillator_sample();
        let accent_gain = (self.accent_env * self.accent_amount).mul_add(0.8, 1.0);
        let driven = (oscillator * accent_gain * self.drive.mul_add(5.0, 1.0)).tanh();

        let base_cutoff = exp_map(self.cutoff, 80.0, 2_800.0);
        let env_cutoff = self.filter_env * self.env_mod * 4_800.0;
        let accent_cutoff = self.accent_env * self.accent_amount * 1_800.0;
        let cutoff_hz = (base_cutoff + env_cutoff + accent_cutoff).clamp(45.0, 12_000.0);

        let filtered = self.filter.process(
            driven,
            cutoff_hz,
            self.resonance,
            self.sample_rate,
        );
        (filtered * self.amp_env * 1.6).tanh()
    }

    fn oscillator_sample(&mut self) -> f32 {
        let increment = self.current_freq / self.sample_rate;
        self.phase = (self.phase + increment).fract();
        match self.waveform {
            Waveform::Saw => self.phase.mul_add(2.0, -1.0),
            Waveform::Square => {
                if self.phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
        }
    }

    fn glide_frequency(&mut self) {
        let time = self.slide_time.mul_add(0.22, 0.015);
        let coeff = (-1.0 / (time * self.sample_rate)).exp();
        self.current_freq = (self.current_freq - self.target_freq).mul_add(coeff, self.target_freq);
    }

    fn advance_envelopes(&mut self) {
        let decay_time = self.decay.mul_add(1.2, 0.08);
        let filter_coeff = (-1.0 / (decay_time * self.sample_rate)).exp();
        self.filter_env *= filter_coeff;

        let amp_time = if self.gate { 0.45 } else { 0.035 };
        let amp_coeff = (-1.0 / (amp_time * self.sample_rate)).exp();
        let amp_target = if self.gate { 0.88 } else { 0.0 };
        self.amp_env = (self.amp_env - amp_target).mul_add(amp_coeff, amp_target);

        let accent_coeff = (-1.0 / (0.13 * self.sample_rate)).exp();
        self.accent_env *= accent_coeff;
    }
}

/// A compact resonant low-pass inspired by diode ladder behavior.
///
/// This is not a circuit clone; it is a stable four-pole nonlinear filter chosen
/// for acid-style squelch in a simple real-time terminal synth.
#[derive(Debug, Default)]
struct DiodeLowPass {
    z1: f32,
    z2: f32,
    z3: f32,
    z4: f32,
}

impl DiodeLowPass {
    const fn new() -> Self {
        Self {
            z1: 0.0,
            z2: 0.0,
            z3: 0.0,
            z4: 0.0,
        }
    }

    fn process(&mut self, input: f32, cutoff_hz: f32, resonance: f32, sample_rate: f32) -> f32 {
        let normalized = (cutoff_hz / sample_rate).clamp(0.0001, 0.22);
        let g = (TAU * normalized).sin().clamp(0.0, 1.0);
        let feedback = resonance * 3.85;
        let x = self.z4.mul_add(-feedback, input).tanh();

        self.z1 += g * ((x - self.z1).tanh());
        self.z2 += g * ((self.z1 - self.z2).tanh());
        self.z3 += g * ((self.z2 - self.z3).tanh());
        self.z4 += g * ((self.z3 - self.z4).tanh());
        self.z4
    }
}

#[must_use]
pub fn midi_to_hz(note: i8) -> f32 {
    440.0 * ((f32::from(note) - 69.0) / 12.0).exp2()
}

fn exp_map(value: f32, min: f32, max: f32) -> f32 {
    min * (max / min).powf(value.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_frequency_reference() {
        assert!((midi_to_hz(69) - 440.0).abs() < 0.001);
    }

    #[test]
    fn synth_generates_math_not_samples() {
        let mut synth = AcidSynth::new(44_100.0);
        synth.note_on(36, true, false);
        let mut energy = 0.0;
        for _ in 0..1024 {
            energy += synth.process().abs();
        }
        assert!(energy > 0.1);
    }
}
