//! Application state for the Prophet TUI.

use kazoo_prophet::synth::oscillator::{OctaveRange, Waveform};
use kazoo_prophet::{NUM_VOICES, SynthParams, VoiceStatus};

/// Waveform buffer length copied from the audio callback.
pub const WAVEFORM_BUF_SIZE: usize = 1024;

/// Focused parameter group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Oscillators,
    Mixer,
    Filter,
    Envelopes,
    PolyMod,
    Performance,
}

impl Section {
    pub const ALL: [Self; 6] = [
        Self::Oscillators,
        Self::Mixer,
        Self::Filter,
        Self::Envelopes,
        Self::PolyMod,
        Self::Performance,
    ];

    #[must_use]
    pub fn next(self) -> Self {
        let idx = Self::ALL
            .iter()
            .position(|&section| section == self)
            .unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }

    #[must_use]
    pub fn prev(self) -> Self {
        let idx = Self::ALL
            .iter()
            .position(|&section| section == self)
            .unwrap_or(0);
        Self::ALL[(idx + Self::ALL.len() - 1) % Self::ALL.len()]
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Oscillators => "OSCILLATORS",
            Self::Mixer => "MIXER",
            Self::Filter => "FILTER",
            Self::Envelopes => "ENVELOPES",
            Self::PolyMod => "POLY-MOD",
            Self::Performance => "PERFORMANCE",
        }
    }

    #[must_use]
    pub const fn param_count(self) -> usize {
        match self {
            Self::Oscillators => 10,
            Self::Mixer => 2,
            Self::Filter => 4,
            Self::Envelopes => 8,
            Self::PolyMod => 5,
            Self::Performance => 3,
        }
    }
}

/// Full UI-side state.
#[derive(Debug)]
pub struct App {
    pub should_quit: bool,
    pub params: SynthParams,
    pub section: Section,
    pub param_index: usize,
    pub sample_rate: u32,
    pub voice_status: [VoiceStatus; NUM_VOICES],
    pub waveform_buf: [f32; WAVEFORM_BUF_SIZE],
    pub held_notes: [Option<u8>; 16],
}

