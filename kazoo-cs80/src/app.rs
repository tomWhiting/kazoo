//! Application state for the CS-80 pad synth.

use crate::modular::graph::NodeGraph;
use crate::synth::{Cs80Synth, NUM_VOICES, SynthParams, VoiceStatus};

/// Top-level view mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    /// Normal synth editing view.
    Synth,
    /// Modular node graph view.
    Modular,
}

/// Which UI section is focused for editing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Layer1,
    Layer2,
    RingMod,
    Lfo,
    Mixer,
}

impl Section {
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Layer1 => Self::Layer2,
            Self::Layer2 => Self::RingMod,
            Self::RingMod => Self::Lfo,
            Self::Lfo => Self::Mixer,
            Self::Mixer => Self::Layer1,
        }
    }

    #[must_use]
    pub const fn prev(self) -> Self {
        match self {
            Self::Layer1 => Self::Mixer,
            Self::Layer2 => Self::Layer1,
            Self::RingMod => Self::Layer2,
            Self::Lfo => Self::RingMod,
            Self::Mixer => Self::Lfo,
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Layer1 => "LAYER I",
            Self::Layer2 => "LAYER II",
            Self::RingMod => "RING MOD",
            Self::Lfo => "LFO",
            Self::Mixer => "MIXER",
        }
    }

    /// Number of editable parameters in this section.
    #[must_use]
    pub const fn param_count(self) -> usize {
        match self {
            // VCO (waveform, octave, fine tune, pulse width) + HPF (cutoff, Q)
            // + LPF (cutoff, Q) + Filter Env (IL, AL, atk, dec, rel, depth)
            // + VCA (atk, dec, sus, rel) + level = 19
            Self::Layer1 | Self::Layer2 => 19,
            Self::RingMod => 4,
            // rate, waveform, pitch, filter, vca = 5
            Self::Lfo => 5,
            Self::Mixer => 3,
        }
    }
}

/// Waveform display buffer size (matches `Cs80Synth::HISTORY_SIZE`).
pub const WAVEFORM_BUF_SIZE: usize = 2048;

/// Full application state.
#[derive(Debug)]
pub struct App {
    /// The synth engine (UI-side, used only for param storage).
    pub synth: Cs80Synth,
    /// Whether the app should quit.
    pub should_quit: bool,
    /// Currently focused UI section.
    pub section: Section,
    /// Currently selected parameter index within the section.
    pub param_index: usize,
    /// Current keyboard octave offset.
    pub octave: i8,
    /// Set of currently held note keys (for note-off tracking).
    pub held_notes: [bool; 128],
    /// Maps keyboard character (ASCII byte) to the MIDI note that was
    /// triggered when the key was pressed. This ensures note-off uses the
    /// correct note even if the octave changed while the key was held.
    pub key_note_map: [Option<u8>; 128],
    /// Cached voice status for UI display.
    pub voice_status: [VoiceStatus; NUM_VOICES],
    /// Waveform display buffer (copied from audio thread).
    pub waveform_buf: Vec<f32>,
    /// Frame counter for animations.
    pub frame: u64,
    /// Current aftertouch pressure [0, 1].
    pub aftertouch: f32,
    /// Whether Shift is held (for coarse adjustments).
    pub shift_held: bool,
    /// Current view mode (synth editor or modular).
    pub view_mode: ViewMode,
    /// Modular node graph for the modular view.
    pub modular_graph: NodeGraph,
}

