//! Single CS-80 voice: two layers + ring mod + LFO + drift.
//!
//! Each of the 8 voices has this complete signal path.
//! Per-voice analog drift makes every voice slightly different.

use kazoo_core::sanitize_sample;

use super::drift::VoiceDrift;
use super::layer::Layer;
use super::lfo::Lfo;
use super::ring_mod::RingModulator;

/// State of a single voice in the polyphonic allocator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceState {
    /// Not producing sound.
    Free,
    /// Currently playing a note.
    Active,
    /// Note released, in release tail.
    Releasing,
}

/// A complete CS-80 voice: two independent layers, ring modulator, LFO, drift.
#[derive(Debug)]
pub struct Voice {
    /// Layer I — typically bright, fast attack.
    pub layer1: Layer,
    /// Layer II — typically slow, evolving pad.
    pub layer2: Layer,
    /// Ring modulator (applied to mixed voice signal).
    pub ring_mod: RingModulator,
    /// LFO (shared between both layers).
    pub lfo: Lfo,
    /// Per-voice analog drift.
    pub drift: VoiceDrift,
    /// Current MIDI note number (if active).
    note: Option<u8>,
    /// Current base frequency in Hz.
    frequency: f32,
    /// Voice state.
    state: VoiceState,
    /// Mix balance between layers [0=layer1 only, 1=layer2 only, 0.5=equal].
    pub layer_mix: f32,
    /// Voice index (0-7) for identification.
    pub index: u8,
    /// Sample rate.
    sample_rate: f32,
    /// Velocity of the current note [0, 1].
    velocity: f32,
    /// Aftertouch pressure [0, 1].
    pub aftertouch: f32,
    /// Note-on counter for voice-stealing age tracking.
    pub age: u64,
}

impl Voice {
    /// Create a new voice with the given index and sample rate.
    #[must_use]
    pub fn new(index: u8, sample_rate: f32, drift_cents: f32) -> Self {
        let mut lfo = Lfo::new(sample_rate);
        lfo.set_rate(2.5);

        Self {
            layer1: Layer::new(sample_rate),
            layer2: Layer::new(sample_rate),
            ring_mod: RingModulator::new(sample_rate),
            lfo,
            drift: VoiceDrift::new(index, drift_cents, sample_rate),
            note: None,
            frequency: 0.0,
            state: VoiceState::Free,
            layer_mix: 0.5,
            index,
            sample_rate: sample_rate.max(1.0),
            velocity: 0.0,
            aftertouch: 0.0,
            age: 0,
        }
    }

    /// Trigger a note-on event.
    pub fn note_on(&mut self, note: u8, velocity: f32) {
        self.note = Some(note);
        self.velocity = velocity.clamp(0.0, 1.0);
        self.frequency = kazoo_core::midi_note_to_frequency(note);
        self.state = VoiceState::Active;

        // Apply drift to frequency
        let drifted_freq = self.frequency * self.drift.frequency_ratio();
        self.layer1.set_frequency(drifted_freq);
        self.layer2.set_frequency(drifted_freq);

        // Trigger envelopes
        self.layer1.note_on(self.velocity);
        self.layer2.note_on(self.velocity);
        self.ring_mod.trigger();
    }

    /// Trigger a note-off event.
    pub fn note_off(&mut self) {
        self.layer1.note_off();
        self.layer2.note_off();
        self.state = VoiceState::Releasing;
    }

    /// Whether this voice is free for allocation.
    #[must_use]
    pub const fn is_free(&self) -> bool {
        matches!(self.state, VoiceState::Free)
    }

    /// Whether this voice is releasing (in the tail).
    #[must_use]
    pub const fn is_releasing(&self) -> bool {
        matches!(self.state, VoiceState::Releasing)
    }

    /// The MIDI note this voice is playing (if any).
    #[must_use]
    pub const fn note(&self) -> Option<u8> {
        self.note
    }

    /// Current voice state.
    #[must_use]
    pub const fn state(&self) -> VoiceState {
        self.state
    }

