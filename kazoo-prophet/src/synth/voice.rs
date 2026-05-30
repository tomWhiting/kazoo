//! One Prophet voice.

use kazoo_core::{midi_note_to_frequency, sanitize_sample};

use super::envelope::{AdsrEnvelope, EnvelopeStage};
use super::filter::CurtisLowPass;
use super::noise::WhiteNoise;
use super::oscillator::Oscillator;
use super::params::{EnvelopeParams, OscillatorParams, SynthParams};

/// Runtime voice state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceState {
    Free,
    Active,
    Releasing,
}

/// Complete dual-VCO voice.
#[derive(Debug, Clone)]
pub struct ProphetVoice {
    pub osc_a: Oscillator,
    pub osc_b: Oscillator,
    pub filter: CurtisLowPass,
    pub filter_env: AdsrEnvelope,
    pub amp_env: AdsrEnvelope,
    pub age: u64,
    note: Option<u8>,
    velocity: f32,
    sample_rate: f32,
    state: VoiceState,
    drift_cents: f32,
    last_osc_b: f32,
    seed: u32,
    noise: WhiteNoise,
}

impl ProphetVoice {
    #[must_use]
    pub fn new(index: u8, sample_rate: f32) -> Self {
        let seed = 0x9E37_79B9 ^ u32::from(index);
        let mut osc_b = Oscillator::new(sample_rate);
        osc_b.apply_params(&OscillatorParams::oscillator_b_default());

        Self {
            osc_a: Oscillator::new(sample_rate),
            osc_b,
            filter: CurtisLowPass::new(sample_rate),
            filter_env: AdsrEnvelope::new(sample_rate),
            amp_env: AdsrEnvelope::new(sample_rate),
            age: 0,
            note: None,
            velocity: 0.0,
            sample_rate,
            state: VoiceState::Free,
            drift_cents: 0.0,
            last_osc_b: 0.0,
            seed,
            noise: WhiteNoise::new(seed),
        }
    }

    pub const fn apply_params(&mut self, params: &SynthParams) {
        self.osc_a.apply_params(&params.oscillator_a);
        self.osc_b.apply_params(&params.oscillator_b);

        self.filter.cutoff_hz = params.filter.cutoff_hz;
        self.filter.resonance = params.filter.resonance;
        self.filter.key_track = params.filter.key_track;

        apply_envelope_params(&mut self.filter_env, params.filter_envelope);
        apply_envelope_params(&mut self.amp_env, params.amplifier_envelope);
    }

    pub fn note_on(&mut self, note: u8, velocity: f32, drift_depth_cents: f32) {
        self.note = Some(note);
        self.velocity = velocity.clamp(0.0, 1.0);
        self.state = VoiceState::Active;
        self.drift_cents = self.noise.next_bipolar() * drift_depth_cents;
        self.last_osc_b = 0.0;
        self.filter_env.gate_on();
        self.amp_env.gate_on();
    }

    pub fn note_off(&mut self) {
        if self.state == VoiceState::Active {
            self.state = VoiceState::Releasing;
            self.filter_env.gate_off();
            self.amp_env.gate_off();
        }
    }

    #[inline]
    pub fn process(&mut self, params: &SynthParams) -> f32 {
        let Some(note) = self.note else {
            return 0.0;
        };

        let freq = midi_note_to_frequency(note);
        let filter_env = self.filter_env.tick();
        let amp_env = self.amp_env.tick();
        if self.amp_env.stage() == EnvelopeStage::Idle {
            self.state = VoiceState::Free;
            self.note = None;
            return 0.0;
        }

        let filter_env_bipolar = filter_env.mul_add(2.0, -1.0);
        let osc_b_to_a = self.last_osc_b * params.poly_mod.osc_b_to_osc_a_cents;
        let filter_env_to_a = filter_env_bipolar * params.poly_mod.filter_env_to_osc_a_cents;
        let freq_a = self
            .osc_a
            .effective_frequency(freq, self.drift_cents + osc_b_to_a + filter_env_to_a);
        let freq_b = self.osc_b.effective_frequency(
            freq,
            self.drift_cents * params.drift.oscillator_b_detune_scale,
        );

        let (osc_a, a_wrapped) = self.osc_a.tick(freq_a, false);
        if params.poly_mod.oscillator_sync && a_wrapped {
            self.osc_b.sync_reset();
        }
        let (osc_b, _) = self.osc_b.tick(freq_b, false);
        self.last_osc_b = osc_b;

        let noise = self.noise.next_bipolar() * params.mixer.noise_level;
        let mixed = (osc_a + osc_b + noise) * params.mixer.pre_filter_gain;
        let keyboard_tracking = (freq / 261.63).max(0.25).log2() * 1200.0 * self.filter.key_track;
        let key_cutoff = params.filter.cutoff_hz * (keyboard_tracking / 1200.0).exp2();
        let envelope_cutoff = filter_env * params.filter.envelope_amount * 6000.0;
        let poly_filter_cutoff = osc_b.mul_add(
            params.poly_mod.osc_b_to_filter_hz,
            filter_env * params.poly_mod.filter_env_to_filter_hz,
        );
        let cutoff = key_cutoff + envelope_cutoff + poly_filter_cutoff;
        let filtered = self.filter.process(mixed, cutoff, params.filter.resonance);

        sanitize_sample(filtered * amp_env * self.velocity.sqrt() * params.master_level)
    }

    #[must_use]
    pub const fn note(&self) -> Option<u8> {
        self.note
    }

    #[must_use]
    pub const fn state(&self) -> VoiceState {
        self.state
    }

    #[must_use]
    pub const fn drift_cents(&self) -> f32 {
        self.drift_cents
    }

    pub const fn reset(&mut self) {
        self.osc_a.reset();
        self.osc_b.reset();
        self.filter.reset();
        self.filter_env.reset();
        self.amp_env.reset();
        self.note = None;
        self.state = VoiceState::Free;
        self.velocity = 0.0;
        self.last_osc_b = 0.0;
        self.noise.reset(self.seed);
    }

    pub const fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        self.osc_a.set_sample_rate(self.sample_rate);
        self.osc_b.set_sample_rate(self.sample_rate);
        self.filter.set_sample_rate(self.sample_rate);
        self.filter_env.set_sample_rate(self.sample_rate);
        self.amp_env.set_sample_rate(self.sample_rate);
    }
}

const fn apply_envelope_params(envelope: &mut AdsrEnvelope, params: EnvelopeParams) {
    envelope.attack = params.attack;
    envelope.decay = params.decay;
    envelope.sustain = params.sustain;
    envelope.release = params.release;
}
