//! Node trait and typed ports for the modular node graph.

use std::fmt;

/// Signal type flowing through a port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortType {
    /// Audio-rate signal (sample-by-sample).
    Audio,
    /// Control-rate signal (parameter modulation, envelopes).
    Control,
    /// Trigger/gate signal (note on/off events).
    Trigger,
}

impl fmt::Display for PortType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Audio => write!(f, "Audio"),
            Self::Control => write!(f, "Control"),
            Self::Trigger => write!(f, "Trigger"),
        }
    }
}

/// Describes an input or output port on a node.
#[derive(Debug, Clone)]
pub struct PortDescriptor {
    /// Human-readable name.
    pub name: String,
    /// Signal type.
    pub port_type: PortType,
}

/// Unique identifier for a node in the graph.
pub type NodeId = u32;

/// A processing node in the modular graph.
///
/// Nodes read from input buffers, process, and write to output buffers.
/// All buffers are pre-allocated by the graph engine.
pub trait ModularNode: fmt::Debug + Send {
    /// Human-readable name (e.g. "VCO", "LPF", "ADSR").
    fn name(&self) -> &'static str;

    /// Descriptions of all input ports.
    fn inputs(&self) -> &[PortDescriptor];

    /// Descriptions of all output ports.
    fn outputs(&self) -> &[PortDescriptor];

    /// Process one block of audio.
    ///
    /// `input_buffers`: one slice per input port, length = `block_size`.
    /// `output_buffers`: one slice per output port, length = `block_size`.
    ///
    /// The graph engine guarantees buffer sizes match and upstream nodes
    /// have already been processed (topological order).
    fn process(&mut self, input_buffers: &[&[f32]], output_buffers: &mut [&mut [f32]]);

    /// Reset all internal state.
    fn reset(&mut self);

    /// Update sample rate.
    fn set_sample_rate(&mut self, sample_rate: f32);

    /// Number of user-controllable parameters.
    fn param_count(&self) -> usize {
        0
    }

    /// Get parameter name and current value.
    fn param_info(&self, _index: usize) -> Option<(String, f32, f32, f32)> {
        None
    }

    /// Set parameter value.
    fn set_param(&mut self, _index: usize, _value: f32) {}
}
