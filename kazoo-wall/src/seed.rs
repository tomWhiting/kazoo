//! The seed patch: where a new wall starts. A starting point, not a
//! destination.
//!
//! A clock drives an eight-step sequence through a quantiser (A minor
//! pentatonic) into two oscillators a fifth apart. They are mixed into a
//! resonant filter that a very slow LFO sweeps, shaped by an envelope on a
//! VCA, then sent through a delay and a reverb from `kazoo-fx` to the
//! output. A second, smooth-random LFO wanders the second oscillator's pulse
//! width and the sequence's gate chance, so the wall keeps moving on its own
//! without anybody touching it.
//!
//! The delay is the first of `tape` and `bbd` the catalogue has, and the
//! reverb the first of `plate` and `hall`. A stage with none of its effects
//! is left out (and said so); the rest still plays.

use crate::catalogue::Kind;
use crate::patch::Patch;
use crate::protocol::{WallError, What};

/// Tempo of a new wall.
pub const SEED_TEMPO: f64 = 92.0;

/// Delays the seed tries, in order.
pub const SEED_DELAYS: [&str; 2] = ["tape", "bbd"];

/// Reverbs the seed tries, in order.
pub const SEED_REVERBS: [&str; 2] = ["plate", "hall"];

/// The seed's knobs: module, knob, value.
const SEED_KNOBS: &[(&str, &str, f64)] = &[
    ("clock1", "division", 1.0),
    ("seq1", "steps", 8.0),
    ("seq1", "step1", 0.0),
    ("seq1", "step2", 3.0),
    ("seq1", "step3", 7.0),
    ("seq1", "step4", 10.0),
    ("seq1", "step5", 12.0),
    ("seq1", "step6", 7.0),
    ("seq1", "step7", 5.0),
    ("seq1", "step8", -2.0),
    ("seq1", "gate", 0.45),
    ("seq1", "chance", 0.8),
    ("quant1", "scale", 8.0),
    ("quant1", "root", 9.0),
    ("vco1", "octave", -1.0),
    ("vco1", "shape", 2.0),
    ("vco1", "level", 0.7),
    ("vco2", "octave", -1.0),
    ("vco2", "tune", 7.05),
    ("vco2", "shape", 3.0),
    ("vco2", "width", 0.35),
    ("vco2", "level", 0.45),
    ("mix1", "level_a", 0.6),
    ("mix1", "level_b", 0.5),
    ("vcf1", "cutoff", 900.0),
    ("vcf1", "resonance", 0.55),
    ("vcf1", "drive", 0.25),
    ("lfo1", "rate", 0.04),
    ("lfo2", "rate", 0.11),
    ("lfo2", "shape", 5.0),
    ("env1", "attack", 0.004),
    ("env1", "decay", 0.28),
    ("env1", "sustain", 0.25),
    ("env1", "release", 0.45),
    ("out1", "level", 0.8),
];

/// The seed's cables up to the amp: from, to, amount.
const SEED_CABLES: &[(&str, &str, f64)] = &[
    ("clock1.out", "seq1.clock", 1.0),
    ("seq1.pitch", "quant1.in", 1.0),
    ("quant1.out", "vco1.pitch", 1.0),
    ("quant1.out", "vco2.pitch", 1.0),
    ("vco1.out", "mix1.a", 1.0),
    ("vco2.out", "mix1.b", 1.0),
    ("mix1.out", "vcf1.in", 1.0),
    ("lfo1.out", "vcf1.cutoff", 0.35),
    ("env1.out", "vcf1.resonance", 0.15),
    ("lfo2.out", "vco2.width", 0.4),
    ("lfo2.out", "seq1.chance", 0.3),
    ("seq1.gate", "env1.gate", 1.0),
    ("vcf1.out", "vca1.in", 1.0),
    ("env1.out", "vca1.cv", 1.0),
];

/// Build the seed patch.
///
/// # Errors
///
/// Never expected: every step is a valid operation on a patch this function
/// builds from nothing. An error names the step that failed.
pub fn seed() -> Result<Patch, WallError> {
    seed_from(&SEED_DELAYS, &SEED_REVERBS)
}

