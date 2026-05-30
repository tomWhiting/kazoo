//! Concrete node implementations for the modular graph.
//!
//! Reuses synth modules where possible.

use std::f32::consts::TAU;

use kazoo_core::sanitize_sample;

use super::node::{ModularNode, PortDescriptor, PortType};

// ---------------------------------------------------------------------------
// Oscillator Node
// ---------------------------------------------------------------------------

/// Simple oscillator node for the modular graph.
#[derive(Debug)]
pub struct OscNode {
    phase: f32,
    frequency: f32,
    sample_rate: f32,
    inputs: Vec<PortDescriptor>,
    outputs: Vec<PortDescriptor>,
}

impl OscNode {
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        Self {
            phase: 0.0,
            frequency: 440.0,
            sample_rate: sample_rate.max(1.0),
            inputs: vec![PortDescriptor {
                name: "FM".to_string(),
                port_type: PortType::Audio,
            }],
            outputs: vec![PortDescriptor {
                name: "Out".to_string(),
                port_type: PortType::Audio,
            }],
        }
    }
}

impl ModularNode for OscNode {
    fn name(&self) -> &'static str {
        "VCO"
    }

    fn inputs(&self) -> &[PortDescriptor] {
        &self.inputs
    }

    fn outputs(&self) -> &[PortDescriptor] {
        &self.outputs
    }

    fn process(&mut self, input_buffers: &[&[f32]], output_buffers: &mut [&mut [f32]]) {
        let fm_input = input_buffers.first().map_or(&[] as &[f32], |b| *b);
        let Some(output) = output_buffers.first_mut() else {
            return;
        };

        for (i, sample) in output.iter_mut().enumerate() {
            let fm = if i < fm_input.len() { fm_input[i] } else { 0.0 };
            let freq = (self.frequency + fm * 100.0).max(0.1);
            let phase_inc = freq / self.sample_rate;

            *sample = sanitize_sample((self.phase * TAU).sin());

            self.phase += phase_inc;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
            }
            if !self.phase.is_finite() {
                self.phase = 0.0;
            }
        }
    }

    fn reset(&mut self) {
        self.phase = 0.0;
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
    }

    fn param_count(&self) -> usize {
        1
    }

    fn param_info(&self, index: usize) -> Option<(String, f32, f32, f32)> {
        if index == 0 {
            Some(("Frequency".to_string(), 20.0, 20000.0, self.frequency))
        } else {
            None
        }
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if index == 0 {
            self.frequency = value.clamp(20.0, 20000.0);
        }
    }
}

// ---------------------------------------------------------------------------
// Filter Node
// ---------------------------------------------------------------------------

/// SVF filter node for the modular graph.
#[derive(Debug)]
pub struct FilterNode {
    filter: crate::synth::filter::StateVariableFilter,
    use_lp: bool,
    inputs: Vec<PortDescriptor>,
    outputs: Vec<PortDescriptor>,
}

impl FilterNode {
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        Self {
            filter: crate::synth::filter::StateVariableFilter::new(sample_rate),
            use_lp: true,
            inputs: vec![
                PortDescriptor {
                    name: "In".to_string(),
                    port_type: PortType::Audio,
                },
                PortDescriptor {
                    name: "Cutoff CV".to_string(),
                    port_type: PortType::Control,
                },
            ],
            outputs: vec![PortDescriptor {
                name: "Out".to_string(),
                port_type: PortType::Audio,
            }],
        }
    }
}

impl ModularNode for FilterNode {
    fn name(&self) -> &'static str {
        "VCF"
    }

    fn inputs(&self) -> &[PortDescriptor] {
        &self.inputs
    }

    fn outputs(&self) -> &[PortDescriptor] {
        &self.outputs
    }

    fn process(&mut self, input_buffers: &[&[f32]], output_buffers: &mut [&mut [f32]]) {
        let audio_in = input_buffers.first().map_or(&[] as &[f32], |b| *b);
        let Some(output) = output_buffers.first_mut() else {
            return;
        };

        for (i, sample) in output.iter_mut().enumerate() {
            let input = if i < audio_in.len() { audio_in[i] } else { 0.0 };
            let (hp, lp) = self.filter.tick(input);
            *sample = sanitize_sample(if self.use_lp { lp } else { hp });
        }
    }

    fn reset(&mut self) {
        self.filter.reset();
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.filter.set_sample_rate(sample_rate);
    }

    fn param_count(&self) -> usize {
        2
    }

    fn param_info(&self, index: usize) -> Option<(String, f32, f32, f32)> {
        match index {
            0 => Some(("Cutoff".to_string(), 20.0, 20000.0, 1000.0)),
            1 => Some(("Resonance".to_string(), 0.0, 0.95, 0.0)),
            _ => None,
        }
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.filter.set_cutoff(value),
            1 => self.filter.set_resonance(value),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// VCA (Amplifier) Node
// ---------------------------------------------------------------------------

/// Simple VCA node — multiplies audio by control signal.
#[derive(Debug)]
pub struct VcaNode {
    gain: f32,
    inputs: Vec<PortDescriptor>,
    outputs: Vec<PortDescriptor>,
}

impl Default for VcaNode {
    fn default() -> Self {
        Self::new()
    }
}

impl VcaNode {
    #[must_use]
    pub fn new() -> Self {
        Self {
            gain: 1.0,
            inputs: vec![
                PortDescriptor {
                    name: "In".to_string(),
                    port_type: PortType::Audio,
                },
                PortDescriptor {
                    name: "CV".to_string(),
                    port_type: PortType::Control,
                },
            ],
            outputs: vec![PortDescriptor {
                name: "Out".to_string(),
                port_type: PortType::Audio,
            }],
        }
    }
}

impl ModularNode for VcaNode {
    fn name(&self) -> &'static str {
        "VCA"
    }

    fn inputs(&self) -> &[PortDescriptor] {
        &self.inputs
    }

    fn outputs(&self) -> &[PortDescriptor] {
        &self.outputs
    }

    fn process(&mut self, input_buffers: &[&[f32]], output_buffers: &mut [&mut [f32]]) {
        let audio_in = input_buffers.first().map_or(&[] as &[f32], |b| *b);
        let cv_in = input_buffers.get(1).map_or(&[] as &[f32], |b| *b);
        let Some(output) = output_buffers.first_mut() else {
            return;
        };

        for (i, sample) in output.iter_mut().enumerate() {
            let audio = if i < audio_in.len() { audio_in[i] } else { 0.0 };
            let cv = if i < cv_in.len() {
                cv_in[i].clamp(0.0, 1.0)
            } else {
                1.0
            };
            *sample = sanitize_sample(audio * cv * self.gain);
        }
    }

    fn reset(&mut self) {}

    fn set_sample_rate(&mut self, _sample_rate: f32) {}

    fn param_count(&self) -> usize {
        1
    }

    fn param_info(&self, index: usize) -> Option<(String, f32, f32, f32)> {
        if index == 0 {
            Some(("Gain".to_string(), 0.0, 2.0, self.gain))
        } else {
            None
        }
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if index == 0 {
            self.gain = value.clamp(0.0, 2.0);
        }
    }
}

// ---------------------------------------------------------------------------
// Noise Generator Node
// ---------------------------------------------------------------------------

/// White noise generator node.
#[derive(Debug)]
pub struct NoiseNode {
    /// Simple LCG state for deterministic noise (no allocation).
    state: u32,
    outputs: Vec<PortDescriptor>,
}

impl Default for NoiseNode {
    fn default() -> Self {
        Self::new()
    }
}

impl NoiseNode {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: 0x1234_5678,
            outputs: vec![PortDescriptor {
                name: "Out".to_string(),
                port_type: PortType::Audio,
            }],
        }
    }
}