impl App {
    /// Create a new application state.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let synth = Cs80Synth::new(sample_rate);
        let voice_status = synth.voice_status();
        Self {
            synth,
            should_quit: false,
            section: Section::Layer1,
            param_index: 0,
            octave: 0,
            held_notes: [false; 128],
            key_note_map: [None; 128],
            voice_status,
            waveform_buf: vec![0.0; WAVEFORM_BUF_SIZE],
            frame: 0,
            aftertouch: 0.0,
            shift_held: false,
            view_mode: ViewMode::Synth,
            modular_graph: NodeGraph::new(sample_rate, 128),
        }
    }

    /// Move to next section.
    pub const fn next_section(&mut self) {
        self.section = self.section.next();
        self.param_index = 0;
    }

    /// Move to previous section.
    pub const fn prev_section(&mut self) {
        self.section = self.section.prev();
        self.param_index = 0;
    }

    /// Move to next parameter within current section.
    pub const fn next_param(&mut self) {
        let max = self.section.param_count();
        if max > 0 {
            self.param_index = (self.param_index + 1) % max;
        }
    }

    /// Move to previous parameter within current section.
    pub const fn prev_param(&mut self) {
        let max = self.section.param_count();
        if max > 0 {
            self.param_index = (self.param_index + max - 1) % max;
        }
    }

    /// Increment the currently selected parameter.
    pub fn increment_param(&mut self) {
        let multiplier = if self.shift_held { 10.0 } else { 1.0 };
        self.adjust_param(multiplier);
    }

    /// Decrement the currently selected parameter.
    pub fn decrement_param(&mut self) {
        let multiplier = if self.shift_held { -10.0 } else { -1.0 };
        self.adjust_param(multiplier);
    }

    /// Adjust the currently selected parameter by a signed step.
    /// `direction` magnitude > 1.0 for coarse (Shift+arrow), 1.0 for fine.
    fn adjust_param(&mut self, direction: f32) {
        let params = &mut self.synth.params;
        match self.section {
            Section::Layer1 => adjust_layer1_param(params, self.param_index, direction),
            Section::Layer2 => adjust_layer2_param(params, self.param_index, direction),
            Section::RingMod => adjust_ring_mod_param(params, self.param_index, direction),
            Section::Lfo => adjust_lfo_param(params, self.param_index, direction),
            Section::Mixer => adjust_mixer_param(params, self.param_index, direction),
        }
        self.synth.apply_params();
    }

    /// Play a note from keyboard input.
    pub fn note_on(&mut self, note: u8, velocity: f32) {
        if note < 128 {
            self.held_notes[note as usize] = true;
            self.synth.note_on(note, velocity);
        }
    }

    /// Release a note.
    pub fn note_off(&mut self, note: u8) {
        if note < 128 {
            self.held_notes[note as usize] = false;
            self.synth.note_off(note);
        }
    }

    /// Octave up.
    pub fn octave_up(&mut self) {
        self.octave = (self.octave + 1).min(4);
    }

    /// Octave down.
    pub fn octave_down(&mut self) {
        self.octave = (self.octave - 1).max(-4);
    }

    /// Toggle between synth editor and modular view.
    pub const fn toggle_view(&mut self) {
        self.view_mode = match self.view_mode {
            ViewMode::Synth => ViewMode::Modular,
            ViewMode::Modular => ViewMode::Synth,
        };
    }

    /// Increase aftertouch pressure. Returns new value.
    pub fn increase_aftertouch(&mut self) -> f32 {
        self.aftertouch = (self.aftertouch + 0.1).min(1.0);
        self.aftertouch
    }

    /// Decrease aftertouch pressure. Returns new value.
    pub fn decrease_aftertouch(&mut self) -> f32 {
        self.aftertouch = (self.aftertouch - 0.1).max(0.0);
        self.aftertouch
    }

    /// Get inline hint text for the currently selected parameter (if any).
    #[must_use]
    pub const fn current_param_hint(&self) -> Option<&'static str> {
        match self.section {
            Section::Layer1 | Section::Layer2 => match self.param_index {
                8 => Some("filter starts here at note-on"),
                9 => Some("filter sweeps to here during attack"),
                _ => None,
            },
            _ => None,
        }
    }

    /// Get current parameter name and value string for display.
    #[must_use]
    pub fn current_param_display(&self) -> (String, String) {
        let params = &self.synth.params;
        match self.section {
            Section::Layer1 => layer1_param_display(params, self.param_index),
            Section::Layer2 => layer2_param_display(params, self.param_index),
            Section::RingMod => ring_mod_param_display(params, self.param_index),
            Section::Lfo => lfo_param_display(params, self.param_index),
            Section::Mixer => mixer_param_display(params, self.param_index),
        }
    }
}

// ---------------------------------------------------------------------------
// Enum cycling helpers for discrete parameters
// ---------------------------------------------------------------------------

use crate::synth::lfo::LfoWaveform;
use crate::synth::oscillator::{OctaveRange, Waveform};

fn cycle_waveform(w: Waveform, dir: f32) -> Waveform {
    if dir > 0.0 {
        match w {
            Waveform::Sawtooth => Waveform::Pulse,
            Waveform::Pulse => Waveform::Sine,
            Waveform::Sine => Waveform::Sawtooth,
        }
    } else {
        match w {
            Waveform::Sawtooth => Waveform::Sine,
            Waveform::Pulse => Waveform::Sawtooth,
            Waveform::Sine => Waveform::Pulse,
        }
    }
}