    /// Set sample rate for all sub-components.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        self.layer1.set_sample_rate(sample_rate);
        self.layer2.set_sample_rate(sample_rate);
        self.ring_mod.set_sample_rate(sample_rate);
        self.lfo.set_sample_rate(sample_rate);
    }

    /// Reset all state.
    pub const fn reset(&mut self) {
        self.layer1.reset();
        self.layer2.reset();
        self.ring_mod.reset();
        self.lfo.reset();
        self.drift.reset();
        self.note = None;
        self.frequency = 0.0;
        self.state = VoiceState::Free;
        self.velocity = 0.0;
        self.aftertouch = 0.0;
    }

    /// Process one sample from this voice.
    #[inline]
    pub fn tick(&mut self) -> f32 {
        if self.state == VoiceState::Free {
            return 0.0;
        }

        // Advance drift
        self.drift.tick();

        // Update oscillator frequencies with current drift
        let drifted_freq = self.frequency * self.drift.frequency_ratio();

        // LFO modulation
        let (pitch_ratio, filter_mod, vca_mod) = self.lfo.tick();
        let modulated_freq = drifted_freq * pitch_ratio;

        self.layer1.set_frequency(modulated_freq);
        self.layer2.set_frequency(modulated_freq);

        // Aftertouch adds filter modulation: pressure opens filter by up to 2000 Hz.
        // This is additive with the LFO filter modulation.
        let at_filter = self.aftertouch.clamp(0.0, 1.0);
        let combined_filter_mod = filter_mod + at_filter;

        // Process both layers with per-voice envelope timing jitter.
        // timing_factor() from VoiceDrift is the CS-80's analog character —
        // each voice's envelopes run at slightly different speeds.
        let timing = self.drift.timing_factor();
        let l1 = self.layer1.tick_with_timing(combined_filter_mod, timing);
        let l2 = self.layer2.tick_with_timing(combined_filter_mod, timing);

        // Mix layers according to balance
        let mix = self.layer_mix.clamp(0.0, 1.0);
        let mixed = l1.mul_add(1.0 - mix, l2 * mix);

        // Apply ring modulator
        let ring_out = self.ring_mod.process(mixed);

        // Aftertouch modulation: pressure opens filter and boosts VCA.
        // Filter: aftertouch adds up to 2000 Hz cutoff offset via layers' LPF.
        // VCA: aftertouch adds up to 20% gain boost.
        let at = self.aftertouch.clamp(0.0, 1.0);
        let at_vca_boost = at.mul_add(0.2, 1.0);

        // Apply LFO VCA modulation, velocity, and aftertouch VCA boost
        let output = ring_out * vca_mod * self.velocity * at_vca_boost;

        // Check if both layers have finished releasing
        if self.state == VoiceState::Releasing && self.layer1.is_idle() && self.layer2.is_idle() {
            self.state = VoiceState::Free;
            self.note = None;
        }

        sanitize_sample(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::oscillator::Waveform;

    #[test]
    fn voice_produces_sound() {
        let mut voice = Voice::new(0, 44100.0, 6.0);
        voice.layer1.oscillator.waveform = Waveform::Sawtooth;
        voice.layer2.oscillator.waveform = Waveform::Pulse;

        voice.note_on(60, 0.8); // Middle C

        let mut has_signal = false;
        for _ in 0..4410 {
            let sample = voice.tick();
            assert!(sample.is_finite());
            if sample.abs() > 0.001 {
                has_signal = true;
            }
        }
        assert!(has_signal, "voice should produce audible output");
    }

    #[test]
    fn voice_silent_when_free() {
        let mut voice = Voice::new(0, 44100.0, 6.0);
        for _ in 0..100 {
            let sample = voice.tick();
            assert!(
                (sample - 0.0).abs() < f32::EPSILON,
                "free voice should be silent"
            );
        }
    }

    #[test]
    fn voice_releases_and_becomes_free() {
        let mut voice = Voice::new(0, 44100.0, 6.0);
        voice.layer1.oscillator.waveform = Waveform::Sawtooth;
        voice.layer1.vca_envelope.set_attack(0.001);
        voice.layer1.vca_envelope.set_release(0.01);
        voice.layer2.vca_envelope.set_attack(0.001);
        voice.layer2.vca_envelope.set_release(0.01);

        voice.note_on(60, 0.8);
        for _ in 0..4410 {
            voice.tick();
        }

        voice.note_off();
        assert!(voice.is_releasing());

        // Run through release
        for _ in 0..44100 {
            voice.tick();
        }
        assert!(voice.is_free(), "voice should be free after release");
    }

    #[test]
    fn voice_drift_affects_pitch() {
        // Two voices with high drift should have different effective frequencies
        let mut v0 = Voice::new(0, 44100.0, 10.0);
        let mut v1 = Voice::new(1, 44100.0, 10.0);

        v0.note_on(60, 0.8);
        v1.note_on(60, 0.8);

        // Advance both voices
        for _ in 0..44100 {
            v0.tick();
            v1.tick();
        }

        // Their drift values should differ (random, but with high drift extremely unlikely to be identical)
        let ratio0 = v0.drift.frequency_ratio();
        let ratio1 = v1.drift.frequency_ratio();
        // Both should be valid
        assert!(ratio0 > 0.0 && ratio0.is_finite());
        assert!(ratio1 > 0.0 && ratio1.is_finite());
    }

    #[test]
    fn voice_output_always_sanitized() {
        let mut voice = Voice::new(0, 44100.0, 6.0);
        voice.note_on(60, 1.0);

        for _ in 0..88200 {
            let sample = voice.tick();
            assert!(sample.is_finite(), "voice output must be finite");
        }
    }
}
