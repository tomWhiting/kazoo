//! CS-80 synthesis engine.
//!
//! 8-voice polyphonic. Each voice = 2 complete synthesis layers mixed.
//! Per-voice analog drift (random detuning + envelope jitter).

pub mod drift;
pub mod envelope;
pub mod filter;
pub mod layer;
pub mod lfo;
pub mod oscillator;
pub mod ring_mod;
pub mod voice;

use kazoo_core::{sanitize_sample, soft_limit};

use self::lfo::LfoRouting;
use self::oscillator::{OctaveRange, Waveform};
use self::voice::{Voice, VoiceState};

/// Number of polyphonic voices.
pub const NUM_VOICES: usize = 8;

/// Snapshot of voice state for UI display.
#[derive(Debug, Clone, Copy)]
pub struct VoiceStatus {
    /// Voice index (0-7).
    pub index: u8,
    /// Whether the voice is active (sounding or releasing).
    pub active: bool,
    /// Whether the voice is in the release tail (note-off sent, fading out).
    pub releasing: bool,
    /// Current detuning in cents from drift.
    pub detune_cents: f32,
    /// MIDI note being played (if active).
    pub note: Option<u8>,
}

/// Shared parameters that apply to all voices.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SynthParams {
    // -- Layer I --
    pub layer1_waveform: Waveform,
    pub layer1_octave: OctaveRange,
    pub layer1_fine_tune: f32,
    pub layer1_pulse_width: f32,
    pub layer1_hpf_cutoff: f32,
    pub layer1_hpf_resonance: f32,
    pub layer1_lpf_cutoff: f32,
    pub layer1_lpf_resonance: f32,
    pub layer1_filter_env_il: f32,
    pub layer1_filter_env_al: f32,
    pub layer1_filter_env_attack: f32,
    pub layer1_filter_env_decay: f32,
    pub layer1_filter_env_release: f32,
    pub layer1_filter_env_depth: f32,
    pub layer1_vca_attack: f32,
    pub layer1_vca_decay: f32,
    pub layer1_vca_sustain: f32,
    pub layer1_vca_release: f32,
    pub layer1_level: f32,

    // -- Layer II --
    pub layer2_waveform: Waveform,
    pub layer2_octave: OctaveRange,
    pub layer2_fine_tune: f32,
    pub layer2_pulse_width: f32,
    pub layer2_hpf_cutoff: f32,
    pub layer2_hpf_resonance: f32,
    pub layer2_lpf_cutoff: f32,
    pub layer2_lpf_resonance: f32,
    pub layer2_filter_env_il: f32,
    pub layer2_filter_env_al: f32,
    pub layer2_filter_env_attack: f32,
    pub layer2_filter_env_decay: f32,
    pub layer2_filter_env_release: f32,
    pub layer2_filter_env_depth: f32,
    pub layer2_vca_attack: f32,
    pub layer2_vca_decay: f32,
    pub layer2_vca_sustain: f32,
    pub layer2_vca_release: f32,
    pub layer2_level: f32,

    // -- Shared --
    pub ring_mod_depth: f32,
    pub ring_mod_carrier_freq: f32,
    pub ring_mod_attack: f32,
    pub ring_mod_decay: f32,
    pub lfo_rate: f32,
    pub lfo_waveform: lfo::LfoWaveform,
    pub lfo_routing: LfoRouting,
    pub layer_mix: f32,
    pub drift_cents: f32,
    pub master_level: f32,
}

impl Default for SynthParams {
    fn default() -> Self {
        Self {
            // Layer I: bright, fast attack
            layer1_waveform: Waveform::Sawtooth,
            layer1_octave: OctaveRange::Eight,
            layer1_fine_tune: 2.0,
            layer1_pulse_width: 0.5,
            layer1_hpf_cutoff: 120.0,
            layer1_hpf_resonance: 0.3,
            layer1_lpf_cutoff: 2400.0,
            layer1_lpf_resonance: 0.5,
            layer1_filter_env_il: 0.4,
            layer1_filter_env_al: 0.8,
            layer1_filter_env_attack: 0.05,
            layer1_filter_env_decay: 0.3,
            layer1_filter_env_release: 0.5,
            layer1_filter_env_depth: 4000.0,
            layer1_vca_attack: 0.01,
            layer1_vca_decay: 0.2,
            layer1_vca_sustain: 0.7,
            layer1_vca_release: 0.4,
            layer1_level: 0.7,

            // Layer II: slow, evolving
            layer2_waveform: Waveform::Pulse,
            layer2_octave: OctaveRange::Sixteen,
            layer2_fine_tune: -3.0,
            layer2_pulse_width: 0.65,
            layer2_hpf_cutoff: 80.0,
            layer2_hpf_resonance: 0.2,
            layer2_lpf_cutoff: 800.0,
            layer2_lpf_resonance: 0.4,
            layer2_filter_env_il: 0.2,
            layer2_filter_env_al: 0.6,
            layer2_filter_env_attack: 0.2,
            layer2_filter_env_decay: 0.8,
            layer2_filter_env_release: 1.2,
            layer2_filter_env_depth: 3000.0,
            layer2_vca_attack: 0.08,
            layer2_vca_decay: 0.5,
            layer2_vca_sustain: 0.8,
            layer2_vca_release: 0.8,
            layer2_level: 0.6,

            // Shared
            ring_mod_depth: 0.4,
            ring_mod_carrier_freq: 200.0,
            ring_mod_attack: 0.005,
            ring_mod_decay: 0.2,
            lfo_rate: 2.5,
            lfo_waveform: lfo::LfoWaveform::Sine,
            lfo_routing: LfoRouting {
                pitch_cents: 5.0,
                filter_depth: 0.2,
                vca_depth: 0.0,
            },
            layer_mix: 0.5,
            drift_cents: 6.0,
            master_level: 0.7,
        }
    }
}

