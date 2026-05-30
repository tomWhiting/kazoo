//! Prophet-style VCO with saw, pulse, and triangle outputs.

use std::f32::consts::TAU;

use super::params::OscillatorParams;

/// Oscillator waveform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Waveform {
    Saw,
    Pulse,
    Triangle,
}

/// Classic octave footage ranges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum OctaveRange {
    Footage32,
    Footage16,
    Footage8,
    Footage4,
}

impl OctaveRange {
    #[must_use]
    pub const fn multiplier(self) -> f32 {
        match self {
            Self::Footage32 => 0.25,
            Self::Footage16 => 0.5,
            Self::Footage8 => 1.0,
            Self::Footage4 => 2.0,
        }
    }
}

/// Free-running VCO.
#[derive(Debug, Clone)]
pub struct Oscillator {
    phase: f32,
    sample_rate: f32,
    pub waveform: Waveform,
    pub octave: OctaveRange,
    pub fine_tune_cents: f32,
    pub pulse_width: f32,
    pub level: f32,
}

impl Oscillator {
    #[must_use]
    pub const fn new(sample_rate: f32) -> Self {
        Self {
            phase: 0.0,
            sample_rate,
            waveform: Waveform::Saw,
            octave: OctaveRange::Footage8,
            fine_tune_cents: 0.0,
            pulse_width: 0.5,
            level: 0.8,
        }
    }

    pub const fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
    }

    pub const fn reset(&mut self) {
        self.phase = 0.0;
    }

    pub const fn apply_params(&mut self, params: &OscillatorParams) {
        self.waveform = params.waveform;
        self.octave = params.octave;
        self.fine_tune_cents = params.fine_tune_cents;
        self.pulse_width = params.pulse_width;
        self.level = params.level;
    }

    #[must_use]
    pub fn effective_frequency(&self, base_hz: f32, extra_cents: f32) -> f32 {
        let cents = self.fine_tune_cents + extra_cents;
        (base_hz * self.octave.multiplier() * (cents / 1200.0).exp2()).clamp(0.0, 20_000.0)
    }

    #[inline]
    pub fn tick(&mut self, freq_hz: f32, hard_sync: bool) -> (f32, bool) {
        let sr = self.sample_rate.max(1.0);
        let dt = (freq_hz / sr).clamp(0.0, 0.49);
        let sample = match self.waveform {
            Waveform::Saw => self.saw(dt),
            Waveform::Pulse => self.pulse(dt),
            Waveform::Triangle => (self.phase * TAU).sin().asin() * (2.0 / std::f32::consts::PI),
        } * self.level;

        self.phase += dt;
        let wrapped = self.phase >= 1.0;
        if wrapped || hard_sync {
            self.phase = self.phase.fract();
        }
        if !self.phase.is_finite() || self.phase < 0.0 {
            self.phase = 0.0;
        }

        (sample, wrapped)
    }

    pub const fn sync_reset(&mut self) {
        self.phase = 0.0;
    }

    #[inline]
    fn saw(&self, dt: f32) -> f32 {
        2.0f32.mul_add(self.phase, -1.0) - poly_blep(self.phase, dt)
    }

    #[inline]
    fn pulse(&self, dt: f32) -> f32 {
        let width = self.pulse_width.clamp(0.05, 0.95);
        let mut sample = if self.phase < width { 1.0 } else { -1.0 };
        sample += poly_blep(self.phase, dt);
        let edge = (self.phase - width).rem_euclid(1.0);
        sample -= poly_blep(edge, dt);
        sample
    }
}

#[inline]
fn poly_blep(t: f32, dt: f32) -> f32 {
    if dt <= 0.0 {
        return 0.0;
    }
    if t < dt {
        let x = t / dt;
        return x.mul_add(-x, x + x) - 1.0;
    }
    if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        return x * x + x + x + 1.0;
    }
    0.0
}
