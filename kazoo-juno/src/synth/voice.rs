//! Six-voice DCO/sub/noise voice implementation.

use std::f32::consts::TAU;

use super::envelope::{AdsrEnvelope, EnvelopeStage};
use super::filter::{DcBlockHighPass, LowPassFilter};
use super::params::SynthParams;

#[derive(Debug, Clone, Copy)]
pub struct VoiceStatus {
    pub index: u8,
    pub active: bool,
    pub releasing: bool,
    pub note: Option<u8>,
    pub drift_cents: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceState {
    Free,
    Active,
    Releasing,
}

#[derive(Debug, Clone)]
pub struct JunoVoice {
    index: u8,
    sample_rate: f32,
    note: Option<u8>,
    frequency: f32,
    velocity: f32,
    phase: f32,
    sub_phase: f32,
    lfo_phase: f32,
    drift_cents: f32,
    age: u64,
    envelope: AdsrEnvelope,
    hpf: DcBlockHighPass,
    lpf: LowPassFilter,
    noise_state: u32,
}

impl JunoVoice {
    #[must_use]
    pub fn new(index: u8, sample_rate: f32) -> Self {
        Self {
            index,
            sample_rate: sample_rate.max(1.0),
            note: None,
            frequency: 440.0,
            velocity: 0.0,
            phase: (f32::from(index) * 0.137).fract(),
            sub_phase: (f32::from(index) * 0.271).fract(),
            lfo_phase: (f32::from(index) * 0.083).fract(),
            drift_cents: 0.0,
            age: 0,
            envelope: AdsrEnvelope::new(sample_rate.max(1.0)),
            hpf: DcBlockHighPass::new(sample_rate.max(1.0)),
            lpf: LowPassFilter::new(sample_rate.max(1.0)),
            noise_state: 0x1234_abcd ^ (u32::from(index) * 0x1f12_3bb5),
        }
    }

    pub const fn age(&self) -> u64 {
        self.age
    }

    pub const fn set_age(&mut self, age: u64) {
        self.age = age;
    }

    pub const fn note(&self) -> Option<u8> {
        self.note
    }

    pub const fn state(&self) -> VoiceState {
        if self.envelope.is_idle() {
            VoiceState::Free
        } else if matches!(self.envelope.stage(), EnvelopeStage::Release) {
            VoiceState::Releasing
        } else {
            VoiceState::Active
        }
    }

    pub const fn drift_cents(&self) -> f32 {
        self.drift_cents
    }

    pub fn note_on(&mut self, note: u8, velocity: f32, drift_spread_cents: f32) {
        self.note = Some(note);
        self.velocity = velocity.clamp(0.0, 1.0);
        let centered = f32::from(self.index) - 2.5;
        self.drift_cents = centered * drift_spread_cents * 0.37;
        self.frequency = midi_to_hz(note) * (self.drift_cents / 1200.0).exp2();
        self.envelope.note_on();
    }

    pub const fn note_off(&mut self) {
        self.envelope.note_off();
    }

    pub const fn reset(&mut self) {
        self.note = None;
        self.velocity = 0.0;
        self.envelope.reset();
        self.hpf.reset();
        self.lpf.reset();
    }

    pub const fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        self.envelope.set_sample_rate(self.sample_rate);
        self.hpf.set_sample_rate(self.sample_rate);
        self.lpf.set_sample_rate(self.sample_rate);
    }

    pub fn process(&mut self, params: &SynthParams) -> f32 {
        if self.state() == VoiceState::Free {
            self.note = None;
            return 0.0;
        }

        let env = self.envelope.process(&params.envelope);
        let lfo = (self.lfo_phase * TAU).sin();
        self.lfo_phase = (self.lfo_phase + params.dco.lfo_rate_hz / self.sample_rate).fract();
        let pulse_width = params.dco.pwm_depth.mul_add(lfo, params.dco.pulse_width).clamp(0.08, 0.92);

        let saw = self.phase.mul_add(2.0, -1.0);
        let pulse = if self.phase < pulse_width { 1.0 } else { -1.0 };
        let sub = if self.sub_phase < 0.5 { 1.0 } else { -1.0 };
        let noise = self.white_noise();

        let mix = noise.mul_add(
            params.dco.noise_level,
            saw * params.dco.saw_level + pulse * params.dco.pulse_level + sub * params.dco.sub_level,
        );

        self.advance_phases();

        let hpf = self.hpf.process(mix * 0.42, params.filter.hpf_amount);
        let note_hz = self.frequency.max(1.0);
        let key_track = (note_hz / midi_to_hz(60)).powf(params.filter.key_track.clamp(0.0, 1.0));
        let cutoff = params.filter.cutoff_hz.mul_add(
            key_track,
            env.powf(1.4) * params.filter.envelope_amount * 7000.0,
        );
        let filtered = self
            .lpf
            .process(hpf, cutoff, params.filter.resonance.clamp(0.0, 0.96));
        filtered * env * self.velocity
    }

    fn advance_phases(&mut self) {
        self.phase = (self.phase + self.frequency / self.sample_rate).fract();
        self.sub_phase = (self.sub_phase + self.frequency * 0.5 / self.sample_rate).fract();
    }

    fn white_noise(&mut self) -> f32 {
        self.noise_state = self
            .noise_state
            .wrapping_mul(1_103_515_245)
            .wrapping_add(12_345);
        let unit = ((self.noise_state / 65_536) % 32_768) as f32 / 32_768.0;
        unit * 2.0 - 1.0
    }
}

fn midi_to_hz(note: u8) -> f32 {
    440.0 * ((f32::from(note) - 69.0) / 12.0).exp2()
}