/// 8-voice polyphonic CS-80 synthesizer.
///
/// Manages voice allocation, parameter distribution, and summing.
/// All voices are pre-allocated at creation — no allocations during processing.
#[derive(Debug)]
pub struct Cs80Synth {
    /// The 8 voices — pre-allocated, never resized.
    voices: [Voice; NUM_VOICES],
    /// Shared parameters (applied to voices on parameter change).
    pub params: SynthParams,
    /// Sample rate.
    sample_rate: f32,
    /// Output buffer for spectrum analysis (pre-allocated).
    output_history: Vec<f32>,
    /// Write position in output history.
    history_pos: usize,
    /// Monotonic counter for voice age tracking (voice stealing).
    age_counter: u64,
}

impl Cs80Synth {
    /// History buffer size for spectrum display.
    const HISTORY_SIZE: usize = 2048;

    /// Create a new CS-80 synth with all 8 voices pre-allocated.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let params = SynthParams::default();
        let voices = std::array::from_fn(|i| {
            let mut voice = Voice::new(i as u8, sample_rate, params.drift_cents);
            Self::apply_params_to_voice(&params, &mut voice);
            voice
        });

        Self {
            voices,
            params,
            sample_rate: sample_rate.max(1.0),
            output_history: vec![0.0; Self::HISTORY_SIZE],
            history_pos: 0,
            age_counter: 0,
        }
    }

    /// Note-on: allocate a voice and start playing.
    pub fn note_on(&mut self, note: u8, velocity: f32) {
        // Voice stealing priority:
        // 1. Free voice
        // 2. Releasing voice (steal the oldest releasing)
        // 3. Oldest active voice (last resort)

        let voice_idx = self
            .find_free_voice()
            .or_else(|| self.find_releasing_voice())
            .unwrap_or_else(|| self.find_oldest_active_voice());

        self.age_counter += 1;
        let voice = &mut self.voices[voice_idx];
        Self::apply_params_to_voice(&self.params, voice);
        voice.age = self.age_counter;
        voice.note_on(note, velocity);
    }

    /// Note-off: release the voice playing this note.
    pub fn note_off(&mut self, note: u8) {
        for voice in &mut self.voices {
            if voice.note() == Some(note) && voice.state() == VoiceState::Active {
                voice.note_off();
                return;
            }
        }
    }

    /// Set polyphonic aftertouch for a specific note.
    pub fn aftertouch(&mut self, note: u8, pressure: f32) {
        for voice in &mut self.voices {
            if voice.note() == Some(note) {
                voice.aftertouch = pressure.clamp(0.0, 1.0);
            }
        }
    }

    /// Apply current parameters to all active voices.
    pub fn apply_params(&mut self) {
        let params = self.params.clone();
        for voice in &mut self.voices {
            Self::apply_params_to_voice(&params, voice);
        }
    }

    /// Get voice status for UI display.
    #[must_use]
    pub fn voice_status(&self) -> [VoiceStatus; NUM_VOICES] {
        std::array::from_fn(|i| VoiceStatus {
            index: i as u8,
            active: !self.voices[i].is_free(),
            releasing: self.voices[i].is_releasing(),
            detune_cents: self.voices[i].drift.detune_cents(),
            note: self.voices[i].note(),
        })
    }

    /// Get recent output samples for waveform/spectrum display.
    ///
    /// Returns a linearized view starting from the oldest sample. The caller
    /// receives two slices: `[history_pos..end]` then `[0..history_pos]`.
    /// We copy into a pre-allocated buffer to avoid returning disjoint slices.
    #[must_use]
    pub fn output_history_linearized(&self) -> Vec<f32> {
        let pos = self.history_pos;
        let mut out = Vec::with_capacity(self.output_history.len());
        out.extend_from_slice(&self.output_history[pos..]);
        out.extend_from_slice(&self.output_history[..pos]);
        out
    }

    /// Set sample rate for all voices.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        for voice in &mut self.voices {
            voice.set_sample_rate(sample_rate);
        }
    }

    /// Reset all voices.
    pub fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.reset();
        }
        self.output_history.fill(0.0);
        self.history_pos = 0;
    }

    /// Process one sample from all active voices, summed.
    #[inline]
    pub fn tick(&mut self) -> f32 {
        let mut sum = 0.0_f32;
        for voice in &mut self.voices {
            sum += voice.tick();
        }

        // Apply master level and soft limiter
        let output = soft_limit(sum * self.params.master_level);
        let output = sanitize_sample(output);

        // Store in history for spectrum display
        self.output_history[self.history_pos] = output;
        self.history_pos = (self.history_pos + 1) % Self::HISTORY_SIZE;

        output
    }

    /// Process a block of samples into the output buffer.
    pub fn process_block(&mut self, output: &mut [f32]) {
        for sample in output.iter_mut() {
            *sample = self.tick();
        }
    }

    /// Apply shared parameters to a single voice.
    fn apply_params_to_voice(params: &SynthParams, voice: &mut Voice) {
        // Layer I
        voice.layer1.oscillator.waveform = params.layer1_waveform;
        voice.layer1.oscillator.octave_range = params.layer1_octave;
        voice.layer1.oscillator.fine_tune_cents = params.layer1_fine_tune;
        voice.layer1.oscillator.pulse_width = params.layer1_pulse_width;
        voice.layer1.params.hpf_cutoff = params.layer1_hpf_cutoff;
        voice.layer1.params.hpf_resonance = params.layer1_hpf_resonance;
        voice.layer1.params.lpf_cutoff = params.layer1_lpf_cutoff;
        voice.layer1.params.lpf_resonance = params.layer1_lpf_resonance;
        voice.layer1.filter_envelope.initial_level = params.layer1_filter_env_il;
        voice.layer1.filter_envelope.attack_level = params.layer1_filter_env_al;
        voice
            .layer1
            .filter_envelope
            .set_attack(params.layer1_filter_env_attack);
        voice
            .layer1
            .filter_envelope
            .set_decay(params.layer1_filter_env_decay);
        voice
            .layer1
            .filter_envelope
            .set_release(params.layer1_filter_env_release);
        voice.layer1.params.filter_env_depth = params.layer1_filter_env_depth;
        voice
            .layer1
            .vca_envelope
            .set_attack(params.layer1_vca_attack);
        voice.layer1.vca_envelope.set_decay(params.layer1_vca_decay);
        voice
            .layer1
            .vca_envelope
            .set_sustain(params.layer1_vca_sustain);
        voice
            .layer1
            .vca_envelope
            .set_release(params.layer1_vca_release);
        voice.layer1.params.level = params.layer1_level;

        // Layer II
        voice.layer2.oscillator.waveform = params.layer2_waveform;
        voice.layer2.oscillator.octave_range = params.layer2_octave;
        voice.layer2.oscillator.fine_tune_cents = params.layer2_fine_tune;
        voice.layer2.oscillator.pulse_width = params.layer2_pulse_width;
        voice.layer2.params.hpf_cutoff = params.layer2_hpf_cutoff;
        voice.layer2.params.hpf_resonance = params.layer2_hpf_resonance;
        voice.layer2.params.lpf_cutoff = params.layer2_lpf_cutoff;
        voice.layer2.params.lpf_resonance = params.layer2_lpf_resonance;
        voice.layer2.filter_envelope.initial_level = params.layer2_filter_env_il;
        voice.layer2.filter_envelope.attack_level = params.layer2_filter_env_al;
        voice
            .layer2
            .filter_envelope
            .set_attack(params.layer2_filter_env_attack);
        voice
            .layer2
            .filter_envelope
            .set_decay(params.layer2_filter_env_decay);
        voice
            .layer2
            .filter_envelope
            .set_release(params.layer2_filter_env_release);
        voice.layer2.params.filter_env_depth = params.layer2_filter_env_depth;
        voice
            .layer2
            .vca_envelope
            .set_attack(params.layer2_vca_attack);
        voice.layer2.vca_envelope.set_decay(params.layer2_vca_decay);
        voice
            .layer2
            .vca_envelope
            .set_sustain(params.layer2_vca_sustain);
        voice
            .layer2
            .vca_envelope
            .set_release(params.layer2_vca_release);
        voice.layer2.params.level = params.layer2_level;

        // Shared
        voice.ring_mod.depth = params.ring_mod_depth;
        voice
            .ring_mod
            .set_carrier_freq(params.ring_mod_carrier_freq);
        voice.ring_mod.set_attack(params.ring_mod_attack);
        voice.ring_mod.set_decay(params.ring_mod_decay);
        voice.lfo.set_rate(params.lfo_rate);
        voice.lfo.waveform = params.lfo_waveform;
        voice.lfo.routing = params.lfo_routing;
        voice.layer_mix = params.layer_mix;
        voice.drift.max_cents = params.drift_cents;
    }

    /// Find a free voice. Returns index.
    fn find_free_voice(&self) -> Option<usize> {
        self.voices.iter().position(Voice::is_free)
    }

    /// Find a releasing voice (for stealing).
    fn find_releasing_voice(&self) -> Option<usize> {
        self.voices.iter().position(Voice::is_releasing)
    }

    /// Find the oldest active voice (last resort for stealing).
    /// Returns the voice with the lowest age (earliest note-on).
    fn find_oldest_active_voice(&self) -> usize {
        self.voices
            .iter()
            .enumerate()
            .min_by_key(|(_, v)| v.age)
            .map_or(0, |(i, _)| i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synth_creates_with_8_voices() {
        let synth = Cs80Synth::new(44100.0);
        assert_eq!(synth.voices.len(), NUM_VOICES);
        for voice in &synth.voices {
            assert!(voice.is_free());
        }
    }

    #[test]
    fn synth_note_on_allocates_voice() {
        let mut synth = Cs80Synth::new(44100.0);
        synth.note_on(60, 0.8);

        let statuses = synth.voice_status();
        let active_count = statuses.iter().filter(|s| s.active).count();
        assert_eq!(active_count, 1, "one voice should be active");
    }

    #[test]
    fn synth_polyphony() {
        let mut synth = Cs80Synth::new(44100.0);
        for note in 60..68 {
            synth.note_on(note, 0.8);
        }

        let statuses = synth.voice_status();
        let active_count = statuses.iter().filter(|s| s.active).count();
        assert_eq!(active_count, 8, "all 8 voices should be active");
    }

    #[test]
    fn synth_produces_output() {
        let mut synth = Cs80Synth::new(44100.0);
        synth.note_on(60, 0.8);

        let mut has_signal = false;
        for _ in 0..4410 {
            let sample = synth.tick();
            assert!(sample.is_finite());
            if sample.abs() > 0.001 {
                has_signal = true;
            }
        }
        assert!(has_signal, "synth should produce audible output");
    }

    #[test]
    fn synth_output_within_limits() {
        let mut synth = Cs80Synth::new(44100.0);
        // Play a full chord
        for note in [60, 64, 67, 72] {
            synth.note_on(note, 1.0);
        }

        for _ in 0..88200 {
            let sample = synth.tick();
            assert!(sample.is_finite());
            assert!(
                sample.abs() <= 1.01,
                "soft limiter should keep output <= 1.0, got {sample}"
            );
        }
    }

    #[test]
    fn synth_per_voice_drift_demonstrable() {
        let synth = Cs80Synth::new(44100.0);
        let statuses = synth.voice_status();

        // Each voice should have a different drift value (they're random)
        let drift_values: Vec<f32> = statuses.iter().map(|s| s.detune_cents).collect();
        let all_same = drift_values.windows(2).all(|w| (w[0] - w[1]).abs() < 0.001);
        assert!(
            !all_same,
            "per-voice drift should produce different detuning values: {drift_values:?}"
        );
    }

    #[test]
    fn synth_note_off_releases() {
        let mut synth = Cs80Synth::new(44100.0);
        synth.note_on(60, 0.8);

        // Process some samples
        for _ in 0..4410 {
            synth.tick();
        }

        synth.note_off(60);

        // Find the voice that was playing note 60
        let has_releasing = synth.voices.iter().any(|v| v.is_releasing());
        assert!(
            has_releasing,
            "should have a releasing voice after note_off"
        );
    }

    #[test]
    fn synth_voice_stealing() {
        let mut synth = Cs80Synth::new(44100.0);
        // Fill all 8 voices
        for note in 60..68 {
            synth.note_on(note, 0.8);
        }

        // 9th note should steal
        synth.note_on(70, 0.8);

        let statuses = synth.voice_status();
        let has_note_70 = statuses.iter().any(|s| s.note == Some(70));
        assert!(has_note_70, "voice stealing should allocate note 70");
    }

    #[test]
    fn synth_process_block() {
        let mut synth = Cs80Synth::new(44100.0);
        synth.note_on(60, 0.8);

        let mut buffer = vec![0.0_f32; 256];
        synth.process_block(&mut buffer);

        let has_signal = buffer.iter().any(|&s| s.abs() > 0.001);
        assert!(has_signal, "process_block should produce output");

        for &sample in &buffer {
            assert!(sample.is_finite());
        }
    }
}
