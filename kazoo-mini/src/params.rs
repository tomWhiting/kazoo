//! Parameter snapshots sent from the UI to the audio callback.

use crate::synth::MiniVoice;
use crate::synth::oscillator::{OctaveRange, Waveform};
use crate::synth::xmod::ModWheelDest;

/// Performance-section switches and glide rate.
#[derive(Debug, Clone, Copy)]
pub struct PerformanceParams {
    pub glide_rate: f32,
    pub glide_enabled: bool,
    pub legato: bool,
    pub retrigger: bool,
}

/// Flat parameter snapshot sent from the UI to the audio thread.
///
/// Contains every user-editable parameter value. Sent on every param change.
/// No heap allocations: creating, sending or dropping one never touches
/// the heap, so the audio callback may receive and discard it.
#[derive(Debug, Clone)]
pub struct MiniParams {
    // Oscillators (3 × 5 = 15 fields)
    osc1_waveform: Waveform,
    osc1_octave: OctaveRange,
    osc1_fine_tune: f32,
    osc1_level: f32,

    osc2_waveform: Waveform,
    osc2_octave: OctaveRange,
    osc2_fine_tune: f32,
    osc2_level: f32,

    osc3_waveform: Waveform,
    osc3_octave: OctaveRange,
    osc3_fine_tune: f32,
    osc3_level: f32,
    osc3_lfo_mode: bool,

    // Mixer (5 fields)
    mixer_osc1: f32,
    mixer_osc2: f32,
    mixer_osc3: f32,
    mixer_noise: f32,
    mixer_ext: f32,

    // Filter (5 fields)
    filter_cutoff: f32,
    filter_resonance: f32,
    filter_key_track: f32,
    filter_env_amount: f32,
    filter_drive: f32,

    // Envelopes (8 fields)
    filter_env_attack: f32,
    filter_env_decay: f32,
    filter_env_sustain: f32,
    filter_env_release: f32,

    amp_env_attack: f32,
    amp_env_decay: f32,
    amp_env_sustain: f32,
    amp_env_release: f32,

    // Performance
    perf: PerformanceParams,

    // Cross-mod (4 fields)
    xmod_osc3_to_osc2_fm: f32,
    xmod_osc2_to_filter: f32,
    xmod_mod_wheel: f32,
    xmod_mod_wheel_dest: ModWheelDest,
}

impl MiniParams {
    /// Capture current parameter state from the UI-side voice.
    pub const fn from_voice(voice: &MiniVoice) -> Self {
        Self {
            osc1_waveform: voice.osc1.waveform,
            osc1_octave: voice.osc1.octave,
            osc1_fine_tune: voice.osc1.fine_tune_cents,
            osc1_level: voice.osc1.level,

            osc2_waveform: voice.osc2.waveform,
            osc2_octave: voice.osc2.octave,
            osc2_fine_tune: voice.osc2.fine_tune_cents,
            osc2_level: voice.osc2.level,

            osc3_waveform: voice.osc3.waveform,
            osc3_octave: voice.osc3.octave,
            osc3_fine_tune: voice.osc3.fine_tune_cents,
            osc3_level: voice.osc3.level,
            osc3_lfo_mode: voice.osc3.lfo_mode,

            mixer_osc1: voice.mixer.osc1_level,
            mixer_osc2: voice.mixer.osc2_level,
            mixer_osc3: voice.mixer.osc3_level,
            mixer_noise: voice.mixer.noise_level,
            mixer_ext: voice.mixer.ext_level,

            filter_cutoff: voice.filter.base_cutoff,
            filter_resonance: voice.filter.resonance(),
            filter_key_track: voice.filter.key_track,
            filter_env_amount: voice.filter_env_amount,
            filter_drive: voice.filter.drive(),

            filter_env_attack: voice.filter_env.attack,
            filter_env_decay: voice.filter_env.decay,
            filter_env_sustain: voice.filter_env.sustain,
            filter_env_release: voice.filter_env.release,

            amp_env_attack: voice.amp_env.attack,
            amp_env_decay: voice.amp_env.decay,
            amp_env_sustain: voice.amp_env.sustain,
            amp_env_release: voice.amp_env.release,

            perf: PerformanceParams {
                glide_rate: voice.glide.rate,
                glide_enabled: voice.glide.enabled,
                legato: voice.legato,
                retrigger: voice.retrigger,
            },

            xmod_osc3_to_osc2_fm: voice.xmod.osc3_to_osc2_fm,
            xmod_osc2_to_filter: voice.xmod.osc2_to_filter,
            xmod_mod_wheel: voice.xmod.mod_wheel,
            xmod_mod_wheel_dest: voice.xmod.mod_wheel_dest,
        }
    }

