//! Node graph engine for generative/modular synthesis.
//!
//! Each module (VCO, VCF, VCA, ENV, LFO, Ring Mod, Noise, Mixer)
//! is a node with typed inputs (audio, control, trigger) and outputs.
//! Connections are patched freely. Processing order via topological sort.

pub mod graph;
pub mod node;
pub mod nodes;
