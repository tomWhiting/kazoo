//! The drive family: circuit-modelled overdrives, a fuzz and a valve amp,
//! a wavefolder, a bit crusher, a ring modulator and a compressor.
//!
//! Each effect lives in its own file and is listed in [`KINDS`]. The files
//! beside them hold what the family shares: gliding knobs and the output
//! ceiling (`kit`), analogue-style filter design (`filter`), oversampling
//! to a target internal rate (`oversample`, which the tape echo's
//! saturation borrows), the stereo pair of
//! oversampled circuits the models are built on (`pair`), the circuit
//! solver (`solve`) and nodal analysis for linear networks (`network`).

use crate::EffectKind;

mod amp;
mod comp;
mod crush;
mod filter;
mod fold;
mod fuzz;
mod kit;
mod klon;
mod network;
pub(crate) mod oversample;
mod pair;
mod ringmod;
mod screamer;
mod solve;
#[cfg(test)]
mod testkit;

/// Every effect in this family.
pub static KINDS: &[EffectKind] = &[
    klon::KIND,
    screamer::KIND,
    fuzz::KIND,
    amp::KIND,
    fold::KIND,
    crush::KIND,
    ringmod::KIND,
    comp::KIND,
];