    /// Apply this parameter snapshot to an audio-thread voice.
    pub fn apply_to(&self, v: &mut MiniVoice) {
        // Oscillators
        v.osc1.waveform = self.osc1_waveform;
        v.osc1.octave = self.osc1_octave;
        v.osc1.fine_tune_cents = self.osc1_fine_tune;
        v.osc1.level = self.osc1_level;

        v.osc2.waveform = self.osc2_waveform;
        v.osc2.octave = self.osc2_octave;
        v.osc2.fine_tune_cents = self.osc2_fine_tune;
        v.osc2.level = self.osc2_level;

        v.osc3.waveform = self.osc3_waveform;
        v.osc3.octave = self.osc3_octave;
        v.osc3.fine_tune_cents = self.osc3_fine_tune;
        v.osc3.level = self.osc3_level;
        v.osc3.lfo_mode = self.osc3_lfo_mode;

        // Mixer
        v.mixer.osc1_level = self.mixer_osc1;
        v.mixer.osc2_level = self.mixer_osc2;
        v.mixer.osc3_level = self.mixer_osc3;
        v.mixer.noise_level = self.mixer_noise;
        v.mixer.ext_level = self.mixer_ext;

        // Filter
        v.filter.base_cutoff = self.filter_cutoff;
        v.filter.set_cutoff(self.filter_cutoff);
        v.filter.set_resonance(self.filter_resonance);
        v.filter.key_track = self.filter_key_track;
        v.filter.set_drive(self.filter_drive);
        v.filter_env_amount = self.filter_env_amount;

        // Filter envelope
        v.filter_env.attack = self.filter_env_attack;
        v.filter_env.decay = self.filter_env_decay;
        v.filter_env.sustain = self.filter_env_sustain;
        v.filter_env.release = self.filter_env_release;
        v.filter_env.recompute_coefficients();

        // Amp envelope
        v.amp_env.attack = self.amp_env_attack;
        v.amp_env.decay = self.amp_env_decay;
        v.amp_env.sustain = self.amp_env_sustain;
        v.amp_env.release = self.amp_env_release;
        v.amp_env.recompute_coefficients();

        // Performance
        v.glide.rate = self.perf.glide_rate;
        v.glide.enabled = self.perf.glide_enabled;
        v.legato = self.perf.legato;
        v.retrigger = self.perf.retrigger;

        // Cross-mod
        v.xmod.osc3_to_osc2_fm = self.xmod_osc3_to_osc2_fm;
        v.xmod.osc2_to_filter = self.xmod_osc2_to_filter;
        v.xmod.mod_wheel = self.xmod_mod_wheel;
        v.xmod.mod_wheel_dest = self.xmod_mod_wheel_dest;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that `MiniParams::from_voice` captures all editable parameters
    /// and `apply_to` correctly restores them on a fresh voice.
    #[test]
    fn mini_params_round_trip() {
        let mut voice = MiniVoice::new(44100.0);

        // Modify every parameter to non-default values.
        voice.osc1.waveform = Waveform::Square;
        voice.osc1.octave = OctaveRange::Footage16;
        voice.osc1.fine_tune_cents = 7.5;
        voice.osc1.level = 0.42;

        voice.osc2.waveform = Waveform::NarrowPulse;
        voice.osc2.octave = OctaveRange::Footage4;
        voice.osc2.fine_tune_cents = -12.0;
        voice.osc2.level = 0.33;

        voice.osc3.waveform = Waveform::WidePulse;
        voice.osc3.octave = OctaveRange::Footage32;
        voice.osc3.fine_tune_cents = 25.0;
        voice.osc3.level = 0.91;
        voice.osc3.lfo_mode = true;

        voice.mixer.osc1_level = 0.11;
        voice.mixer.osc2_level = 0.22;
        voice.mixer.osc3_level = 0.33;
        voice.mixer.noise_level = 0.44;
        voice.mixer.ext_level = 0.55;

        voice.filter.base_cutoff = 1234.0;
        voice.filter.set_cutoff(1234.0);
        voice.filter.set_resonance(0.77);
        voice.filter.set_drive(2.5);
        voice.filter.key_track = 0.65;
        voice.filter_env_amount = 0.88;

        voice.filter_env.attack = 0.05;
        voice.filter_env.decay = 0.3;
        voice.filter_env.sustain = 0.45;
        voice.filter_env.release = 0.8;

        voice.amp_env.attack = 0.002;
        voice.amp_env.decay = 0.15;
        voice.amp_env.sustain = 0.7;
        voice.amp_env.release = 1.2;

        voice.glide.rate = 42.0;
        voice.glide.enabled = true;
        voice.legato = false;
        voice.retrigger = true;

        voice.xmod.osc3_to_osc2_fm = 0.6;
        voice.xmod.osc2_to_filter = 0.35;
        voice.xmod.mod_wheel = 0.8;
        voice.xmod.mod_wheel_dest = ModWheelDest::Pitch;

        // Capture params.
        let params = MiniParams::from_voice(&voice);

        // Apply to a fresh voice.
        let mut target = MiniVoice::new(44100.0);
        params.apply_to(&mut target);

        // Verify all fields.
        assert_eq!(target.osc1.waveform, Waveform::Square);
        assert_eq!(target.osc1.octave, OctaveRange::Footage16);
        assert!((target.osc1.fine_tune_cents - 7.5).abs() < f32::EPSILON);
        assert!((target.osc1.level - 0.42).abs() < f32::EPSILON);

        assert_eq!(target.osc2.waveform, Waveform::NarrowPulse);
        assert_eq!(target.osc2.octave, OctaveRange::Footage4);
        assert!((target.osc2.fine_tune_cents - (-12.0)).abs() < f32::EPSILON);
        assert!((target.osc2.level - 0.33).abs() < f32::EPSILON);

        assert_eq!(target.osc3.waveform, Waveform::WidePulse);
        assert_eq!(target.osc3.octave, OctaveRange::Footage32);
        assert!((target.osc3.fine_tune_cents - 25.0).abs() < f32::EPSILON);
        assert!((target.osc3.level - 0.91).abs() < f32::EPSILON);
        assert!(target.osc3.lfo_mode);

        assert!((target.mixer.osc1_level - 0.11).abs() < f32::EPSILON);
        assert!((target.mixer.osc2_level - 0.22).abs() < f32::EPSILON);
        assert!((target.mixer.osc3_level - 0.33).abs() < f32::EPSILON);
        assert!((target.mixer.noise_level - 0.44).abs() < f32::EPSILON);
        assert!((target.mixer.ext_level - 0.55).abs() < f32::EPSILON);

        assert!((target.filter.base_cutoff - 1234.0).abs() < f32::EPSILON);
        assert!((target.filter.resonance() - 0.77).abs() < 0.01);
        assert!((target.filter.drive() - 2.5).abs() < f32::EPSILON);
        assert!((target.filter.key_track - 0.65).abs() < f32::EPSILON);
        assert!((target.filter_env_amount - 0.88).abs() < f32::EPSILON);

        assert!((target.filter_env.attack - 0.05).abs() < f32::EPSILON);
        assert!((target.filter_env.decay - 0.3).abs() < f32::EPSILON);
        assert!((target.filter_env.sustain - 0.45).abs() < f32::EPSILON);
        assert!((target.filter_env.release - 0.8).abs() < f32::EPSILON);

        assert!((target.amp_env.attack - 0.002).abs() < f32::EPSILON);
        assert!((target.amp_env.decay - 0.15).abs() < f32::EPSILON);
        assert!((target.amp_env.sustain - 0.7).abs() < f32::EPSILON);
        assert!((target.amp_env.release - 1.2).abs() < f32::EPSILON);

        assert!((target.glide.rate - 42.0).abs() < f32::EPSILON);
        assert!(target.glide.enabled);
        assert!(!target.legato);
        assert!(target.retrigger);

        assert!((target.xmod.osc3_to_osc2_fm - 0.6).abs() < f32::EPSILON);
        assert!((target.xmod.osc2_to_filter - 0.35).abs() < f32::EPSILON);
        assert!((target.xmod.mod_wheel - 0.8).abs() < f32::EPSILON);
        assert_eq!(target.xmod.mod_wheel_dest, ModWheelDest::Pitch);
    }

    /// Verify that applying params doesn't disrupt an active note.
    #[test]
    fn apply_params_preserves_audio_state() {
        let mut voice = MiniVoice::new(44100.0);
        voice.note_on(60);

        // Process some audio.
        let mut buf = vec![0.0; 441]; // 10ms
        voice.process_block(&mut buf);
        let pre_max = buf.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
        assert!(
            pre_max > 0.01,
            "voice should produce output before param change"
        );

        // Change a parameter via the MiniParams pipeline.
        let mut params = MiniParams::from_voice(&voice);
        params.filter_cutoff = 800.0;
        params.osc2_level = 0.5;
        params.apply_to(&mut voice);

        // Process more audio — should still be producing sound.
        let mut buf = vec![0.0; 441];
        voice.process_block(&mut buf);
        let post_max = buf.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
        assert!(
            post_max > 0.01,
            "voice should still produce output after param change, got {post_max}"
        );
    }

    /// Verify `MiniParams::from_voice` captures defaults correctly.
    #[test]
    fn default_params_capture() {
        let voice = MiniVoice::new(48000.0);
        let params = MiniParams::from_voice(&voice);

        // Check known defaults from MiniVoice::new.
        assert_eq!(params.osc1_waveform, Waveform::Saw);
        assert_eq!(params.osc1_octave, OctaveRange::Footage8);
        assert!((params.osc1_level - 0.8).abs() < f32::EPSILON);
        assert!((params.osc2_fine_tune - 2.0).abs() < f32::EPSILON); // slight detune
        assert_eq!(params.osc3_waveform, Waveform::Triangle);
        assert!(!params.osc3_lfo_mode);
        assert!(params.perf.legato); // default legato on
        assert!(!params.perf.retrigger); // default retrigger off
    }
}