fn cycle_octave(o: OctaveRange, dir: f32) -> OctaveRange {
    if dir > 0.0 {
        match o {
            OctaveRange::ThirtyTwo => OctaveRange::Sixteen,
            OctaveRange::Sixteen => OctaveRange::Eight,
            OctaveRange::Eight => OctaveRange::Four,
            OctaveRange::Four => OctaveRange::ThirtyTwo,
        }
    } else {
        match o {
            OctaveRange::ThirtyTwo => OctaveRange::Four,
            OctaveRange::Sixteen => OctaveRange::ThirtyTwo,
            OctaveRange::Eight => OctaveRange::Sixteen,
            OctaveRange::Four => OctaveRange::Eight,
        }
    }
}

fn cycle_lfo_waveform(w: LfoWaveform, dir: f32) -> LfoWaveform {
    if dir > 0.0 {
        match w {
            LfoWaveform::Sine => LfoWaveform::Sawtooth,
            LfoWaveform::Sawtooth => LfoWaveform::Ramp,
            LfoWaveform::Ramp => LfoWaveform::Pulse,
            LfoWaveform::Pulse => LfoWaveform::Noise,
            LfoWaveform::Noise => LfoWaveform::Sine,
        }
    } else {
        match w {
            LfoWaveform::Sine => LfoWaveform::Noise,
            LfoWaveform::Sawtooth => LfoWaveform::Sine,
            LfoWaveform::Ramp => LfoWaveform::Sawtooth,
            LfoWaveform::Pulse => LfoWaveform::Ramp,
            LfoWaveform::Noise => LfoWaveform::Pulse,
        }
    }
}

const fn waveform_name(w: Waveform) -> &'static str {
    match w {
        Waveform::Sawtooth => "Saw",
        Waveform::Pulse => "Pulse",
        Waveform::Sine => "Sine",
    }
}

const fn octave_name(o: OctaveRange) -> &'static str {
    match o {
        OctaveRange::ThirtyTwo => "32'",
        OctaveRange::Sixteen => "16'",
        OctaveRange::Eight => "8'",
        OctaveRange::Four => "4'",
    }
}

const fn lfo_waveform_name(w: LfoWaveform) -> &'static str {
    match w {
        LfoWaveform::Sine => "Sine",
        LfoWaveform::Sawtooth => "Saw",
        LfoWaveform::Ramp => "Ramp",
        LfoWaveform::Pulse => "Pulse",
        LfoWaveform::Noise => "Noise",
    }
}

/// Format a time value in seconds as a human-readable string (ms or s).
fn format_time(seconds: f32) -> String {
    if seconds < 1.0 {
        format!("{:.0}ms", seconds * 1000.0)
    } else {
        format!("{seconds:.2}s")
    }
}

// ---------------------------------------------------------------------------
// Parameter adjustment helpers
// ---------------------------------------------------------------------------

