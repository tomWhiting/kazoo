//! The space family: reverbs, a resonator bank and an EQ. Each effect lives
//! in its own file here and is listed in [`KINDS`].
//!
//! - [`plate`]: Dattorro's figure-eight plate.
//! - [`hall`]: an eight-line feedback delay network with three-band decay
//!   and freeze.
//! - [`spring`]: a two- or three-spring tank built from dispersive allpass
//!   chains.
//! - [`shimmer`]: a reverb with an octave (or other interval) shifter in
//!   its feedback.
//! - [`resonator`]: tuned modal and string resonators excited by the input.
//! - [`eq`]: a musical EQ with matched, uncramped bells and shelves.

use crate::EffectKind;

pub mod eq;
pub mod hall;
pub mod plate;
pub mod resonator;
pub mod shimmer;
pub mod spring;

mod halfband;
mod kit;
mod line;
mod matched;
#[cfg(test)]
mod testing;

/// Every effect in this family.
pub static KINDS: &[EffectKind] = &[
    plate::KIND,
    hall::KIND,
    spring::KIND,
    shimmer::KIND,
    resonator::KIND,
    eq::KIND,
];