impl App {
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        Self {
            should_quit: false,
            params: SynthParams::default(),
            section: Section::Oscillators,
            param_index: 0,
            sample_rate,
            voice_status: [VoiceStatus {
                index: 0,
                active: false,
                releasing: false,
                note: None,
                drift_cents: 0.0,
            }; NUM_VOICES],
            waveform_buf: [0.0; WAVEFORM_BUF_SIZE],
            held_notes: [None; 16],
        }
    }

    pub fn next_section(&mut self) {
        self.section = self.section.next();
        self.param_index = 0;
    }

    pub fn prev_section(&mut self) {
        self.section = self.section.prev();
        self.param_index = 0;
    }

    pub const fn next_param(&mut self) {
        self.param_index = (self.param_index + 1) % self.section.param_count();
    }

    pub const fn prev_param(&mut self) {
        self.param_index =
            (self.param_index + self.section.param_count() - 1) % self.section.param_count();
    }

    pub fn adjust_param(&mut self, delta: f32) {
        match self.section {
            Section::Oscillators => self.adjust_oscillator(delta),
            Section::Mixer => self.adjust_mixer(delta),
            Section::Filter => self.adjust_filter(delta),
            Section::Envelopes => self.adjust_envelope(delta),
            Section::PolyMod => self.adjust_poly_mod(delta),
            Section::Performance => self.adjust_performance(delta),
        }
    }

    pub fn add_held_note(&mut self, note: u8) {
        if self.held_notes.contains(&Some(note)) {
            return;
        }
        if let Some(slot) = self.held_notes.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(note);
        }
    }

    pub fn remove_held_note(&mut self, note: u8) {
        if let Some(slot) = self.held_notes.iter_mut().find(|slot| **slot == Some(note)) {
            *slot = None;
        }
    }

    pub fn param_rows(&self) -> Vec<String> {
        match self.section {
            Section::Oscillators => self.oscillator_rows(),
            Section::Mixer => vec![
                format!("NOISE LEVEL     {:.2}", self.params.mixer.noise_level),
                format!("PRE-FILTER GAIN {:.2}", self.params.mixer.pre_filter_gain),
            ],
            Section::Filter => vec![
                format!("CUTOFF          {:.0} Hz", self.params.filter.cutoff_hz),
                format!("RESONANCE       {:.2}", self.params.filter.resonance),
                format!("KEY TRACK       {:.2}", self.params.filter.key_track),
                format!("ENV AMOUNT      {:.2}", self.params.filter.envelope_amount),
            ],
            Section::Envelopes => self.envelope_rows(),
            Section::PolyMod => self.poly_mod_rows(),
            Section::Performance => vec![
                format!(
                    "VOICE DRIFT     {:.1} cents",
                    self.params.drift.voice_detune_cents
                ),
                format!(
                    "OSC B DRIFT     {:.2}x",
                    self.params.drift.oscillator_b_detune_scale
                ),
                format!("MASTER LEVEL    {:.2}", self.params.master_level),
            ],
        }
    }

    fn oscillator_rows(&self) -> Vec<String> {
        vec![
            format!(
                "OSC A WAVE      {}",
                wave_name(self.params.oscillator_a.waveform)
            ),
            format!(
                "OSC A OCTAVE    {}",
                octave_name(self.params.oscillator_a.octave)
            ),
            format!(
                "OSC A FINE      {:+.1} cents",
                self.params.oscillator_a.fine_tune_cents
            ),
            format!(
                "OSC A WIDTH     {:.2}",
                self.params.oscillator_a.pulse_width
            ),
            format!("OSC A LEVEL     {:.2}", self.params.oscillator_a.level),
            format!(
                "OSC B WAVE      {}",
                wave_name(self.params.oscillator_b.waveform)
            ),
            format!(
                "OSC B OCTAVE    {}",
                octave_name(self.params.oscillator_b.octave)
            ),
            format!(
                "OSC B FINE      {:+.1} cents",
                self.params.oscillator_b.fine_tune_cents
            ),
            format!(
                "OSC B WIDTH     {:.2}",
                self.params.oscillator_b.pulse_width
            ),
            format!("OSC B LEVEL     {:.2}", self.params.oscillator_b.level),
        ]
    }

    fn envelope_rows(&self) -> Vec<String> {
        vec![
            format!("FLT ATTACK      {:.3}s", self.params.filter_envelope.attack),
            format!("FLT DECAY       {:.3}s", self.params.filter_envelope.decay),
            format!("FLT SUSTAIN     {:.2}", self.params.filter_envelope.sustain),
            format!(
                "FLT RELEASE     {:.3}s",
                self.params.filter_envelope.release
            ),
            format!(
                "AMP ATTACK      {:.3}s",
                self.params.amplifier_envelope.attack
            ),
            format!(
                "AMP DECAY       {:.3}s",
                self.params.amplifier_envelope.decay
            ),
            format!(
                "AMP SUSTAIN     {:.2}",
                self.params.amplifier_envelope.sustain
            ),
            format!(
                "AMP RELEASE     {:.3}s",
                self.params.amplifier_envelope.release
            ),
        ]
    }

    fn poly_mod_rows(&self) -> Vec<String> {
        vec![
            format!(
                "OSC B -> OSC A  {:.0} cents",
                self.params.poly_mod.osc_b_to_osc_a_cents
            ),
            format!(
                "OSC B -> FILTER {:.0} Hz",
                self.params.poly_mod.osc_b_to_filter_hz
            ),
            format!(
                "ENV -> OSC A    {:.0} cents",
                self.params.poly_mod.filter_env_to_osc_a_cents
            ),
            format!(
                "ENV -> FILTER   {:.0} Hz",
                self.params.poly_mod.filter_env_to_filter_hz
            ),
            format!(
                "OSC SYNC        {}",
                on_off(self.params.poly_mod.oscillator_sync)
            ),
        ]
    }

    fn adjust_oscillator(&mut self, delta: f32) {
        let osc = if self.param_index < 5 {
            &mut self.params.oscillator_a
        } else {
            &mut self.params.oscillator_b
        };
        match self.param_index % 5 {
            0 => osc.waveform = cycle_wave(osc.waveform, delta),
            1 => osc.octave = cycle_octave(osc.octave, delta),
            2 => osc.fine_tune_cents = (osc.fine_tune_cents + delta).clamp(-1200.0, 1200.0),
            3 => osc.pulse_width = delta.mul_add(0.005, osc.pulse_width).clamp(0.05, 0.95),
            _ => osc.level = delta.mul_add(0.01, osc.level).clamp(0.0, 1.2),
        }
    }

    fn adjust_mixer(&mut self, delta: f32) {
        let d = delta * 0.01;
        if self.param_index == 0 {
            self.params.mixer.noise_level = (self.params.mixer.noise_level + d).clamp(0.0, 0.5);
        } else {
            self.params.mixer.pre_filter_gain =
                (self.params.mixer.pre_filter_gain + d).clamp(0.05, 1.0);
        }
    }

    fn adjust_filter(&mut self, delta: f32) {
        match self.param_index {
            0 => {
                self.params.filter.cutoff_hz =
                    (self.params.filter.cutoff_hz * (delta * 0.02).exp2()).clamp(20.0, 18_000.0);
            }
            1 => {
                self.params.filter.resonance = delta
                    .mul_add(0.01, self.params.filter.resonance)
                    .clamp(0.0, 0.92);
            }
            2 => {
                self.params.filter.key_track = delta
                    .mul_add(0.01, self.params.filter.key_track)
                    .clamp(0.0, 1.0);
            }
            _ => {
                self.params.filter.envelope_amount = delta
                    .mul_add(0.01, self.params.filter.envelope_amount)
                    .clamp(-1.0, 1.0);
            }
        }
    }

    fn adjust_envelope(&mut self, delta: f32) {
        let env = if self.param_index < 4 {
            &mut self.params.filter_envelope
        } else {
            &mut self.params.amplifier_envelope
        };
        match self.param_index % 4 {
            0 => env.attack = (env.attack * (delta * 0.05).exp2()).clamp(0.001, 10.0),
            1 => env.decay = (env.decay * (delta * 0.05).exp2()).clamp(0.001, 10.0),
            2 => env.sustain = delta.mul_add(0.01, env.sustain).clamp(0.0, 1.0),
            _ => env.release = (env.release * (delta * 0.05).exp2()).clamp(0.001, 15.0),
        }
    }

    fn adjust_poly_mod(&mut self, delta: f32) {
        match self.param_index {
            0 => {
                self.params.poly_mod.osc_b_to_osc_a_cents = delta
                    .mul_add(10.0, self.params.poly_mod.osc_b_to_osc_a_cents)
                    .clamp(-2400.0, 2400.0);
            }
            1 => {
                self.params.poly_mod.osc_b_to_filter_hz = delta
                    .mul_add(25.0, self.params.poly_mod.osc_b_to_filter_hz)
                    .clamp(-8000.0, 8000.0);
            }
            2 => {
                self.params.poly_mod.filter_env_to_osc_a_cents = delta
                    .mul_add(10.0, self.params.poly_mod.filter_env_to_osc_a_cents)
                    .clamp(-2400.0, 2400.0);
            }
            3 => {
                self.params.poly_mod.filter_env_to_filter_hz = delta
                    .mul_add(25.0, self.params.poly_mod.filter_env_to_filter_hz)
                    .clamp(-8000.0, 8000.0);
            }
            _ if delta.abs() > 0.0 => {
                self.params.poly_mod.oscillator_sync = !self.params.poly_mod.oscillator_sync;
            }
            _ => {}
        }
    }

    fn adjust_performance(&mut self, delta: f32) {
        match self.param_index {
            0 => {
                self.params.drift.voice_detune_cents = delta
                    .mul_add(0.2, self.params.drift.voice_detune_cents)
                    .clamp(0.0, 25.0);
            }
            1 => {
                self.params.drift.oscillator_b_detune_scale = delta
                    .mul_add(0.01, self.params.drift.oscillator_b_detune_scale)
                    .clamp(-2.0, 2.0);
            }
            _ => {
                self.params.master_level = delta
                    .mul_add(0.01, self.params.master_level)
                    .clamp(0.0, 1.0);
            }
        }
    }
}

