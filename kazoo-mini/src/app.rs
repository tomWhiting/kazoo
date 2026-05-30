//! Application state for the Minimoog bass/lead synth.
//!
//! Owns the synth voice and manages UI navigation state.

use crate::synth::MiniVoice;

// ---------------------------------------------------------------------------
// UI section navigation
// ---------------------------------------------------------------------------

/// Which section of the TUI is currently focused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Oscillators,
    Mixer,
    Filter,
    Envelopes,
    Performance,
}

impl Section {
    pub const ALL: [Self; 5] = [
        Self::Oscillators,
        Self::Mixer,
        Self::Filter,
        Self::Envelopes,
        Self::Performance,
    ];

    #[must_use]
    pub fn next(self) -> Self {
        let idx = Self::ALL.iter().position(|&s| s == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }

    #[must_use]
    pub fn prev(self) -> Self {
        let idx = Self::ALL.iter().position(|&s| s == self).unwrap_or(0);
        Self::ALL[(idx + Self::ALL.len() - 1) % Self::ALL.len()]
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Oscillators => "OSCILLATORS",
            Self::Mixer => "MIXER",
            Self::Filter => "FILTER",
            Self::Envelopes => "ENVELOPES",
            Self::Performance => "PERFORMANCE",
        }
    }

    /// Number of parameters in this section.
    #[must_use]
    pub const fn param_count(self) -> usize {
        match self {
            Self::Oscillators => 13, // 4 params × 3 oscs + LFO toggle for osc3
            Self::Mixer => 5,        // osc1, osc2, osc3, noise, ext levels
            Self::Filter => 7,       // cutoff, resonance, contour, key track, drive, O3>O2, O2>Flt
            Self::Envelopes => 8,    // filter ADSR + amp ADSR
            Self::Performance => 6, // glide rate, glide on/off, legato, retrigger, mod wheel, mod wheel dest
        }
    }
}

// ---------------------------------------------------------------------------
// Application state
// ---------------------------------------------------------------------------

/// Full application state for the Minimoog TUI.
#[derive(Debug)]
pub struct App {
    /// Whether the app should quit.
    pub should_quit: bool,
    /// The synth voice engine.
    pub voice: MiniVoice,
    /// Currently focused UI section.
    pub section: Section,
    /// Selected parameter index within the current section.
    pub param_index: usize,
    /// Audio sample rate (for display).
    pub sample_rate: u32,
}

