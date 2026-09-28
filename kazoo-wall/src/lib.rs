//! The wall: a shared modular synth that never stops.
//!
//! A daemon plays one persistent patch forever. Anybody on the machine can
//! reach up at any time, over the daemon's control socket, and twiddle a
//! knob, move a plug, add a module or take one away; every change is logged
//! with who made it and can be undone.
//!
//! - [`daemon`]: the daemon: control socket, audio, persistence.
//! - [`catalogue`]: every module kind, with its knobs and ports.
//! - [`dsp`]: the realtime processor for each kind.
//! - [`adapters`]: drum voices, rhythm generators, the vocoder and the
//!   speaker from the studio's other crates, as modules.
//! - [`engine`]: the realtime engine that renders the patch.
//! - [`patch`]: the patch model the daemon keeps and saves.
//! - [`protocol`]: the control protocol's messages, a line codec, and a
//!   blocking client.
//! - [`change`]: plain-words summaries of changes, and timestamps.
//! - [`format`]: values with their units, as people read them.
//! - [`paths`]: where the socket and the saved state live.
//! - [`store`]: saving the patch and the change log.
//! - [`migrate`]: bringing patches saved by older walls up to date.
//! - [`seed`]: the patch a new wall starts from.
//! - [`listen`]: describing what the wall sounds like.
//! - [`fingerprints`]: who touched what, and where the signal carried it.

pub mod adapters;
pub mod catalogue;
pub mod change;
pub mod daemon;
pub mod dsp;
pub mod engine;
pub mod fingerprints;
pub mod format;
pub mod listen;
pub mod migrate;
pub mod patch;
pub mod paths;
pub mod protocol;
pub mod seed;
pub mod store;

/// In tests, every allocation and free goes through an allocator that can be
/// told a region must not allocate (`assert_no_alloc`): the engine's tests
/// render under it and fail on any allocation or free.
#[cfg(test)]
#[global_allocator]
static TEST_ALLOCATOR: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

/// Most modules the wall holds.
pub const MAX_MODULES: usize = 96;

/// Most cables the wall holds.
pub const MAX_CABLES: usize = 320;

// The processing order keeps slots as bytes, and delay lines are numbered
// in u16.
const _: () = assert!(MAX_MODULES <= 256);
const _: () = assert!(MAX_CABLES <= u16::MAX as usize);

/// Frames the engine renders at once: every module sees the others' output
/// from this far back at most, which is the delay a feedback cycle adds.
pub const SUB_BLOCK: usize = 32;

/// Most inputs a module kind has.
pub const MAX_INPUTS: usize = 4;

/// Most outputs a module kind has.
pub const MAX_OUTPUTS: usize = 4;

/// Most knobs a module kind has (`kazoo-perc`'s probability sequencer has
/// 22; the wall's own sequencer 19).
pub const MAX_KNOBS: usize = 24;

/// Places a cable can plug into on one module: its inputs, then a jack for
/// each knob. Jack `i` below [`MAX_INPUTS`] is input `i`; jack
/// `MAX_INPUTS + k` is knob `k`'s jack.
pub const MAX_JACKS: usize = MAX_INPUTS + MAX_KNOBS;
