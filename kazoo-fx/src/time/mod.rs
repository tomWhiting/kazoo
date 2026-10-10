//! The time family. Each effect lives in its own file here and is listed in
//! [`KINDS`].
//!
//! Delays and the modulation effects built on short delays and allpasses:
//! a tape echo, a bucket-brigade delay, a clean digital delay, a chorus, a
//! flanger, a phaser, a tremolo, a frequency shifter and a granular delay;
//! and a pitch shifter, which reads a delay line at a moving delay.

use crate::EffectKind;

pub mod bbd;
pub mod chorus;
pub mod digital;
pub mod flanger;
mod formant;
pub mod granular;
mod parts;
pub mod phaser;
pub mod pitch;
pub mod shift;
pub mod tape;
pub mod trem;

#[cfg(test)]
mod contract;
#[cfg(test)]
mod rates;
#[cfg(test)]
mod review;
#[cfg(test)]
mod testkit;

/// Every effect in this family.
pub static KINDS: &[EffectKind] = &[
    tape::KIND,
    bbd::KIND,
    digital::KIND,
    chorus::KIND,
    flanger::KIND,
    phaser::KIND,
    trem::KIND,
    shift::KIND,
    granular::KIND,
    pitch::KIND,
];