impl ModularNode for NoiseNode {
    fn name(&self) -> &'static str {
        "Noise"
    }

    fn inputs(&self) -> &[PortDescriptor] {
        &[]
    }

    fn outputs(&self) -> &[PortDescriptor] {
        &self.outputs
    }

    fn process(&mut self, _input_buffers: &[&[f32]], output_buffers: &mut [&mut [f32]]) {
        let Some(output) = output_buffers.first_mut() else {
            return;
        };

        for sample in output.iter_mut() {
            // Simple LCG for white noise
            self.state = self
                .state
                .wrapping_mul(1_664_525)
                .wrapping_add(1_013_904_223);
            // Map to [-1, 1]
            *sample = (self.state as f32 / u32::MAX as f32).mul_add(2.0, -1.0);
        }
    }

    fn reset(&mut self) {
        self.state = 0x1234_5678;
    }

    fn set_sample_rate(&mut self, _sample_rate: f32) {}
}

// ---------------------------------------------------------------------------
// Mixer Node
// ---------------------------------------------------------------------------

/// Simple 2-input mixer node.
#[derive(Debug)]
pub struct MixerNode {
    mix: f32,
    inputs: Vec<PortDescriptor>,
    outputs: Vec<PortDescriptor>,
}

impl Default for MixerNode {
    fn default() -> Self {
        Self::new()
    }
}

impl MixerNode {
    #[must_use]
    pub fn new() -> Self {
        Self {
            mix: 0.5,
            inputs: vec![
                PortDescriptor {
                    name: "A".to_string(),
                    port_type: PortType::Audio,
                },
                PortDescriptor {
                    name: "B".to_string(),
                    port_type: PortType::Audio,
                },
            ],
            outputs: vec![PortDescriptor {
                name: "Out".to_string(),
                port_type: PortType::Audio,
            }],
        }
    }
}

impl ModularNode for MixerNode {
    fn name(&self) -> &'static str {
        "Mixer"
    }

    fn inputs(&self) -> &[PortDescriptor] {
        &self.inputs
    }

    fn outputs(&self) -> &[PortDescriptor] {
        &self.outputs
    }

    fn process(&mut self, input_buffers: &[&[f32]], output_buffers: &mut [&mut [f32]]) {
        let a = input_buffers.first().map_or(&[] as &[f32], |b| *b);
        let b = input_buffers.get(1).map_or(&[] as &[f32], |b| *b);
        let Some(output) = output_buffers.first_mut() else {
            return;
        };

        for (i, sample) in output.iter_mut().enumerate() {
            let va = if i < a.len() { a[i] } else { 0.0 };
            let vb = if i < b.len() { b[i] } else { 0.0 };
            *sample = sanitize_sample(va * (1.0 - self.mix) + vb * self.mix);
        }
    }

    fn reset(&mut self) {}

    fn set_sample_rate(&mut self, _sample_rate: f32) {}

    fn param_count(&self) -> usize {
        1
    }

    fn param_info(&self, index: usize) -> Option<(String, f32, f32, f32)> {
        if index == 0 {
            Some(("Mix".to_string(), 0.0, 1.0, self.mix))
        } else {
            None
        }
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if index == 0 {
            self.mix = value.clamp(0.0, 1.0);
        }
    }
}

// ---------------------------------------------------------------------------
// LFO Node
// ---------------------------------------------------------------------------

/// LFO waveform shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LfoWaveform {
    Sine,
    Triangle,
    Saw,
    Square,
}

impl LfoWaveform {
    const ALL: [Self; 4] = [Self::Sine, Self::Triangle, Self::Saw, Self::Square];

    /// Map a parameter index (0-3) to a waveform.
    fn from_index(i: usize) -> Self {
        Self::ALL[i.min(Self::ALL.len() - 1)]
    }
}

/// Low-frequency oscillator for modulation.
///
/// Generates control-rate waveforms (sine, triangle, saw, square) for
/// modulating other node parameters. Rate is in Hz (0.01–100). Output
/// is bipolar (−1 to +1) scaled by depth.
///
/// Inputs:
/// - Rate CV (control): additive offset in Hz to the base rate.
/// - Sync (trigger): resets phase on rising edge.
///
/// Outputs:
/// - Out (control): the LFO waveform, range [−depth, +depth].
#[derive(Debug)]
pub struct LfoNode {
    phase: f32,
    rate: f32,
    depth: f32,
    waveform: LfoWaveform,
    sample_rate: f32,
    /// Previous sync input for edge detection.
    last_sync: f32,
    inputs: Vec<PortDescriptor>,
    outputs: Vec<PortDescriptor>,
}

impl LfoNode {
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        Self {
            phase: 0.0,
            rate: 1.0,
            depth: 1.0,
            waveform: LfoWaveform::Sine,
            sample_rate: sample_rate.max(1.0),
            last_sync: 0.0,
            inputs: vec![
                PortDescriptor {
                    name: "Rate CV".to_string(),
                    port_type: PortType::Control,
                },
                PortDescriptor {
                    name: "Sync".to_string(),
                    port_type: PortType::Trigger,
                },
            ],
            outputs: vec![PortDescriptor {
                name: "Out".to_string(),
                port_type: PortType::Control,
            }],
        }
    }

    /// Generate one sample of the LFO waveform at the current phase.
    fn generate(&self) -> f32 {
        match self.waveform {
            LfoWaveform::Sine => (self.phase * TAU).sin(),
            LfoWaveform::Triangle => {
                // Phase 0→0.25: rise 0→1, 0.25→0.75: fall 1→−1, 0.75→1: rise −1→0
                let p = self.phase;
                if p < 0.25 {
                    p * 4.0
                } else if p < 0.75 {
                    (0.5 - p).mul_add(4.0, 0.0)
                } else {
                    (p - 1.0) * 4.0
                }
            }
            LfoWaveform::Saw => self.phase.mul_add(2.0, -1.0),
            LfoWaveform::Square => {
                if self.phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
        }
    }
}

