//! Polyphonic Juno-style engine and voice allocation.

use kazoo_core::{sanitize_sample, soft_limit};

use super::NUM_VOICES;
use super::chorus::JunoChorus;
use super::params::SynthParams;
use super::voice::{JunoVoice, VoiceState, VoiceStatus};

#[derive(Debug)]
pub struct JunoSynth {
    voices: [JunoVoice; NUM_VOICES],
    chorus: JunoChorus,
    pub params: SynthParams,
    age_counter: u64,
    output_history: Vec<f32>,
    history_pos: usize,
}

impl JunoSynth {
    const HISTORY_SIZE: usize = 2048;

    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(1.0);
        Self {
            voices: std::array::from_fn(|i| JunoVoice::new(i as u8, sr)),
            chorus: JunoChorus::new(sr),
            params: SynthParams::default(),
            age_counter: 0,
            output_history: vec![0.0; Self::HISTORY_SIZE],
            history_pos: 0,
        }
    }

    pub fn note_on(&mut self, note: u8, velocity: f32) {
        let idx = self
            .find_voice(VoiceState::Free)
            .or_else(|| self.find_voice(VoiceState::Releasing))
            .unwrap_or_else(|| self.oldest_voice());
        self.age_counter = self.age_counter.saturating_add(1);
        let voice = &mut self.voices[idx];
        voice.set_age(self.age_counter);
        voice.note_on(note, velocity, self.params.voice_drift_cents);
    }

    pub fn note_off(&mut self, note: u8) {
        for voice in &mut self.voices {
            if voice.note() == Some(note) && voice.state() == VoiceState::Active {
                voice.note_off();
                return;
            }
        }
    }

    pub fn all_notes_off(&mut self) {
        for voice in &mut self.voices {
            voice.note_off();
        }
    }

    pub const fn apply_params(&mut self) {}

    pub fn process_block(&mut self, output: &mut [f32]) {
        for sample in output {
            *sample = self.process_sample();
        }
    }

    pub fn process_sample(&mut self) -> f32 {
        let mut sum = 0.0;
        for voice in &mut self.voices {
            sum += voice.process(&self.params);
        }
        let chorused = self.chorus.process(sum, &self.params.chorus);
        let sample = soft_limit(sanitize_sample(chorused * self.params.master_level));
        self.store_history(sample);
        sample
    }

    #[must_use]
    pub fn voice_status(&self) -> [VoiceStatus; NUM_VOICES] {
        std::array::from_fn(|i| {
            let voice = &self.voices[i];
            VoiceStatus {
                index: i as u8,
                active: voice.state() != VoiceState::Free,
                releasing: voice.state() == VoiceState::Releasing,
                note: voice.note(),
                drift_cents: voice.drift_cents(),
            }
        })
    }

    #[must_use]
    pub fn output_history(&self) -> &[f32] {
        &self.output_history
    }

    #[must_use]
    pub const fn history_pos(&self) -> usize {
        self.history_pos
    }

    pub fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.reset();
        }
        self.chorus.reset();
        self.output_history.fill(0.0);
        self.history_pos = 0;
        self.age_counter = 0;
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        let sr = sample_rate.max(1.0);
        for voice in &mut self.voices {
            voice.set_sample_rate(sr);
        }
        self.chorus.set_sample_rate(sr);
    }

    fn find_voice(&self, state: VoiceState) -> Option<usize> {
        self.voices.iter().position(|voice| voice.state() == state)
    }

    fn oldest_voice(&self) -> usize {
        self.voices
            .iter()
            .enumerate()
            .min_by_key(|(_, voice)| voice.age())
            .map_or(0, |(idx, _)| idx)
    }

    fn store_history(&mut self, sample: f32) {
        self.output_history[self.history_pos] = sample;
        self.history_pos = (self.history_pos + 1) % self.output_history.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_six_voices() {
        let synth = JunoSynth::new(44_100.0);
        assert_eq!(synth.voice_status().len(), NUM_VOICES);
    }

    #[test]
    fn produces_procedural_audio() {
        let mut synth = JunoSynth::new(44_100.0);
        synth.note_on(48, 0.9);
        synth.note_on(55, 0.8);
        synth.note_on(60, 0.75);
        let mut output = vec![0.0; 4096];
        synth.process_block(&mut output);
        let peak = output.iter().map(|sample| sample.abs()).fold(0.0, f32::max);
        assert!(peak > 0.01, "expected generated audio, got {peak}");
    }

    #[test]
    fn steals_oldest_voice_when_full() {
        let mut synth = JunoSynth::new(44_100.0);
        for note in 60..68 {
            synth.note_on(note, 0.8);
        }
        let notes: Vec<_> = synth
            .voice_status()
            .iter()
            .filter_map(|voice| voice.note)
            .collect();
        assert_eq!(notes.len(), NUM_VOICES);
        assert!(!notes.contains(&60));
        assert!(notes.contains(&67));
    }

    #[test]
    fn output_remains_finite_with_hot_settings() {
        let mut synth = JunoSynth::new(96_000.0);
        synth.params.filter.resonance = 0.95;
        synth.params.filter.envelope_amount = 1.0;
        synth.params.chorus.noise = 0.08;
        synth.note_on(84, 1.0);
        for _ in 0..96_000 {
            assert!(synth.process_sample().is_finite());
        }
    }

    #[test]
    fn chorus_does_not_panic_at_common_device_rates() {
        for sample_rate in [44_100.0, 48_000.0, 96_000.0] {
            let mut synth = JunoSynth::new(sample_rate);
            synth.note_on(60, 0.9);
            for _ in 0..(sample_rate as usize * 3) {
                assert!(synth.process_sample().is_finite());
            }
        }
    }
}