impl App {
    /// Create a new app with the given sample rate.
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        Self {
            should_quit: false,
            voice: MiniVoice::new(sample_rate as f32),
            section: Section::Oscillators,
            param_index: 0,
            sample_rate,
        }
    }

    /// Move to the next section (Tab).
    pub fn next_section(&mut self) {
        self.section = self.section.next();
        self.param_index = 0;
    }

    /// Move to the previous section (Shift+Tab).
    pub fn prev_section(&mut self) {
        self.section = self.section.prev();
        self.param_index = 0;
    }

    /// Move to the next parameter within the section (j or Down).
    pub const fn next_param(&mut self) {
        let max = self.section.param_count();
        if max > 0 {
            self.param_index = (self.param_index + 1) % max;
        }
    }

    /// Move to the previous parameter within the section (k or Up).
    pub const fn prev_param(&mut self) {
        let max = self.section.param_count();
        if max > 0 {
            self.param_index = (self.param_index + max - 1) % max;
        }
    }

    /// Adjust the currently selected parameter by a delta.
    ///
    /// Positive = increase, negative = decrease.
    pub fn adjust_param(&mut self, delta: f32) {
        match self.section {
            Section::Oscillators => self.adjust_osc_param(delta),
            Section::Mixer => self.adjust_mixer_param(delta),
            Section::Filter => self.adjust_filter_param(delta),
            Section::Envelopes => self.adjust_envelope_param(delta),
            Section::Performance => self.adjust_performance_param(delta),
        }
    }

    fn adjust_osc_param(&mut self, delta: f32) {
        // 13 params: osc1 (0-3), osc2 (4-7), osc3 (8-12)
        // osc1/osc2: 4 each (wave, octave, tune, level)
        // osc3: 5 (wave, octave, tune, level, LFO toggle)
        let (osc_idx, param_within) = if self.param_index < 4 {
            (0, self.param_index)
        } else if self.param_index < 8 {
            (1, self.param_index - 4)
        } else {
            (2, self.param_index - 8)
        };

        // LFO toggle is osc3's 5th param (index 12, param_within=4)
        if osc_idx == 2 && param_within == 4 {
            if delta.abs() > 0.0 {
                self.voice.osc3.lfo_mode = !self.voice.osc3.lfo_mode;
            }
            return;
        }

        let osc = match osc_idx {
            0 => &mut self.voice.osc1,
            1 => &mut self.voice.osc2,
            _ => &mut self.voice.osc3,
        };

        match param_within {
            0 => {
                // Waveform: cycle through
                if delta > 0.0 {
                    osc.waveform = osc.waveform.next();
                } else {
                    osc.waveform = osc.waveform.prev();
                }
            }
            1 => {
                // Octave range: cycle through
                let allow_lo = osc_idx == 2; // Only Osc 3 gets Lo
                if delta > 0.0 {
                    osc.octave = osc.octave.next(allow_lo);
                } else {
                    osc.octave = osc.octave.prev(allow_lo);
                }
            }
            2 => {
                // Fine tune (cents): -50 to +50
                osc.fine_tune_cents = (osc.fine_tune_cents + delta).clamp(-50.0, 50.0);
            }
            _ => {
                // Level: 0 to 1
                osc.level = delta.mul_add(0.01, osc.level).clamp(0.0, 1.0);
            }
        }
    }

    fn adjust_mixer_param(&mut self, delta: f32) {
        let d = delta * 0.01;
        match self.param_index {
            0 => {
                self.voice.mixer.osc1_level = (self.voice.mixer.osc1_level + d).clamp(0.0, 1.0);
            }
            1 => {
                self.voice.mixer.osc2_level = (self.voice.mixer.osc2_level + d).clamp(0.0, 1.0);
            }
            2 => {
                self.voice.mixer.osc3_level = (self.voice.mixer.osc3_level + d).clamp(0.0, 1.0);
            }
            3 => {
                self.voice.mixer.noise_level = (self.voice.mixer.noise_level + d).clamp(0.0, 1.0);
            }
            _ => {
                self.voice.mixer.ext_level = (self.voice.mixer.ext_level + d).clamp(0.0, 1.0);
            }
        }
    }

    fn adjust_filter_param(&mut self, delta: f32) {
        match self.param_index {
            0 => {
                // Cutoff: exponential adjustment (musical)
                let current = self.voice.filter.base_cutoff;
                let multiplier = (delta * 0.02).exp2();
                let new_cutoff =
                    (current * multiplier).clamp(MoogLadder::MIN_CUTOFF, MoogLadder::MAX_CUTOFF);
                self.voice.filter.base_cutoff = new_cutoff;
                self.voice.filter.set_cutoff(new_cutoff);
            }
            1 => {
                // Resonance: 0 to 1
                let current = self.voice.filter.resonance();
                self.voice
                    .filter
                    .set_resonance(delta.mul_add(0.01, current).clamp(0.0, 1.0));
            }
            2 => {
                // Filter env amount: 0 to 1
                self.voice.filter_env_amount = delta
                    .mul_add(0.01, self.voice.filter_env_amount)
                    .clamp(0.0, 1.0);
            }
            3 => {
                // Key tracking: 0 to 1
                self.voice.filter.key_track = delta
                    .mul_add(0.01, self.voice.filter.key_track)
                    .clamp(0.0, 1.0);
            }
            4 => {
                // Drive: 0.1 to 4.0
                let current = self.voice.filter.drive();
                let new_drive = delta.mul_add(0.05, current).clamp(0.1, 4.0);
                self.voice.filter.set_drive(new_drive);
            }
            5 => {
                // Cross-mod: Osc3 -> Osc2 FM depth (0 to 1)
                self.voice.xmod.osc3_to_osc2_fm = delta
                    .mul_add(0.01, self.voice.xmod.osc3_to_osc2_fm)
                    .clamp(0.0, 1.0);
            }
            _ => {
                // Cross-mod: Osc2 -> Filter cutoff depth (0 to 1)
                self.voice.xmod.osc2_to_filter = delta
                    .mul_add(0.01, self.voice.xmod.osc2_to_filter)
                    .clamp(0.0, 1.0);
            }
        }
    }

    fn adjust_envelope_param(&mut self, delta: f32) {
        // 0-3: filter env (A, D, S, R), 4-7: amp env (A, D, S, R)
        let is_amp = self.param_index >= 4;
        let env = if is_amp {
            &mut self.voice.amp_env
        } else {
            &mut self.voice.filter_env
        };
        let param_within = self.param_index % 4;

        match param_within {
            0 => {
                // Attack: 1ms to 10s (exponential)
                let mult = (delta * 0.05).exp2();
                env.attack = (env.attack * mult).clamp(0.001, 10.0);
            }
            1 => {
                // Decay
                let mult = (delta * 0.05).exp2();
                env.decay = (env.decay * mult).clamp(0.001, 10.0);
            }
            2 => {
                // Sustain: 0 to 1
                env.sustain = delta.mul_add(0.01, env.sustain).clamp(0.0, 1.0);
            }
            _ => {
                // Release
                let mult = (delta * 0.05).exp2();
                env.release = (env.release * mult).clamp(0.001, 10.0);
            }
        }
        env.recompute_coefficients();
    }

    fn adjust_performance_param(&mut self, delta: f32) {
        match self.param_index {
            0 => {
                // Glide rate (semitones/sec)
                let mult = (delta * 0.05).exp2();
                self.voice.glide.rate = (self.voice.glide.rate * mult).clamp(1.0, 1200.0);
            }
            1 => {
                // Glide on/off
                if delta.abs() > 0.0 {
                    self.voice.glide.enabled = !self.voice.glide.enabled;
                }
            }
            2 => {
                // Legato on/off
                if delta.abs() > 0.0 {
                    self.voice.legato = !self.voice.legato;
                }
            }
            3 => {
                // Retrigger on/off
                if delta.abs() > 0.0 {
                    self.voice.retrigger = !self.voice.retrigger;
                }
            }
            4 => {
                // Mod wheel amount: 0 to 1
                self.voice.xmod.mod_wheel = delta
                    .mul_add(0.01, self.voice.xmod.mod_wheel)
                    .clamp(0.0, 1.0);
            }
            _ => {
                // Mod wheel destination: cycle through
                if delta.abs() > 0.0 {
                    self.voice.xmod.mod_wheel_dest = self.voice.xmod.mod_wheel_dest.next();
                }
            }
        }
    }
}

use crate::synth::ladder::MoogLadder;