impl ModularNode for LfoNode {
    fn name(&self) -> &'static str {
        "LFO"
    }

    fn inputs(&self) -> &[PortDescriptor] {
        &self.inputs
    }

    fn outputs(&self) -> &[PortDescriptor] {
        &self.outputs
    }

    fn process(&mut self, input_buffers: &[&[f32]], output_buffers: &mut [&mut [f32]]) {
        let rate_cv = input_buffers.first().map_or(&[] as &[f32], |b| *b);
        let sync_in = input_buffers.get(1).map_or(&[] as &[f32], |b| *b);
        let Some(output) = output_buffers.first_mut() else {
            return;
        };

        for (i, sample) in output.iter_mut().enumerate() {
            // Sync: reset phase on rising edge.
            let sync_val = if i < sync_in.len() { sync_in[i] } else { 0.0 };
            if sync_val > 0.5 && self.last_sync <= 0.5 {
                self.phase = 0.0;
            }
            self.last_sync = sync_val;

            // Effective rate: base + CV modulation.
            let rate_mod = if i < rate_cv.len() { rate_cv[i] } else { 0.0 };
            let effective_rate = (self.rate + rate_mod * 10.0).clamp(0.01, 100.0);

            *sample = sanitize_sample(self.generate() * self.depth);

            self.phase += effective_rate / self.sample_rate;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
            }
            if !self.phase.is_finite() {
                self.phase = 0.0;
            }
        }
    }

    fn reset(&mut self) {
        self.phase = 0.0;
        self.last_sync = 0.0;
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
    }

    fn param_count(&self) -> usize {
        3
    }

    fn param_info(&self, index: usize) -> Option<(String, f32, f32, f32)> {
        match index {
            0 => Some(("Rate".to_string(), 0.01, 100.0, self.rate)),
            1 => Some(("Depth".to_string(), 0.0, 1.0, self.depth)),
            2 => Some((
                "Waveform".to_string(),
                0.0,
                3.0,
                LfoWaveform::ALL
                    .iter()
                    .position(|&w| w == self.waveform)
                    .unwrap_or(0) as f32,
            )),
            _ => None,
        }
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.rate = value.clamp(0.01, 100.0),
            1 => self.depth = value.clamp(0.0, 1.0),
            #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
            2 => self.waveform = LfoWaveform::from_index(value.round() as usize),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// ADSR Envelope Node
// ---------------------------------------------------------------------------

/// ADSR envelope generator stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdsrStage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// ADSR envelope generator node.
///
/// Standard attack-decay-sustain-release envelope driven by a gate input.
/// Rising edge on gate = note on (attack), falling edge = note off (release).
/// Output is a control signal (0.0 to 1.0).
///
/// Inputs:
/// - Gate (trigger): gate signal, >0.5 = gate on, ≤0.5 = gate off.
///
/// Outputs:
/// - Out (control): envelope value 0.0–1.0.
#[derive(Debug)]
pub struct AdsrNode {
    stage: AdsrStage,
    value: f32,
    attack: f32,
    decay: f32,
    sustain: f32,
    release: f32,
    /// Coefficients (recomputed when params or sample rate change).
    attack_coeff: f32,
    decay_coeff: f32,
    release_coeff: f32,
    sample_rate: f32,
    /// Previous gate for edge detection.
    last_gate: bool,
    inputs: Vec<PortDescriptor>,
    outputs: Vec<PortDescriptor>,
}

/// Overshoot target for exponential attack (reaches 1.0 at specified time).
const ADSR_ATTACK_TARGET: f32 = 1.37;

/// Compute exponential coefficient for a given time constant.
fn adsr_exp_coeff(time_secs: f32, sample_rate: f32) -> f32 {
    if time_secs <= 0.0 || sample_rate <= 0.0 {
        return 0.0;
    }
    (-1.0 / (time_secs * sample_rate)).exp()
}

impl AdsrNode {
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(1.0);
        let mut node = Self {
            stage: AdsrStage::Idle,
            value: 0.0,
            attack: 0.01,
            decay: 0.2,
            sustain: 0.6,
            release: 0.3,
            attack_coeff: 0.0,
            decay_coeff: 0.0,
            release_coeff: 0.0,
            sample_rate: sr,
            last_gate: false,
            inputs: vec![PortDescriptor {
                name: "Gate".to_string(),
                port_type: PortType::Trigger,
            }],
            outputs: vec![PortDescriptor {
                name: "Out".to_string(),
                port_type: PortType::Control,
            }],
        };
        node.recompute_coefficients();
        node
    }

    fn recompute_coefficients(&mut self) {
        self.attack_coeff = adsr_exp_coeff(self.attack, self.sample_rate);
        self.decay_coeff = adsr_exp_coeff(self.decay, self.sample_rate);
        self.release_coeff = adsr_exp_coeff(self.release, self.sample_rate);
    }

    /// Advance the envelope by one sample.
    fn tick(&mut self) -> f32 {
        match self.stage {
            AdsrStage::Idle => {
                self.value = 0.0;
            }
            AdsrStage::Attack => {
                self.value = self
                    .attack_coeff
                    .mul_add(self.value - ADSR_ATTACK_TARGET, ADSR_ATTACK_TARGET);
                if self.value >= 1.0 {
                    self.value = 1.0;
                    self.stage = AdsrStage::Decay;
                }
            }
            AdsrStage::Decay => {
                let target = self.sustain;
                self.value = self.decay_coeff.mul_add(self.value - target, target);
                if (self.value - target).abs() < 1e-5 {
                    self.value = target;
                    self.stage = AdsrStage::Sustain;
                }
            }
            AdsrStage::Sustain => {
                self.value = self.sustain;
            }
            AdsrStage::Release => {
                self.value *= self.release_coeff;
                if self.value < 1e-5 {
                    self.value = 0.0;
                    self.stage = AdsrStage::Idle;
                }
            }
        }
        self.value.clamp(0.0, 1.0)
    }
}

impl ModularNode for AdsrNode {
    fn name(&self) -> &'static str {
        "ADSR"
    }

    fn inputs(&self) -> &[PortDescriptor] {
        &self.inputs
    }

    fn outputs(&self) -> &[PortDescriptor] {
        &self.outputs
    }

    fn process(&mut self, input_buffers: &[&[f32]], output_buffers: &mut [&mut [f32]]) {
        let gate_buf = input_buffers.first().map_or(&[] as &[f32], |b| *b);
        let Some(output) = output_buffers.first_mut() else {
            return;
        };

        for (i, sample) in output.iter_mut().enumerate() {
            let gate_val = if i < gate_buf.len() { gate_buf[i] } else { 0.0 };
            let gate_active = gate_val > 0.5;

            // Edge detection.
            if gate_active && !self.last_gate {
                // Rising edge: start attack.
                self.stage = AdsrStage::Attack;
            } else if !gate_active && self.last_gate {
                // Falling edge: start release.
                if self.stage != AdsrStage::Idle {
                    self.stage = AdsrStage::Release;
                }
            }
            self.last_gate = gate_active;

            *sample = sanitize_sample(self.tick());
        }
    }

    fn reset(&mut self) {
        self.stage = AdsrStage::Idle;
        self.value = 0.0;
        self.last_gate = false;
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        self.recompute_coefficients();
    }

    fn param_count(&self) -> usize {
        4
    }

    fn param_info(&self, index: usize) -> Option<(String, f32, f32, f32)> {
        match index {
            0 => Some(("Attack".to_string(), 0.001, 10.0, self.attack)),
            1 => Some(("Decay".to_string(), 0.001, 10.0, self.decay)),
            2 => Some(("Sustain".to_string(), 0.0, 1.0, self.sustain)),
            3 => Some(("Release".to_string(), 0.001, 10.0, self.release)),
            _ => None,
        }
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.attack = value.clamp(0.001, 10.0),
            1 => self.decay = value.clamp(0.001, 10.0),
            2 => self.sustain = value.clamp(0.0, 1.0),
            3 => self.release = value.clamp(0.001, 10.0),
            _ => return,
        }
        self.recompute_coefficients();
    }
}

// ---------------------------------------------------------------------------
// Clock Node
// ---------------------------------------------------------------------------

/// Trigger clock generator.
///
/// Generates periodic trigger pulses at a configurable BPM. Each pulse is
/// a short burst of 1.0 (configurable gate length as fraction of the beat).
/// Use to drive ADSR envelopes, sequencers, or sample-and-hold modules.
///
/// Outputs:
/// - Trigger (trigger): 1.0 on the sample where phase wraps, 0.0 otherwise.
/// - Gate (trigger): 1.0 while phase < `gate_length` fraction of each beat.
#[derive(Debug)]
pub struct ClockNode {
    /// Beats per minute.
    bpm: f32,
    /// Gate length as fraction of the beat (0.01–0.99).
    gate_length: f32,
    /// Phase accumulator (0.0–1.0 per beat).
    phase: f32,
    sample_rate: f32,
    outputs: Vec<PortDescriptor>,
}

impl ClockNode {
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        Self {
            bpm: 120.0,
            gate_length: 0.5,
            phase: 0.0,
            sample_rate: sample_rate.max(1.0),
            outputs: vec![
                PortDescriptor {
                    name: "Trigger".to_string(),
                    port_type: PortType::Trigger,
                },
                PortDescriptor {
                    name: "Gate".to_string(),
                    port_type: PortType::Trigger,
                },
            ],
        }
    }
}