fn adjust_layer1_param(p: &mut SynthParams, idx: usize, dir: f32) {
    match idx {
        // VCO
        0 => p.layer1_waveform = cycle_waveform(p.layer1_waveform, dir),
        1 => p.layer1_octave = cycle_octave(p.layer1_octave, dir),
        2 => p.layer1_fine_tune = dir.mul_add(1.0, p.layer1_fine_tune).clamp(-100.0, 100.0),
        3 => p.layer1_pulse_width = dir.mul_add(0.05, p.layer1_pulse_width).clamp(0.5, 0.9),
        // HPF
        4 => p.layer1_hpf_cutoff = dir.mul_add(10.0, p.layer1_hpf_cutoff).clamp(20.0, 20000.0),
        5 => p.layer1_hpf_resonance = dir.mul_add(0.05, p.layer1_hpf_resonance).clamp(0.0, 0.95),
        // LPF
        6 => p.layer1_lpf_cutoff = dir.mul_add(50.0, p.layer1_lpf_cutoff).clamp(20.0, 20000.0),
        7 => p.layer1_lpf_resonance = dir.mul_add(0.05, p.layer1_lpf_resonance).clamp(0.0, 0.95),
        // Filter Envelope (IL/AL)
        8 => p.layer1_filter_env_il = dir.mul_add(0.05, p.layer1_filter_env_il).clamp(0.0, 1.0),
        9 => p.layer1_filter_env_al = dir.mul_add(0.05, p.layer1_filter_env_al).clamp(0.0, 1.0),
        10 => {
            p.layer1_filter_env_attack = dir
                .mul_add(0.01, p.layer1_filter_env_attack)
                .clamp(0.001, 10.0);
        }
        11 => {
            p.layer1_filter_env_decay = dir
                .mul_add(0.01, p.layer1_filter_env_decay)
                .clamp(0.001, 10.0);
        }
        12 => {
            p.layer1_filter_env_release = dir
                .mul_add(0.01, p.layer1_filter_env_release)
                .clamp(0.001, 10.0);
        }
        13 => {
            p.layer1_filter_env_depth = dir
                .mul_add(100.0, p.layer1_filter_env_depth)
                .clamp(0.0, 20000.0);
        }
        // VCA Envelope
        14 => p.layer1_vca_attack = dir.mul_add(0.01, p.layer1_vca_attack).clamp(0.001, 10.0),
        15 => p.layer1_vca_decay = dir.mul_add(0.01, p.layer1_vca_decay).clamp(0.001, 10.0),
        16 => p.layer1_vca_sustain = dir.mul_add(0.05, p.layer1_vca_sustain).clamp(0.0, 1.0),
        17 => p.layer1_vca_release = dir.mul_add(0.01, p.layer1_vca_release).clamp(0.001, 10.0),
        // Output
        18 => p.layer1_level = dir.mul_add(0.05, p.layer1_level).clamp(0.0, 1.0),
        _ => {}
    }
}

fn adjust_layer2_param(p: &mut SynthParams, idx: usize, dir: f32) {
    match idx {
        // VCO
        0 => p.layer2_waveform = cycle_waveform(p.layer2_waveform, dir),
        1 => p.layer2_octave = cycle_octave(p.layer2_octave, dir),
        2 => p.layer2_fine_tune = dir.mul_add(1.0, p.layer2_fine_tune).clamp(-100.0, 100.0),
        3 => p.layer2_pulse_width = dir.mul_add(0.05, p.layer2_pulse_width).clamp(0.5, 0.9),
        // HPF
        4 => p.layer2_hpf_cutoff = dir.mul_add(10.0, p.layer2_hpf_cutoff).clamp(20.0, 20000.0),
        5 => p.layer2_hpf_resonance = dir.mul_add(0.05, p.layer2_hpf_resonance).clamp(0.0, 0.95),
        // LPF
        6 => p.layer2_lpf_cutoff = dir.mul_add(50.0, p.layer2_lpf_cutoff).clamp(20.0, 20000.0),
        7 => p.layer2_lpf_resonance = dir.mul_add(0.05, p.layer2_lpf_resonance).clamp(0.0, 0.95),
        // Filter Envelope (IL/AL)
        8 => p.layer2_filter_env_il = dir.mul_add(0.05, p.layer2_filter_env_il).clamp(0.0, 1.0),
        9 => p.layer2_filter_env_al = dir.mul_add(0.05, p.layer2_filter_env_al).clamp(0.0, 1.0),
        10 => {
            p.layer2_filter_env_attack = dir
                .mul_add(0.01, p.layer2_filter_env_attack)
                .clamp(0.001, 10.0);
        }
        11 => {
            p.layer2_filter_env_decay = dir
                .mul_add(0.01, p.layer2_filter_env_decay)
                .clamp(0.001, 10.0);
        }
        12 => {
            p.layer2_filter_env_release = dir
                .mul_add(0.01, p.layer2_filter_env_release)
                .clamp(0.001, 10.0);
        }
        13 => {
            p.layer2_filter_env_depth = dir
                .mul_add(100.0, p.layer2_filter_env_depth)
                .clamp(0.0, 20000.0);
        }
        // VCA Envelope
        14 => p.layer2_vca_attack = dir.mul_add(0.01, p.layer2_vca_attack).clamp(0.001, 10.0),
        15 => p.layer2_vca_decay = dir.mul_add(0.01, p.layer2_vca_decay).clamp(0.001, 10.0),
        16 => p.layer2_vca_sustain = dir.mul_add(0.05, p.layer2_vca_sustain).clamp(0.0, 1.0),
        17 => p.layer2_vca_release = dir.mul_add(0.01, p.layer2_vca_release).clamp(0.001, 10.0),
        // Output
        18 => p.layer2_level = dir.mul_add(0.05, p.layer2_level).clamp(0.0, 1.0),
        _ => {}
    }
}