/// The seed, with its delay the first of `delays` in the catalogue and its
/// reverb the first of `reverbs`.
///
/// # Errors
///
/// As [`seed`].
pub fn seed_from(delays: &[&str], reverbs: &[&str]) -> Result<Patch, WallError> {
    let mut patch = Patch::empty(SEED_TEMPO);
    for (kind, name) in [
        ("clock", "eighths"),
        ("seq", "melody"),
        ("quant", "a minor pent"),
        ("vco", "saw"),
        ("vco", "pulse fifth"),
        ("mix", "osc mix"),
        ("vcf", "filter"),
        ("lfo", "slow sweep"),
        ("lfo", "wander"),
        ("env", "pluck"),
        ("vca", "amp"),
        ("out", "main"),
    ] {
        patch.add(kind, Some(name), None)?;
    }
    for &(module, knob, value) in SEED_KNOBS {
        patch.turn(module, knob, value, Some(0.0))?;
    }
    for &(from, to, amount) in SEED_CABLES {
        patch.plug(from, to, Some(amount), None)?;
    }
    // The effects chain: whatever stages the catalogue can fill, in order,
    // from the amp to the output.
    let mut from = vec!["vca1.out".to_string()];
    for (stage, choices, settings) in [
        (
            "delay",
            delays,
            &[("mix", 0.35), ("feedback", 0.45)] as &[(&str, f64)],
        ),
        ("reverb", reverbs, &[("mix", 0.3)]),
    ] {
        let Some(kind) = choices.iter().find_map(|name| Kind::from_name(name)) else {
            eprintln!(
                "kazoo-wall: the seed has no {stage}: none of {} is in the catalogue",
                choices.join(", ")
            );
            continue;
        };
        let What::Add { module } = patch.add(kind.name(), Some(stage), None)? else {
            continue;
        };
        for &(knob, value) in settings {
            if kind.spec().knob_index(knob).is_some() {
                patch.turn(&module.id, knob, value, Some(0.0))?;
            }
        }
        // A mono source feeds the left input (the right is normalled to
        // it); a stereo one feeds both.
        for (source, side) in from.iter().zip(["left", "right"]) {
            patch.plug(source, &format!("{}.{side}", module.id), None, None)?;
        }
        from = vec![
            format!("{}.left", module.id),
            format!("{}.right", module.id),
        ];
    }
    for (source, side) in from.iter().zip(["left", "right"]) {
        patch.plug(source, &format!("out1.{side}"), None, None)?;
    }
    // A row of the rack for each group of modules, sources at the top.
    patch.regroup();
    Ok(patch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(patch: &Patch, kind: Kind) -> usize {
        patch.modules().iter().filter(|m| m.kind == kind).count()
    }

    #[test]
    fn the_seed_builds_as_described() {
        let patch = seed_from(&["testgain"], &["testgain"]).unwrap();
        assert_eq!(count(&patch, Kind::VCO), 2);
        assert_eq!(count(&patch, Kind::SEQ), 1);
        assert_eq!(count(&patch, Kind::QUANT), 1);
        assert_eq!(count(&patch, Kind::LFO), 2);
        assert_eq!(count(&patch, Kind::ENV), 1);
        assert_eq!(count(&patch, Kind::VCA), 1);
        assert_eq!(count(&patch, Kind::OUT), 1);
        assert_eq!(count(&patch, Kind::from_name("testgain").unwrap()), 2);
        // 14 cables to the amp, one into the first effect, two into the
        // second, two to the output.
        assert_eq!(patch.cables().len(), 19);
        assert!((patch.tempo - SEED_TEMPO).abs() < f64::EPSILON);
    }

    #[test]
    fn missing_effects_leave_their_stage_out() {
        let patch = seed_from(&["no_such_delay"], &["no_such_reverb"]).unwrap();
        assert_eq!(patch.modules().len(), 12);
        let records = patch.cable_records();
        assert!(
            records
                .iter()
                .any(|c| c.from == "vca1.out" && c.to == "out1.left")
        );
        assert_eq!(records.len(), 15);
        // One stage present, one missing.
        let patch = seed_from(&["nothing"], &["testgain"]).unwrap();
        assert_eq!(patch.modules().len(), 13);
        assert_eq!(patch.cables().len(), 17);
    }

    #[test]
    fn the_real_seed_builds_whatever_kazoo_fx_holds() {
        let patch = seed().unwrap();
        assert!(patch.modules().len() >= 12);
    }
}
