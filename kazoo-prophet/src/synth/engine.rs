//! Polyphonic Prophet engine and voice allocation.

use kazoo_core::{sanitize_sample, soft_limit};

use super::NUM_VOICES;
use super::params::SynthParams;
use super::status::VoiceStatus;
use super::voice::{ProphetVoice, VoiceState};

/// Five-voice Prophet-style synthesizer.
#[derive(Debug)]
pub struct ProphetSynth {
    voices: [ProphetVoice; NUM_VOICES],
    pub params: SynthParams,
    age_counter: u64,
    sample_rate: f32,
    output_history: Vec<f32>,
    history_pos: usize,
}

impl ProphetSynth {
    const HISTORY_SIZE: usize = 2048;

    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let params = SynthParams::default();
        let mut voices = std::array::from_fn(|i| ProphetVoice::new(i as u8, sample_rate.max(1.0)));
        for voice in &mut voices {
            voice.apply_params(&params);
        }
        Self {
            voices,
            params,
            age_counter: 0,
            sample_rate: sample_rate.max(1.0),
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
        voice.age = self.age_counter;
        voice.apply_params(&self.params);
        voice.note_on(note, velocity, self.params.drift.voice_detune_cents);
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

    pub fn apply_params(&mut self) {
        for voice in &mut self.voices {
            voice.apply_params(&self.params);
        }
    }

    pub fn process_block(&mut self, output: &mut [f32]) {
        for sample in output {
            *sample = self.process_sample();
        }
    }

    #[inline]
    pub fn process_sample(&mut self) -> f32 {
        let mut sum = 0.0;
        for voice in &mut self.voices {
            if voice.state() != VoiceState::Free {
                sum += voice.process(&self.params);
            }
        }
        let sample = soft_limit(sanitize_sample(sum));
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
        self.output_history.fill(0.0);
        self.history_pos = 0;
        self.age_counter = 0;
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        for voice in &mut self.voices {
            voice.set_sample_rate(self.sample_rate);
        }
    }

    fn find_voice(&self, state: VoiceState) -> Option<usize> {
        self.voices.iter().position(|voice| voice.state() == state)
    }

    fn oldest_voice(&self) -> usize {
        self.voices
            .iter()
            .enumerate()
            .min_by_key(|(_, voice)| voice.age)
            .map_or(0, |(idx, _)| idx)
    }

    #[inline]
    fn store_history(&mut self, sample: f32) {
        self.output_history[self.history_pos] = sample;
        self.history_pos = (self.history_pos + 1) % self.output_history.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_five_voices() {
        let synth = ProphetSynth::new(44_100.0);
        assert_eq!(synth.voice_status().len(), NUM_VOICES);
    }

    #[test]
    fn note_on_allocates_voice() {
        let mut synth = ProphetSynth::new(44_100.0);
        synth.note_on(60, 0.8);
        let active = synth
            .voice_status()
            .iter()
            .filter(|voice| voice.active)
            .count();
        assert_eq!(active, 1);
    }

    #[test]
    fn steals_oldest_voice_when_full() {
        let mut synth = ProphetSynth::new(44_100.0);
        for note in 60..66 {
            synth.note_on(note, 0.8);
        }
        let notes: Vec<_> = synth
            .voice_status()
            .iter()
            .filter_map(|voice| voice.note)
            .collect();
        assert_eq!(notes.len(), NUM_VOICES);
        assert!(!notes.contains(&60));
        assert!(notes.contains(&65));
    }

    #[test]
    fn synth_produces_audio() {
        let mut synth = ProphetSynth::new(44_100.0);
        synth.note_on(48, 0.9);
        synth.note_on(55, 0.8);
        synth.note_on(60, 0.7);
        let mut output = vec![0.0; 4096];
        synth.process_block(&mut output);
        let peak = output
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0_f32, f32::max);
        assert!(peak > 0.01, "expected audible output, got {peak}");
    }

    #[test]
    fn poly_mod_changes_the_waveform() {
        let mut dry = ProphetSynth::new(44_100.0);
        dry.note_on(60, 0.9);
        let mut dry_output = vec![0.0; 4096];
        dry.process_block(&mut dry_output);

        let mut modded = ProphetSynth::new(44_100.0);
        modded.params.poly_mod.osc_b_to_osc_a_cents = 900.0;
        modded.params.poly_mod.osc_b_to_filter_hz = 1400.0;
        modded.apply_params();
        modded.note_on(60, 0.9);
        let mut modded_output = vec![0.0; 4096];
        modded.process_block(&mut modded_output);

        let difference = dry_output
            .iter()
            .zip(modded_output.iter())
            .map(|(a, b)| (a - b).abs())
            .sum::<f32>();
        assert!(
            difference > 1.0,
            "poly-mod should materially change output, got difference {difference}"
        );
    }

    #[test]
    fn output_remains_finite() {
        let mut synth = ProphetSynth::new(96_000.0);
        synth.params.filter.resonance = 0.9;
        synth.params.poly_mod.osc_b_to_filter_hz = 3000.0;
        synth.apply_params();
        synth.note_on(84, 1.0);
        for _ in 0..96_000 {
            assert!(synth.process_sample().is_finite());
        }
    }

    #[test]
    fn voice_releases_to_inactive() {
        let mut synth = ProphetSynth::new(44_100.0);
        synth.note_on(60, 0.8);
        synth.process_block(&mut vec![0.0; 2048]);
        synth.note_off(60);
        synth.process_block(&mut vec![0.0; 44_100]);
        assert!(synth.voice_status().iter().all(|voice| !voice.active));
    }
}
