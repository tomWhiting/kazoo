//! Tuned modal percussion: a fixed bank of damped resonances per voice.
//!
//! The DSP has no audio-device or terminal dependencies. The executable
//! connects it to Kazoo's mixer, transport, notation and keyboard front panel.

pub mod patch;
pub mod synth;

/// Fail audio-thread tests if the DSP allocates or frees heap memory.
#[cfg(test)]
#[global_allocator]
static TEST_ALLOCATOR: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;