impl ModularNode for ClockNode {
    fn name(&self) -> &'static str {
        "Clock"
    }

    fn inputs(&self) -> &[PortDescriptor] {
        &[]
    }

    fn outputs(&self) -> &[PortDescriptor] {
        &self.outputs
    }

    fn process(&mut self, _input_buffers: &[&[f32]], output_buffers: &mut [&mut [f32]]) {
        let beats_per_sec = self.bpm / 60.0;
        let phase_inc = beats_per_sec / self.sample_rate;

        // We need both outputs simultaneously. Handle borrow checker by
        // splitting the slice.
        let len = output_buffers.first().map_or(0, |b| b.len());
        if len == 0 {
            return;
        }

        for i in 0..len {
            let prev_phase = self.phase;
            self.phase += phase_inc;
            let wrapped = self.phase >= 1.0;
            if wrapped {
                self.phase -= 1.0;
            }

            if !self.phase.is_finite() {
                self.phase = 0.0;
            }

            // Trigger: 1.0 on the sample where phase wraps.
            if let Some(trigger) = output_buffers.first_mut() {
                if i < trigger.len() {
                    trigger[i] = if wrapped || (prev_phase == 0.0 && i == 0) {
                        1.0
                    } else {
                        0.0
                    };
                }
            }

            // Gate: 1.0 while phase < gate_length.
            if let Some(gate) = output_buffers.get_mut(1) {
                if i < gate.len() {
                    gate[i] = if self.phase < self.gate_length {
                        1.0
                    } else {
                        0.0
                    };
                }
            }
        }
    }

    fn reset(&mut self) {
        self.phase = 0.0;
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
    }

    fn param_count(&self) -> usize {
        2
    }

    fn param_info(&self, index: usize) -> Option<(String, f32, f32, f32)> {
        match index {
            0 => Some(("BPM".to_string(), 20.0, 300.0, self.bpm)),
            1 => Some(("Gate Len".to_string(), 0.01, 0.99, self.gate_length)),
            _ => None,
        }
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.bpm = value.clamp(20.0, 300.0),
            1 => self.gate_length = value.clamp(0.01, 0.99),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Sample & Hold Node
// ---------------------------------------------------------------------------

/// Sample and hold: captures input value on trigger rising edge, holds until
/// next trigger.
///
/// Classic random-stepped modulation when driven by noise + clock.
/// Also useful for latching pitch CV from a sequencer.
///
/// Inputs:
/// - In (control): signal to sample.
/// - Trigger (trigger): rising edge captures current input value.
///
/// Outputs:
/// - Out (control): held value.
#[derive(Debug)]
pub struct SampleHoldNode {
    held_value: f32,
    last_trigger: f32,
    inputs: Vec<PortDescriptor>,
    outputs: Vec<PortDescriptor>,
}

impl Default for SampleHoldNode {
    fn default() -> Self {
        Self::new()
    }
}

impl SampleHoldNode {
    #[must_use]
    pub fn new() -> Self {
        Self {
            held_value: 0.0,
            last_trigger: 0.0,
            inputs: vec![
                PortDescriptor {
                    name: "In".to_string(),
                    port_type: PortType::Control,
                },
                PortDescriptor {
                    name: "Trigger".to_string(),
                    port_type: PortType::Trigger,
                },
            ],
            outputs: vec![PortDescriptor {
                name: "Out".to_string(),
                port_type: PortType::Control,
            }],
        }
    }
}

impl ModularNode for SampleHoldNode {
    fn name(&self) -> &'static str {
        "S&H"
    }

    fn inputs(&self) -> &[PortDescriptor] {
        &self.inputs
    }

    fn outputs(&self) -> &[PortDescriptor] {
        &self.outputs
    }

    fn process(&mut self, input_buffers: &[&[f32]], output_buffers: &mut [&mut [f32]]) {
        let signal_in = input_buffers.first().map_or(&[] as &[f32], |b| *b);
        let trigger_in = input_buffers.get(1).map_or(&[] as &[f32], |b| *b);
        let Some(output) = output_buffers.first_mut() else {
            return;
        };

        for (i, sample) in output.iter_mut().enumerate() {
            let trig = if i < trigger_in.len() {
                trigger_in[i]
            } else {
                0.0
            };

            // Rising edge detection.
            if trig > 0.5 && self.last_trigger <= 0.5 {
                let input_val = if i < signal_in.len() {
                    signal_in[i]
                } else {
                    0.0
                };
                self.held_value = sanitize_sample(input_val);
            }
            self.last_trigger = trig;

            *sample = self.held_value;
        }
    }

    fn reset(&mut self) {
        self.held_value = 0.0;
        self.last_trigger = 0.0;
    }

    fn set_sample_rate(&mut self, _sample_rate: f32) {}
}

// ---------------------------------------------------------------------------
// Quantizer Node
// ---------------------------------------------------------------------------

/// Musical scale for the quantizer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    Chromatic,
    Major,
    NaturalMinor,
    PentatonicMajor,
    WholeTone,
}

impl Scale {
    const ALL: [Self; 5] = [
        Self::Chromatic,
        Self::Major,
        Self::NaturalMinor,
        Self::PentatonicMajor,
        Self::WholeTone,
    ];

    /// Semitone degrees within one octave for this scale.
    #[must_use]
    const fn degrees(self) -> &'static [u8] {
        match self {
            Self::Chromatic => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
            Self::Major => &[0, 2, 4, 5, 7, 9, 11],
            Self::NaturalMinor => &[0, 2, 3, 5, 7, 8, 10],
            Self::PentatonicMajor => &[0, 2, 4, 7, 9],
            Self::WholeTone => &[0, 2, 4, 6, 8, 10],
        }
    }

    fn from_index(i: usize) -> Self {
        Self::ALL[i.min(Self::ALL.len() - 1)]
    }
}

/// Pitch quantizer: snaps a control voltage to the nearest note in a scale.
///
/// Uses 1V/octave convention: 1.0 = one octave. Each semitone = 1/12.
/// Snaps input to the nearest scale degree.
///
/// Inputs:
/// - In (control): unquantized pitch CV.
///
/// Outputs:
/// - Out (control): quantized pitch CV.
#[derive(Debug)]
pub struct QuantizerNode {
    scale: Scale,
    inputs: Vec<PortDescriptor>,
    outputs: Vec<PortDescriptor>,
}

impl Default for QuantizerNode {
    fn default() -> Self {
        Self::new()
    }
}

impl QuantizerNode {
    #[must_use]
    pub fn new() -> Self {
        Self {
            scale: Scale::Major,
            inputs: vec![PortDescriptor {
                name: "In".to_string(),
                port_type: PortType::Control,
            }],
            outputs: vec![PortDescriptor {
                name: "Out".to_string(),
                port_type: PortType::Control,
            }],
        }
    }

    /// Quantize a single value (in octave units) to the nearest scale degree.
    fn quantize(&self, value: f32) -> f32 {
        if !value.is_finite() {
            return 0.0;
        }

        // Convert to semitones.
        let semitones = value * 12.0;

        // Separate into octave and fractional semitone within octave.
        let octave = semitones.floor() as i32 / 12;
        let mut semi_in_octave = semitones - (octave * 12) as f32;
        if semi_in_octave < 0.0 {
            semi_in_octave += 12.0;
        }

        // Find nearest scale degree.
        let degrees = self.scale.degrees();
        let mut best = f32::from(degrees[0]);
        let mut best_dist = f32::MAX;

        for &deg in degrees {
            let dist = (semi_in_octave - f32::from(deg)).abs();
            // Also check wrapping (e.g., 11.9 is close to 0 of next octave).
            let dist_wrap = (12.0 - dist).abs().min(dist);
            if dist_wrap < best_dist {
                best_dist = dist_wrap;
                best = f32::from(deg);
            }
        }

        // Check if the note wraps to the next octave's root.
        let dist_to_next_root = 12.0 - semi_in_octave;
        if dist_to_next_root < best_dist && degrees.contains(&0) {
            // Snap to next octave root.
            (octave + 1) as f32
        } else {
            // Reconstruct: octave * 12 + snapped degree, convert back to octave units.
            (octave as f32).mul_add(12.0, best) / 12.0
        }
    }
}