const fn cycle_wave(wave: Waveform, delta: f32) -> Waveform {
    match (wave, delta.is_sign_positive()) {
        (Waveform::Saw, true) | (Waveform::Triangle, false) => Waveform::Pulse,
        (Waveform::Pulse, true) | (Waveform::Saw, false) => Waveform::Triangle,
        (Waveform::Triangle, true) | (Waveform::Pulse, false) => Waveform::Saw,
    }
}

const fn cycle_octave(octave: OctaveRange, delta: f32) -> OctaveRange {
    match (octave, delta.is_sign_positive()) {
        (OctaveRange::Footage32, true) | (OctaveRange::Footage8, false) => OctaveRange::Footage16,
        (OctaveRange::Footage16, true) | (OctaveRange::Footage4, false) => OctaveRange::Footage8,
        (OctaveRange::Footage8, true) | (OctaveRange::Footage32, false) => OctaveRange::Footage4,
        (OctaveRange::Footage4, true) | (OctaveRange::Footage16, false) => OctaveRange::Footage32,
    }
}

const fn wave_name(wave: Waveform) -> &'static str {
    match wave {
        Waveform::Saw => "SAW",
        Waveform::Pulse => "PULSE",
        Waveform::Triangle => "TRI",
    }
}

const fn octave_name(octave: OctaveRange) -> &'static str {
    match octave {
        OctaveRange::Footage32 => "32'",
        OctaveRange::Footage16 => "16'",
        OctaveRange::Footage8 => "8'",
        OctaveRange::Footage4 => "4'",
    }
}

const fn on_off(value: bool) -> &'static str {
    if value { "ON" } else { "OFF" }
}
