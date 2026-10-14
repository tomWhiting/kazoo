//! Input-driven synthesis: clocks and processors made from the input itself.
//!
//! Sidestep follows Esqueda, Lähdeoja and Välimäki, SMC 2018, pp. 444–451,
//! <https://doi.org/10.5281/zenodo.1422641>, for the adaptive clock and
//! processors. Period-driven filtering follows the GR-300 description at
//! <https://gr300.com/GR-300.htm>. The Boss OC-2's flip-flop dividers are
//! described at <https://en.wikipedia.org/wiki/Octave_effect> and compared
//! in The Valve Wizard's own U-Boat design,
//! <https://www.valvewizard.co.uk/uboat.html>. Boss's SY-1 and SY-300
//! method is not public. These effects do not claim to reproduce it.

mod blep;
mod octaves;
mod sidestep;
mod svf;
#[cfg(test)]
pub(crate) mod testkit;
#[cfg(test)]
#[path = "sidestep/tests.rs"]
mod tests;

/// Every effect in this family.
pub static KINDS: &[crate::EffectKind] = &[sidestep::KIND, octaves::KIND];
