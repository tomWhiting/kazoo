//! The lofi family: worn media. Each effect lives in its own file here and
//! is listed in [`KINDS`]; the parts they share are in `parts`.
//!
//! - `vhs`: VHS audio, linear or hi-fi, with tracking and age.
//! - `cassette`: a cassette deck, tape types I, II and IV.
//! - `reel`: a studio reel-to-reel machine.
//! - `vinyl`: a record on a turntable, with a brake.
//! - `genloss`: a chain of up to eight tape dubs.
//! - `radio`: AM radio, telephone, walkie-talkie and megaphone.
//! - `sampler12`: an early sampler's 12-bit converters.
//!
//! Each file explains the physical model it follows.

mod cassette;
mod genloss;
mod parts;
mod radio;
mod reel;
mod sampler12;
mod vhs;
mod vinyl;

use crate::EffectKind;

/// Every effect in this family.
pub static KINDS: &[EffectKind] = &[
    vhs::KIND,
    cassette::KIND,
    reel::KIND,
    vinyl::KIND,
    genloss::KIND,
    radio::KIND,
    sampler12::KIND,
];

#[cfg(test)]
mod tests {
    use super::KINDS;

    #[test]
    fn every_id_is_lower_case_unique_and_found() {
        for (n, kind) in KINDS.iter().enumerate() {
            assert!(
                kind.id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
                "{}",
                kind.id
            );
            assert!(KINDS[n + 1..].iter().all(|other| other.id != kind.id));
            let found = crate::find(kind.id).map(|found| found.name);
            assert_eq!(found, Some(kind.name));
        }
    }
}