impl ModularNode for QuantizerNode {
    fn name(&self) -> &'static str {
        "Quant"
    }

    fn inputs(&self) -> &[PortDescriptor] {
        &self.inputs
    }

    fn outputs(&self) -> &[PortDescriptor] {
        &self.outputs
    }

    fn process(&mut self, input_buffers: &[&[f32]], output_buffers: &mut [&mut [f32]]) {
        let signal_in = input_buffers.first().map_or(&[] as &[f32], |b| *b);
        let Some(output) = output_buffers.first_mut() else {
            return;
        };

        for (i, sample) in output.iter_mut().enumerate() {
            let input_val = if i < signal_in.len() {
                signal_in[i]
            } else {
                0.0
            };
            *sample = self.quantize(input_val);
        }
    }

    fn reset(&mut self) {}

    fn set_sample_rate(&mut self, _sample_rate: f32) {}

    fn param_count(&self) -> usize {
        1
    }

    fn param_info(&self, index: usize) -> Option<(String, f32, f32, f32)> {
        if index == 0 {
            Some((
                "Scale".to_string(),
                0.0,
                4.0,
                Scale::ALL
                    .iter()
                    .position(|&s| s == self.scale)
                    .unwrap_or(0) as f32,
            ))
        } else {
            None
        }
    }

    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    fn set_param(&mut self, index: usize, value: f32) {
        if index == 0 {
            self.scale = Scale::from_index(value.round() as usize);
        }
    }
}

// ---------------------------------------------------------------------------
// Delay Node
// ---------------------------------------------------------------------------

/// Maximum delay time in seconds.
const MAX_DELAY_SECS: f32 = 2.0;

/// Audio delay with feedback.
///
/// Pre-allocates a circular buffer for up to 2 seconds of delay. Feedback
/// controls how much of the output is fed back into the delay line.
///
/// Inputs:
/// - In (audio): audio signal to delay.
/// - Feedback CV (control): modulates feedback amount (additive).
///
/// Outputs:
/// - Out (audio): delayed + dry mixed signal.
#[derive(Debug)]
pub struct DelayNode {
    buffer: Vec<f32>,
    write_pos: usize,
    delay_time: f32,
    feedback: f32,
    dry_wet: f32,
    sample_rate: f32,
    inputs: Vec<PortDescriptor>,
    outputs: Vec<PortDescriptor>,
}

impl DelayNode {
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(1.0);
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let buf_size = (sr * MAX_DELAY_SECS) as usize + 1;
        Self {
            buffer: vec![0.0; buf_size],
            write_pos: 0,
            delay_time: 0.3,
            feedback: 0.4,
            dry_wet: 0.5,
            sample_rate: sr,
            inputs: vec![
                PortDescriptor {
                    name: "In".to_string(),
                    port_type: PortType::Audio,
                },
                PortDescriptor {
                    name: "FB CV".to_string(),
                    port_type: PortType::Control,
                },
            ],
            outputs: vec![PortDescriptor {
                name: "Out".to_string(),
                port_type: PortType::Audio,
            }],
        }
    }

    /// Read from the delay line at the given delay in samples (fractional,
    /// linear interpolation).
    fn read_delayed(&self, delay_samples: f32) -> f32 {
        let buf_len = self.buffer.len();
        if buf_len == 0 {
            return 0.0;
        }

        let delay_int = delay_samples as usize;
        let frac = delay_samples - delay_int as f32;

        // Two read positions for linear interpolation.
        let pos_a = (self.write_pos + buf_len - delay_int) % buf_len;
        let pos_b = (self.write_pos + buf_len - delay_int - 1) % buf_len;

        let a = self.buffer[pos_a];
        let b = self.buffer[pos_b];

        // Linear interpolation.
        frac.mul_add(b - a, a)
    }
}

impl ModularNode for DelayNode {
    fn name(&self) -> &'static str {
        "Delay"
    }

    fn inputs(&self) -> &[PortDescriptor] {
        &self.inputs
    }

    fn outputs(&self) -> &[PortDescriptor] {
        &self.outputs
    }

    fn process(&mut self, input_buffers: &[&[f32]], output_buffers: &mut [&mut [f32]]) {
        let audio_in = input_buffers.first().map_or(&[] as &[f32], |b| *b);
        let fb_cv = input_buffers.get(1).map_or(&[] as &[f32], |b| *b);
        let Some(output) = output_buffers.first_mut() else {
            return;
        };

        let delay_samples = (self.delay_time * self.sample_rate).max(1.0);
        let buf_len = self.buffer.len();

        for (i, sample) in output.iter_mut().enumerate() {
            let dry = if i < audio_in.len() { audio_in[i] } else { 0.0 };
            let fb_mod = if i < fb_cv.len() { fb_cv[i] * 0.2 } else { 0.0 };
            let effective_fb = (self.feedback + fb_mod).clamp(0.0, 0.95);

            // Read from delay line.
            let delayed = self.read_delayed(delay_samples);

            // Write input + feedback to delay line.
            if buf_len > 0 {
                self.buffer[self.write_pos] = sanitize_sample(dry + delayed * effective_fb);
                self.write_pos = (self.write_pos + 1) % buf_len;
            }

            // Mix dry/wet.
            *sample = sanitize_sample(dry * (1.0 - self.dry_wet) + delayed * self.dry_wet);
        }
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.write_pos = 0;
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        let sr = sample_rate.max(1.0);
        self.sample_rate = sr;
        // Reallocate buffer for new sample rate.
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let buf_size = (sr * MAX_DELAY_SECS) as usize + 1;
        self.buffer = vec![0.0; buf_size];
        self.write_pos = 0;
    }

    fn param_count(&self) -> usize {
        3
    }

    fn param_info(&self, index: usize) -> Option<(String, f32, f32, f32)> {
        match index {
            0 => Some(("Time".to_string(), 0.001, MAX_DELAY_SECS, self.delay_time)),
            1 => Some(("Feedback".to_string(), 0.0, 0.95, self.feedback)),
            2 => Some(("Dry/Wet".to_string(), 0.0, 1.0, self.dry_wet)),
            _ => None,
        }
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.delay_time = value.clamp(0.001, MAX_DELAY_SECS),
            1 => self.feedback = value.clamp(0.0, 0.95),
            2 => self.dry_wet = value.clamp(0.0, 1.0),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Slew Limiter Node
// ---------------------------------------------------------------------------

/// Slew limiter: smooths stepped control signals with separate rise/fall rates.
///
/// The modular equivalent of portamento/glide, applicable to any control signal.
/// Rise rate controls how fast the output can increase; fall rate controls how
/// fast it can decrease. With matched rates, this is a simple lowpass on the
/// control signal.
///
/// Inputs:
/// - In (control): signal to smooth.
///
/// Outputs:
/// - Out (control): slew-limited signal.
#[derive(Debug)]
pub struct SlewNode {
    current: f32,
    rise_rate: f32,
    fall_rate: f32,
    sample_rate: f32,
    inputs: Vec<PortDescriptor>,
    outputs: Vec<PortDescriptor>,
}

impl SlewNode {
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        Self {
            current: 0.0,
            rise_rate: 0.1,
            fall_rate: 0.1,
            sample_rate: sample_rate.max(1.0),
            inputs: vec![PortDescriptor {
                name: "In".to_string(),
                port_type: PortType::Control,
            }],
            outputs: vec![PortDescriptor {
                name: "Out".to_string(),
                port_type: PortType::Control,
            }],
        }
    }
}