fn adjust_ring_mod_param(p: &mut SynthParams, idx: usize, dir: f32) {
    match idx {
        0 => p.ring_mod_depth = dir.mul_add(0.05, p.ring_mod_depth).clamp(0.0, 1.0),
        1 => {
            p.ring_mod_carrier_freq = dir
                .mul_add(10.0, p.ring_mod_carrier_freq)
                .clamp(20.0, 5000.0);
        }
        2 => p.ring_mod_attack = dir.mul_add(0.001, p.ring_mod_attack).clamp(0.0005, 1.0),
        3 => p.ring_mod_decay = dir.mul_add(0.01, p.ring_mod_decay).clamp(0.001, 10.0),
        _ => {}
    }
}

fn adjust_lfo_param(p: &mut SynthParams, idx: usize, dir: f32) {
    match idx {
        0 => p.lfo_rate = dir.mul_add(0.1, p.lfo_rate).clamp(0.01, 100.0),
        1 => p.lfo_waveform = cycle_lfo_waveform(p.lfo_waveform, dir),
        2 => {
            p.lfo_routing.pitch_cents = dir
                .mul_add(1.0, p.lfo_routing.pitch_cents)
                .clamp(0.0, 100.0);
        }
        3 => {
            p.lfo_routing.filter_depth = dir
                .mul_add(0.05, p.lfo_routing.filter_depth)
                .clamp(0.0, 1.0);
        }
        4 => p.lfo_routing.vca_depth = dir.mul_add(0.05, p.lfo_routing.vca_depth).clamp(0.0, 1.0),
        _ => {}
    }
}

