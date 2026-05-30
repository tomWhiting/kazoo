//! Library surface for the procedural TR-808 engine.
//!
//! The binary keeps its standalone TUI/audio path, while `kazoo-mix` can reuse
//! the synthesis and sequencer modules through this library surface.

pub mod sequencer;
pub mod synth;