impl ModularNode for SlewNode {
    fn name(&self) -> &'static str {
        "Slew"
    }

    fn inputs(&self) -> &[PortDescriptor] {
        &self.inputs
    }

    fn outputs(&self) -> &[PortDescriptor] {
        &self.outputs
    }

    fn process(&mut self, input_buffers: &[&[f32]], output_buffers: &mut [&mut [f32]]) {
        let signal_in = input_buffers.first().map_or(&[] as &[f32], |b| *b);
        let Some(output) = output_buffers.first_mut() else {
            return;
        };

        for (i, sample) in output.iter_mut().enumerate() {
            let target = if i < signal_in.len() {
                signal_in[i]
            } else {
                0.0
            };

            let diff = target - self.current;
            if diff.abs() < 1e-6 {
                self.current = target;
            } else if diff > 0.0 {
                // Rising: apply rise rate.
                let max_step = self.rise_rate / self.sample_rate;
                self.current += diff.min(max_step);
            } else {
                // Falling: apply fall rate.
                let max_step = self.fall_rate / self.sample_rate;
                self.current += diff.max(-max_step);
            }

            *sample = sanitize_sample(self.current);
        }
    }

    fn reset(&mut self) {
        self.current = 0.0;
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
    }

    fn param_count(&self) -> usize {
        2
    }

    fn param_info(&self, index: usize) -> Option<(String, f32, f32, f32)> {
        match index {
            0 => Some(("Rise".to_string(), 0.01, 100.0, self.rise_rate)),
            1 => Some(("Fall".to_string(), 0.01, 100.0, self.fall_rate)),
            _ => None,
        }
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.rise_rate = value.clamp(0.01, 100.0),
            1 => self.fall_rate = value.clamp(0.01, 100.0),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc_node_produces_output() {
        let mut osc = OscNode::new(44100.0);
        let input: Vec<f32> = vec![0.0; 128];
        let mut output = vec![0.0; 128];

        let inputs = [input.as_slice()];
        let mut outputs = [output.as_mut_slice()];
        osc.process(&inputs, &mut outputs);

        let has_signal = outputs[0].iter().any(|&s| s.abs() > 0.001);
        assert!(has_signal, "osc node should produce output");
    }

    #[test]
    fn noise_node_produces_output() {
        let mut noise = NoiseNode::new();
        let mut output = vec![0.0; 128];

        let inputs: Vec<&[f32]> = vec![];
        let mut outputs = [output.as_mut_slice()];
        noise.process(&inputs, &mut outputs);

        let has_signal = outputs[0].iter().any(|&s| s.abs() > 0.001);
        assert!(has_signal, "noise node should produce output");
    }

    #[test]
    fn vca_node_attenuates() {
        let mut vca = VcaNode::new();
        let audio = vec![1.0_f32; 128];
        let cv = vec![0.5_f32; 128];
        let mut output = vec![0.0; 128];

        let inputs = [audio.as_slice(), cv.as_slice()];
        let mut outputs = [output.as_mut_slice()];
        vca.process(&inputs, &mut outputs);

        for &s in &*outputs[0] {
            assert!(
                (s - 0.5).abs() < 0.01,
                "VCA with 0.5 CV should output ~0.5, got {s}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // LFO Node tests
    // -----------------------------------------------------------------------

    #[test]
    fn lfo_produces_output() {
        let mut lfo = LfoNode::new(44100.0);
        let rate_cv = vec![0.0; 128];
        let sync = vec![0.0; 128];
        let mut output = vec![0.0; 128];

        let inputs = [rate_cv.as_slice(), sync.as_slice()];
        let mut outputs = [output.as_mut_slice()];
        lfo.process(&inputs, &mut outputs);

        let has_signal = outputs[0].iter().any(|&s| s.abs() > 0.001);
        assert!(has_signal, "LFO should produce output");
    }

    #[test]
    fn lfo_all_waveforms_produce_output() {
        for (i, wf) in LfoWaveform::ALL.iter().enumerate() {
            let mut lfo = LfoNode::new(44100.0);
            lfo.waveform = *wf;
            let rate_cv = vec![0.0; 44100]; // 1 second
            let sync = vec![0.0; 44100];
            let mut output = vec![0.0; 44100];

            let inputs = [rate_cv.as_slice(), sync.as_slice()];
            let mut outputs = [output.as_mut_slice()];
            lfo.process(&inputs, &mut outputs);

            let max_abs = outputs[0].iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
            assert!(
                max_abs > 0.5,
                "LFO waveform {i} should produce significant output, got max={max_abs}"
            );
        }
    }

    #[test]
    fn lfo_sync_resets_phase() {
        let mut lfo = LfoNode::new(44100.0);
        lfo.rate = 1.0; // 1 Hz

        // Process half a second.
        let zeros = vec![0.0; 22050];
        let sync_off = vec![0.0; 22050];
        let mut out1 = vec![0.0; 22050];
        let inputs = [zeros.as_slice(), sync_off.as_slice()];
        let mut outputs = [out1.as_mut_slice()];
        lfo.process(&inputs, &mut outputs);

        // Now send a sync pulse — should reset to beginning of cycle.
        let mut sync_pulse = vec![0.0_f32; 128];
        sync_pulse[0] = 1.0;
        let zeros_short = vec![0.0; 128];
        let mut out2 = vec![0.0; 128];
        let inputs = [zeros_short.as_slice(), sync_pulse.as_slice()];
        let mut outputs = [out2.as_mut_slice()];
        lfo.process(&inputs, &mut outputs);

        // After sync, first sample should be near the start of the waveform (sine: ~0).
        assert!(
            outputs[0][0].abs() < 0.1,
            "after sync, LFO should restart near zero, got {}",
            outputs[0][0]
        );
    }

    #[test]
    fn lfo_output_bounded() {
        let mut lfo = LfoNode::new(44100.0);
        lfo.rate = 50.0; // Fast
        lfo.depth = 1.0;

        let zeros = vec![0.0; 4410];
        let sync = vec![0.0; 4410];
        let mut output = vec![0.0; 4410];
        let inputs = [zeros.as_slice(), sync.as_slice()];
        let mut outputs = [output.as_mut_slice()];
        lfo.process(&inputs, &mut outputs);

        for &s in &*outputs[0] {
            assert!(
                s.is_finite() && s >= -1.1 && s <= 1.1,
                "LFO output out of range: {s}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // ADSR Node tests
    // -----------------------------------------------------------------------

    #[test]
    fn adsr_idle_is_zero() {
        let mut adsr = AdsrNode::new(44100.0);
        let gate = vec![0.0; 128];
        let mut output = vec![0.0; 128];
        let inputs = [gate.as_slice()];
        let mut outputs = [output.as_mut_slice()];
        adsr.process(&inputs, &mut outputs);

        for &s in &*outputs[0] {
            assert!(s.abs() < f32::EPSILON, "idle ADSR should be zero, got {s}");
        }
    }

    #[test]
    fn adsr_attack_reaches_peak() {
        let mut adsr = AdsrNode::new(44100.0);
        adsr.attack = 0.01;
        adsr.recompute_coefficients();

        // Gate on for 1 second.
        let gate = vec![1.0; 44100];
        let mut output = vec![0.0; 44100];
        let inputs = [gate.as_slice()];
        let mut outputs = [output.as_mut_slice()];
        adsr.process(&inputs, &mut outputs);

        let max_val = outputs[0].iter().copied().fold(0.0_f32, f32::max);
        assert!(
            max_val > 0.95,
            "ADSR should reach near 1.0 during attack, got {max_val}"
        );
    }

    #[test]
    fn adsr_sustain_holds() {
        let mut adsr = AdsrNode::new(44100.0);
        adsr.attack = 0.001;
        adsr.decay = 0.001;
        adsr.sustain = 0.5;
        adsr.recompute_coefficients();

        let gate = vec![1.0; 44100];
        let mut output = vec![0.0; 44100];
        let inputs = [gate.as_slice()];
        let mut outputs = [output.as_mut_slice()];
        adsr.process(&inputs, &mut outputs);

        // Last portion should be near sustain level.
        let tail_avg: f32 = outputs[0][40000..].iter().sum::<f32>() / 4100.0;
        assert!(
            (tail_avg - 0.5).abs() < 0.05,
            "ADSR sustain should hold at 0.5, got {tail_avg}"
        );
    }

    #[test]
    fn adsr_release_reaches_zero() {
        let mut adsr = AdsrNode::new(44100.0);
        adsr.attack = 0.001;
        adsr.decay = 0.001;
        adsr.sustain = 0.8;
        adsr.release = 0.01;
        adsr.recompute_coefficients();

        // Gate on then off.
        let mut gate = vec![1.0; 44100];
        gate.extend_from_slice(&vec![0.0; 44100]);
        let mut output = vec![0.0; 88200];
        let inputs = [gate.as_slice()];
        let mut outputs = [output.as_mut_slice()];
        adsr.process(&inputs, &mut outputs);

        let tail_max = outputs[0][80000..]
            .iter()
            .map(|s| s.abs())
            .fold(0.0_f32, f32::max);
        assert!(
            tail_max < 0.01,
            "ADSR should reach near zero after release, got {tail_max}"
        );
    }

    #[test]
    fn adsr_output_always_bounded() {
        let mut adsr = AdsrNode::new(44100.0);
        let mut gate = vec![1.0; 22050];
        gate.extend_from_slice(&vec![0.0; 22050]);
        let mut output = vec![0.0; 44100];
        let inputs = [gate.as_slice()];
        let mut outputs = [output.as_mut_slice()];
        adsr.process(&inputs, &mut outputs);

        for (i, &s) in outputs[0].iter().enumerate() {
            assert!(
                s.is_finite() && s >= 0.0 && s <= 1.0,
                "ADSR output at sample {i} out of range: {s}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Clock Node tests
    // -----------------------------------------------------------------------

    #[test]
    fn clock_generates_triggers() {
        let mut clock = ClockNode::new(44100.0);
        clock.bpm = 120.0; // 2 beats per second

        let mut trigger = vec![0.0; 44100]; // 1 second
        let mut gate = vec![0.0; 44100];
        let mut outputs = [trigger.as_mut_slice(), gate.as_mut_slice()];
        clock.process(&[], &mut outputs);

        // Count triggers (should be ~2 per second at 120 BPM).
        let trigger_count = outputs[0].iter().filter(|&&s| s > 0.5).count();
        assert!(
            trigger_count >= 1 && trigger_count <= 3,
            "120 BPM should produce ~2 triggers per second, got {trigger_count}"
        );
    }

    #[test]
    fn clock_gate_respects_length() {
        let mut clock = ClockNode::new(44100.0);
        clock.bpm = 60.0; // 1 beat per second
        clock.gate_length = 0.5; // 50% duty

        let mut trigger = vec![0.0; 44100];
        let mut gate = vec![0.0; 44100];
        let mut outputs = [trigger.as_mut_slice(), gate.as_mut_slice()];
        clock.process(&[], &mut outputs);

        let gate_on_count = outputs[1].iter().filter(|&&s| s > 0.5).count();
        let gate_ratio = gate_on_count as f32 / 44100.0;
        // Should be approximately 50% of the beat.
        assert!(
            (gate_ratio - 0.5).abs() < 0.05,
            "gate should be ~50% duty, got {gate_ratio:.2}"
        );
    }

    // -----------------------------------------------------------------------
    // Sample & Hold tests
    // -----------------------------------------------------------------------

    #[test]
    fn sample_hold_captures_on_trigger() {
        let mut sh = SampleHoldNode::new();

        // Input signal ramps from 0 to 1.
        let input: Vec<f32> = (0..128).map(|i| i as f32 / 127.0).collect();
        // Trigger at sample 64.
        let mut trigger = vec![0.0; 128];
        trigger[64] = 1.0;
        let mut output = vec![0.0; 128];

        let inputs = [input.as_slice(), trigger.as_slice()];
        let mut outputs = [output.as_mut_slice()];
        sh.process(&inputs, &mut outputs);

        // Before trigger (sample 0-63): should be initial value (0.0).
        assert!(
            outputs[0][32].abs() < f32::EPSILON,
            "before trigger, S&H should output 0, got {}",
            outputs[0][32]
        );

        // After trigger (sample 65+): should hold the value at sample 64.
        let expected = 64.0 / 127.0;
        assert!(
            (outputs[0][100] - expected).abs() < 0.01,
            "after trigger, S&H should hold ~{expected:.3}, got {}",
            outputs[0][100]
        );
    }

    #[test]
    fn sample_hold_holds_between_triggers() {
        let mut sh = SampleHoldNode::new();

        // First trigger at value 0.5.
        let input = vec![0.5; 128];
        let mut trigger = vec![0.0; 128];
        trigger[0] = 1.0;
        let mut output = vec![0.0; 128];

        let inputs = [input.as_slice(), trigger.as_slice()];
        let mut outputs = [output.as_mut_slice()];
        sh.process(&inputs, &mut outputs);

        // Now process with different input but no trigger.
        let input2 = vec![0.9; 128];
        let trigger2 = vec![0.0; 128];
        let mut output2 = vec![0.0; 128];

        let inputs = [input2.as_slice(), trigger2.as_slice()];
        let mut outputs = [output2.as_mut_slice()];
        sh.process(&inputs, &mut outputs);

        // Should still hold 0.5 (the previously sampled value).
        for &s in &*outputs[0] {
            assert!(
                (s - 0.5).abs() < f32::EPSILON,
                "S&H should hold 0.5, got {s}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Quantizer tests
    // -----------------------------------------------------------------------

    #[test]
    fn quantizer_chromatic_passes_through() {
        let mut quant = QuantizerNode::new();
        quant.scale = Scale::Chromatic;

        // A chromatic scale should snap to the nearest semitone.
        // Input: exactly 0.0 octaves = root.
        let input = vec![0.0; 128];
        let mut output = vec![0.0; 128];
        let inputs = [input.as_slice()];
        let mut outputs = [output.as_mut_slice()];
        quant.process(&inputs, &mut outputs);

        for &s in &*outputs[0] {
            assert!(
                s.abs() < 0.05,
                "chromatic quantize of 0.0 should be near 0, got {s}"
            );
        }
    }

    #[test]
    fn quantizer_major_scale_snaps() {
        let quant = QuantizerNode::new(); // Default: Major scale

        // Major scale degrees in semitones: 0, 2, 4, 5, 7, 9, 11
        // Test: 1 semitone (0.0833 octaves) should snap to 0 or 2 (nearest = 0)
        let result = quant.quantize(1.0 / 12.0); // 1 semitone in octave units
        let result_semi = result * 12.0;
        // 1 semitone is equidistant between 0 and 2.
        // Should snap to the nearest (0 or 2), depending on implementation.
        assert!(
            (result_semi - 0.0).abs() < 0.01 || (result_semi - 2.0).abs() < 0.01,
            "1 semitone should snap to 0 or 2 in major scale, got {result_semi}"
        );

        // 3 semitones should snap to 2 or 4 (not 3, since Eb is not in C major).
        let result = quant.quantize(3.0 / 12.0);
        let result_semi = result * 12.0;
        assert!(
            (result_semi - 2.0).abs() < 0.01 || (result_semi - 4.0).abs() < 0.01,
            "3 semitones should snap to 2 or 4 in major, got {result_semi}"
        );
    }

    #[test]
    fn quantizer_pentatonic_no_seconds_or_sixths() {
        let mut quant = QuantizerNode::new();
        quant.scale = Scale::PentatonicMajor;

        // Pentatonic major: 0, 2, 4, 7, 9
        // 5 semitones (F in C) should snap to 4 (E) or 7 (G).
        let result = quant.quantize(5.0 / 12.0);
        let result_semi = result * 12.0;
        assert!(
            (result_semi - 4.0).abs() < 0.01 || (result_semi - 7.0).abs() < 0.01,
            "5 semitones should snap to 4 or 7 in pentatonic, got {result_semi}"
        );
    }

    // -----------------------------------------------------------------------
    // Delay Node tests
    // -----------------------------------------------------------------------

    #[test]
    fn delay_produces_echo() {
        let mut delay = DelayNode::new(44100.0);
        delay.delay_time = 0.01; // 10ms = 441 samples
        delay.feedback = 0.0;
        delay.dry_wet = 1.0; // Fully wet

        // Impulse at sample 0.
        let mut input = vec![0.0; 4410];
        input[0] = 1.0;
        let fb_cv = vec![0.0; 4410];
        let mut output = vec![0.0; 4410];

        let inputs = [input.as_slice(), fb_cv.as_slice()];
        let mut outputs = [output.as_mut_slice()];
        delay.process(&inputs, &mut outputs);

        // The impulse should appear delayed by ~441 samples.
        // Check that the region around 441 has some energy.
        let delay_region: f32 = outputs[0][430..460]
            .iter()
            .map(|s| s.abs())
            .fold(0.0, f32::max);
        assert!(
            delay_region > 0.5,
            "should hear echo near 441 samples, got max={delay_region}"
        );

        // Before the delay, should be silence (wet only).
        let pre_delay_max: f32 = outputs[0][2..400]
            .iter()
            .map(|s| s.abs())
            .fold(0.0, f32::max);
        assert!(
            pre_delay_max < 0.01,
            "before delay time, should be silence, got {pre_delay_max}"
        );
    }

    #[test]
    fn delay_feedback_creates_repeats() {
        let mut delay = DelayNode::new(44100.0);
        delay.delay_time = 0.01;
        delay.feedback = 0.5;
        delay.dry_wet = 1.0;

        let mut input = vec![0.0; 4410];
        input[0] = 1.0;
        let fb_cv = vec![0.0; 4410];
        let mut output = vec![0.0; 4410];

        let inputs = [input.as_slice(), fb_cv.as_slice()];
        let mut outputs = [output.as_mut_slice()];
        delay.process(&inputs, &mut outputs);

        // Should have a second echo at ~882 samples (2× delay).
        let second_echo: f32 = outputs[0][870..900]
            .iter()
            .map(|s| s.abs())
            .fold(0.0, f32::max);
        assert!(
            second_echo > 0.2,
            "feedback should produce second echo, got {second_echo}"
        );
    }

    #[test]
    fn delay_output_finite() {
        let mut delay = DelayNode::new(44100.0);
        delay.feedback = 0.9;

        let input = vec![0.5; 4410];
        let fb_cv = vec![0.0; 4410];
        let mut output = vec![0.0; 4410];

        let inputs = [input.as_slice(), fb_cv.as_slice()];
        let mut outputs = [output.as_mut_slice()];
        delay.process(&inputs, &mut outputs);

        for &s in &*outputs[0] {
            assert!(s.is_finite(), "delay output must be finite");
        }
    }

    // -----------------------------------------------------------------------
    // Slew Limiter tests
    // -----------------------------------------------------------------------

    #[test]
    fn slew_smooths_step() {
        let mut slew = SlewNode::new(44100.0);
        slew.rise_rate = 10.0; // 10 units/sec
        slew.fall_rate = 10.0;

        // Step from 0 to 1.
        let input = vec![1.0; 4410]; // 100ms
        let mut output = vec![0.0; 4410];

        let inputs = [input.as_slice()];
        let mut outputs = [output.as_mut_slice()];
        slew.process(&inputs, &mut outputs);

        // First sample should be small (slew limiting the step).
        assert!(
            outputs[0][0] < 0.01,
            "slew should limit initial step, got {}",
            outputs[0][0]
        );

        // Eventually should reach target.
        let last = outputs[0][4409];
        assert!(
            (last - 1.0).abs() < 0.01,
            "slew should reach target after 100ms, got {last}"
        );
    }

    #[test]
    fn slew_asymmetric_rates() {
        let mut slew = SlewNode::new(44100.0);
        slew.rise_rate = 100.0; // Very fast rise
        slew.fall_rate = 1.0; // Slow fall

        // Step up to 1.0.
        let input = vec![1.0; 441]; // 10ms
        let mut output = vec![0.0; 441];
        let inputs = [input.as_slice()];
        let mut outputs = [output.as_mut_slice()];
        slew.process(&inputs, &mut outputs);

        let peak = outputs[0][440];
        assert!(
            peak > 0.9,
            "fast rise should reach target quickly, got {peak}"
        );

        // Now step down to 0.
        let input = vec![0.0; 4410]; // 100ms
        let mut output = vec![0.0; 4410];
        let inputs = [input.as_slice()];
        let mut outputs = [output.as_mut_slice()];
        slew.process(&inputs, &mut outputs);

        // After 10ms, should still be well above zero (slow fall).
        assert!(
            outputs[0][441] > 0.5,
            "slow fall should still be high at 10ms, got {}",
            outputs[0][441]
        );
    }

    #[test]
    fn slew_output_finite() {
        let mut slew = SlewNode::new(44100.0);
        let input = vec![f32::NAN; 128];
        let mut output = vec![0.0; 128];
        let inputs = [input.as_slice()];
        let mut outputs = [output.as_mut_slice()];
        slew.process(&inputs, &mut outputs);

        for &s in &*outputs[0] {
            assert!(s.is_finite(), "slew output must be finite");
        }
    }

    // -----------------------------------------------------------------------
    // Integration: Clock -> ADSR -> VCA pipeline
    // -----------------------------------------------------------------------

    #[test]
    fn clock_adsr_vca_pipeline() {
        use super::super::graph::NodeGraph;

        let mut graph = NodeGraph::new(44100.0, 4410); // 100ms blocks

        let clock_id = graph.add_node(Box::new(ClockNode::new(44100.0)));
        let adsr_id = graph.add_node(Box::new(AdsrNode::new(44100.0)));
        let osc_id = graph.add_node(Box::new(OscNode::new(44100.0)));
        let vca_id = graph.add_node(Box::new(VcaNode::new()));

        // Clock gate -> ADSR gate.
        assert!(graph.connect(clock_id, 1, adsr_id, 0));
        // Osc -> VCA audio in.
        assert!(graph.connect(osc_id, 0, vca_id, 0));
        // ADSR -> VCA CV.
        assert!(graph.connect(adsr_id, 0, vca_id, 1));

        // Process several blocks.
        for _ in 0..10 {
            graph.process();
        }

        // VCA output should have some signal (the oscillator modulated by ADSR).
        let vca_out = graph.get_output(vca_id, 0).expect("VCA should have output");
        let max_abs = vca_out.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
        assert!(
            max_abs > 0.01,
            "clock->ADSR->VCA pipeline should produce audible output, got {max_abs}"
        );
    }
}
