//! Application state for the Juno TUI.

use kazoo_juno::{NUM_VOICES, SynthParams, VoiceStatus};

pub const WAVEFORM_BUF_SIZE: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Dco,
    Filter,
    Envelope,
    Chorus,
    Performance,
}

impl Section {
    pub const ALL: [Self; 5] = [
        Self::Dco,
        Self::Filter,
        Self::Envelope,
        Self::Chorus,
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
            Self::Dco => "DCO",
            Self::Filter => "FILTER",
            Self::Envelope => "ENVELOPE",
            Self::Chorus => "CHORUS",
            Self::Performance => "PERFORMANCE",
        }
    }

    #[must_use]
    pub const fn param_count(self) -> usize {
        match self {
            Self::Dco => 7,
            Self::Filter => 5,
            Self::Envelope => 4,
            Self::Chorus => 3,
            Self::Performance => 2,
        }
    }
}

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
    pub key_note_map: [Option<u8>; 128],
}

impl App {
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        Self {
            should_quit: false,
            params: SynthParams::default(),
            section: Section::Dco,
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
            key_note_map: [None; 128],
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
            Section::Dco => self.adjust_dco(delta),
            Section::Filter => self.adjust_filter(delta),
            Section::Envelope => self.adjust_envelope(delta),
            Section::Chorus => self.adjust_chorus(delta),
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
            Section::Dco => vec![
                format!("SAW LEVEL      {:.2}", self.params.dco.saw_level),
                format!("PULSE LEVEL    {:.2}", self.params.dco.pulse_level),
                format!("SUB LEVEL      {:.2}", self.params.dco.sub_level),
                format!("NOISE LEVEL    {:.2}", self.params.dco.noise_level),
                format!("PULSE WIDTH    {:.2}", self.params.dco.pulse_width),
                format!("PWM DEPTH      {:.2}", self.params.dco.pwm_depth),
                format!("LFO RATE       {:.2} Hz", self.params.dco.lfo_rate_hz),
            ],
            Section::Filter => vec![
                format!("HPF AMOUNT     {:.2}", self.params.filter.hpf_amount),
                format!("CUTOFF         {:.0} Hz", self.params.filter.cutoff_hz),
                format!("RESONANCE      {:.2}", self.params.filter.resonance),
                format!("ENV AMOUNT     {:.2}", self.params.filter.envelope_amount),
                format!("KEY TRACK      {:.2}", self.params.filter.key_track),
            ],
            Section::Envelope => vec![
                format!("ATTACK         {:.3}s", self.params.envelope.attack),
                format!("DECAY          {:.3}s", self.params.envelope.decay),
                format!("SUSTAIN        {:.2}", self.params.envelope.sustain),
                format!("RELEASE        {:.3}s", self.params.envelope.release),
            ],
            Section::Chorus => vec![
                format!("MODE           {}", self.params.chorus.mode.label()),
                format!("MIX            {:.2}", self.params.chorus.mix),
                format!("BBD HISS       {:.3}", self.params.chorus.noise),
            ],
            Section::Performance => vec![
                format!("VOICE DRIFT    {:.1} cents", self.params.voice_drift_cents),
                format!("MASTER LEVEL   {:.2}", self.params.master_level),
            ],
        }
    }

    fn adjust_dco(&mut self, delta: f32) {
        let small = delta * 0.02;
        match self.param_index {
            0 => self.params.dco.saw_level = clamp01(self.params.dco.saw_level + small),
            1 => self.params.dco.pulse_level = clamp01(self.params.dco.pulse_level + small),
            2 => self.params.dco.sub_level = clamp01(self.params.dco.sub_level + small),
            3 => self.params.dco.noise_level = (self.params.dco.noise_level + small).clamp(0.0, 0.5),
            4 => self.params.dco.pulse_width = (self.params.dco.pulse_width + small).clamp(0.08, 0.92),
            5 => self.params.dco.pwm_depth = (self.params.dco.pwm_depth + small).clamp(0.0, 0.45),
            6 => {
                self.params.dco.lfo_rate_hz = delta
                    .mul_add(0.05, self.params.dco.lfo_rate_hz)
                    .clamp(0.05, 12.0);
            }
            _ => {}
        }
    }

    fn adjust_filter(&mut self, delta: f32) {
        match self.param_index {
            0 => self.params.filter.hpf_amount = clamp01(delta.mul_add(0.03, self.params.filter.hpf_amount)),
            1 => {
                self.params.filter.cutoff_hz =
                    (self.params.filter.cutoff_hz * (delta * 0.05).exp2()).clamp(40.0, 18_000.0);
            }
            2 => self.params.filter.resonance = delta.mul_add(0.025, self.params.filter.resonance),
            3 => self.params.filter.envelope_amount = clamp01(delta.mul_add(0.025, self.params.filter.envelope_amount)),
            4 => self.params.filter.key_track = clamp01(delta.mul_add(0.025, self.params.filter.key_track)),
            _ => {}
        }
        self.params.filter.resonance = self.params.filter.resonance.clamp(0.0, 0.96);
    }

    fn adjust_envelope(&mut self, delta: f32) {
        match self.param_index {
            0 => self.params.envelope.attack = scale_time(self.params.envelope.attack, delta),
            1 => self.params.envelope.decay = scale_time(self.params.envelope.decay, delta),
            2 => self.params.envelope.sustain = clamp01(delta.mul_add(0.03, self.params.envelope.sustain)),
            3 => self.params.envelope.release = scale_time(self.params.envelope.release, delta),
            _ => {}
        }
    }

    fn adjust_chorus(&mut self, delta: f32) {
        match self.param_index {
            0 if delta.abs() > 0.0 => self.params.chorus.mode = self.params.chorus.mode.next(),
            1 => self.params.chorus.mix = clamp01(delta.mul_add(0.03, self.params.chorus.mix)),
            2 => self.params.chorus.noise = delta.mul_add(0.003, self.params.chorus.noise).clamp(0.0, 0.08),
            _ => {}
        }
    }

    fn adjust_performance(&mut self, delta: f32) {
        match self.param_index {
            0 => self.params.voice_drift_cents = delta.mul_add(0.2, self.params.voice_drift_cents).clamp(0.0, 8.0),
            1 => self.params.master_level = delta.mul_add(0.02, self.params.master_level).clamp(0.0, 0.8),
            _ => {}
        }
    }
}

const fn clamp01(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

fn scale_time(value: f32, delta: f32) -> f32 {
    (value * (delta * 0.08).exp2()).clamp(0.001, 10.0)
}