fn adjust_mixer_param(p: &mut SynthParams, idx: usize, dir: f32) {
    match idx {
        0 => p.layer_mix = dir.mul_add(0.05, p.layer_mix).clamp(0.0, 1.0),
        1 => p.master_level = dir.mul_add(0.05, p.master_level).clamp(0.0, 1.0),
        2 => p.drift_cents = dir.mul_add(0.5, p.drift_cents).clamp(0.0, 10.0),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Parameter display helpers
// ---------------------------------------------------------------------------

pub fn layer1_param_display(p: &SynthParams, idx: usize) -> (String, String) {
    match idx {
        // VCO
        0 => ("Waveform".into(), waveform_name(p.layer1_waveform).into()),
        1 => ("Octave".into(), octave_name(p.layer1_octave).into()),
        2 => ("Fine".into(), format!("{:+.0}c", p.layer1_fine_tune)),
        3 => ("PW".into(), format!("{:.0}%", p.layer1_pulse_width * 100.0)),
        // HPF
        4 => ("HPF Freq".into(), format!("{:.0} Hz", p.layer1_hpf_cutoff)),
        5 => ("HPF Q".into(), format!("{:.2}", p.layer1_hpf_resonance)),
        // LPF
        6 => ("LPF Freq".into(), format!("{:.0} Hz", p.layer1_lpf_cutoff)),
        7 => ("LPF Q".into(), format!("{:.2}", p.layer1_lpf_resonance)),
        // Filter Envelope
        8 => (
            "IL (Start)".into(),
            format!("{:.0}%", p.layer1_filter_env_il * 100.0),
        ),
        9 => (
            "AL (Peak)".into(),
            format!("{:.0}%", p.layer1_filter_env_al * 100.0),
        ),
        10 => ("Filt Atk".into(), format_time(p.layer1_filter_env_attack)),
        11 => ("Filt Dec".into(), format_time(p.layer1_filter_env_decay)),
        12 => ("Filt Rel".into(), format_time(p.layer1_filter_env_release)),
        13 => (
            "Filt Depth".into(),
            format!("{:.0} Hz", p.layer1_filter_env_depth),
        ),
        // VCA Envelope
        14 => ("VCA Atk".into(), format_time(p.layer1_vca_attack)),
        15 => ("VCA Dec".into(), format_time(p.layer1_vca_decay)),
        16 => (
            "VCA Sus".into(),
            format!("{:.0}%", p.layer1_vca_sustain * 100.0),
        ),
        17 => ("VCA Rel".into(), format_time(p.layer1_vca_release)),
        // Output
        18 => ("Level".into(), format!("{:.0}%", p.layer1_level * 100.0)),
        _ => ("?".into(), "?".into()),
    }
}

pub fn layer2_param_display(p: &SynthParams, idx: usize) -> (String, String) {
    match idx {
        // VCO
        0 => ("Waveform".into(), waveform_name(p.layer2_waveform).into()),
        1 => ("Octave".into(), octave_name(p.layer2_octave).into()),
        2 => ("Fine".into(), format!("{:+.0}c", p.layer2_fine_tune)),
        3 => ("PW".into(), format!("{:.0}%", p.layer2_pulse_width * 100.0)),
        // HPF
        4 => ("HPF Freq".into(), format!("{:.0} Hz", p.layer2_hpf_cutoff)),
        5 => ("HPF Q".into(), format!("{:.2}", p.layer2_hpf_resonance)),
        // LPF
        6 => ("LPF Freq".into(), format!("{:.0} Hz", p.layer2_lpf_cutoff)),
        7 => ("LPF Q".into(), format!("{:.2}", p.layer2_lpf_resonance)),
        // Filter Envelope
        8 => (
            "IL (Start)".into(),
            format!("{:.0}%", p.layer2_filter_env_il * 100.0),
        ),
        9 => (
            "AL (Peak)".into(),
            format!("{:.0}%", p.layer2_filter_env_al * 100.0),
        ),
        10 => ("Filt Atk".into(), format_time(p.layer2_filter_env_attack)),
        11 => ("Filt Dec".into(), format_time(p.layer2_filter_env_decay)),
        12 => ("Filt Rel".into(), format_time(p.layer2_filter_env_release)),
        13 => (
            "Filt Depth".into(),
            format!("{:.0} Hz", p.layer2_filter_env_depth),
        ),
        // VCA Envelope
        14 => ("VCA Atk".into(), format_time(p.layer2_vca_attack)),
        15 => ("VCA Dec".into(), format_time(p.layer2_vca_decay)),
        16 => (
            "VCA Sus".into(),
            format!("{:.0}%", p.layer2_vca_sustain * 100.0),
        ),
        17 => ("VCA Rel".into(), format_time(p.layer2_vca_release)),
        // Output
        18 => ("Level".into(), format!("{:.0}%", p.layer2_level * 100.0)),
        _ => ("?".into(), "?".into()),
    }
}

pub fn ring_mod_param_display(p: &SynthParams, idx: usize) -> (String, String) {
    match idx {
        0 => ("Depth".into(), format!("{:.0}%", p.ring_mod_depth * 100.0)),
        1 => (
            "Carrier".into(),
            format!("{:.0} Hz", p.ring_mod_carrier_freq),
        ),
        2 => (
            "Attack".into(),
            format!("{:.1}ms", p.ring_mod_attack * 1000.0),
        ),
        3 => (
            "Decay".into(),
            format!("{:.0}ms", p.ring_mod_decay * 1000.0),
        ),
        _ => ("?".into(), "?".into()),
    }
}

pub fn lfo_param_display(p: &SynthParams, idx: usize) -> (String, String) {
    match idx {
        0 => ("Rate".into(), format!("{:.1} Hz", p.lfo_rate)),
        1 => ("Shape".into(), lfo_waveform_name(p.lfo_waveform).into()),
        2 => ("Pitch".into(), format!("{:.0}c", p.lfo_routing.pitch_cents)),
        3 => (
            "Filter".into(),
            format!("{:.0}%", p.lfo_routing.filter_depth * 100.0),
        ),
        4 => (
            "VCA".into(),
            format!("{:.0}%", p.lfo_routing.vca_depth * 100.0),
        ),
        _ => ("?".into(), "?".into()),
    }
}

pub fn mixer_param_display(p: &SynthParams, idx: usize) -> (String, String) {
    match idx {
        0 => (
            "Layer Mix".into(),
            format!(
                "I {:.0}% / II {:.0}%",
                (1.0 - p.layer_mix) * 100.0,
                p.layer_mix * 100.0
            ),
        ),
        1 => ("Master".into(), format!("{:.0}%", p.master_level * 100.0)),
        2 => ("Drift".into(), format!("{:.1}c", p.drift_cents)),
        _ => ("?".into(), "?".into()),
    }
}
