//! The quantiser: holds a pitch (1.0 per octave) to the nearest note of a
//! scale on a chosen root.

use super::{Io, Module, Tick, finite};
use crate::SUB_BLOCK;
use crate::catalogue::SCALES;

const SCALE: usize = 0;
const ROOT: usize = 1;

const IN_PITCH: usize = 0;

/// Octaves either side of C4 the quantiser covers.
const RANGE_OCTAVES: f32 = 8.0;

#[derive(Debug)]
pub struct Quant;

impl Quant {
    pub const fn new() -> Self {
        Self
    }
}

/// `pitch` (1.0 per octave) moved to the nearest note of `scale` on `root`
/// (semitones above C).
fn quantise(pitch: f32, scale: &[u8], root: f32) -> f32 {
    let semitones = finite(pitch)
        .clamp(-RANGE_OCTAVES, RANGE_OCTAVES)
        .mul_add(12.0, -root);
    let octave = (semitones / 12.0).floor();
    let within = octave.mul_add(-12.0, semitones);
    let mut best = 0.0_f32;
    let mut distance = f32::MAX;
    // The next octave's root is a candidate too.
    for degree in scale
        .iter()
        .map(|&d| f32::from(d))
        .chain(std::iter::once(12.0))
    {
        let gap = (within - degree).abs();
        if gap < distance {
            distance = gap;
            best = degree;
        }
    }
    (octave.mul_add(12.0, best) + root) / 12.0
}

impl Module for Quant {
    fn process(&mut self, _tick: &Tick, io: Io<'_>) {
        // Clamped and rounded: the casts are exact.
        let scale = io.knob(SCALE) as usize;
        let root = io.knob(ROOT);
        let degrees = SCALES[scale.min(SCALES.len() - 1)];
        for frame in 0..SUB_BLOCK {
            io.outputs[0][frame] = quantise(io.inputs[IN_PITCH][frame], degrees, root);
        }
    }

    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::super::testing::{Bench, stays_in_range, survives_nonsense};
    use super::*;
    use crate::catalogue::Kind;

    #[test]
    fn knob_and_port_indices_match_the_catalogue() {
        let spec = Kind::QUANT.spec();
        assert_eq!(spec.knobs[SCALE].name, "scale");
        assert_eq!(spec.knobs[ROOT].name, "root");
        assert_eq!(spec.inputs[IN_PITCH].name, "in");
    }

    fn semis(pitch: f32) -> f32 {
        (pitch * 12.0 * 1_000.0).round() / 1_000.0
    }

    #[test]
    fn it_snaps_to_the_scale() {
        let major = SCALES[1];
        // C# (1 st) in C major is closer to C or D: ties go to the lower.
        assert!((semis(quantise(1.0 / 12.0, major, 0.0)) - 0.0).abs() < 1e-3);
        assert!((semis(quantise(1.4 / 12.0, major, 0.0)) - 2.0).abs() < 1e-3);
        // F# in C major: F or G, lower wins the tie.
        assert!((semis(quantise(6.0 / 12.0, major, 0.0)) - 5.0).abs() < 1e-3);
        // Below C4: B3 is in the scale.
        assert!((semis(quantise(-1.1 / 12.0, major, 0.0)) + 1.0).abs() < 1e-3);
        // A root of A moves the scale: A minor pentatonic holds C.
        let pentatonic = SCALES[8];
        assert!((semis(quantise(0.2 / 12.0, pentatonic, 9.0)) - 0.0).abs() < 1e-3);
        // Chromatic rounds to the nearest semitone.
        let chromatic = SCALES[0];
        assert!((semis(quantise(2.6 / 12.0, chromatic, 0.0)) - 3.0).abs() < 1e-3);
        // Just under the next octave's root snaps up to it.
        assert!((semis(quantise(11.9 / 12.0, major, 0.0)) - 12.0).abs() < 1e-3);
    }

    #[test]
    fn it_reads_its_knobs() {
        let mut bench = Bench::new(Kind::QUANT);
        bench.knob("scale", 9.0).knob("root", 1.0).hold("in", 0.0);
        let out = bench.render(0, 32);
        // Whole tone on C#: C is between B and C#; ties go down, to B.
        assert!((semis(out[0]) + 1.0).abs() < 1e-3, "{}", semis(out[0]));
    }

    #[test]
    fn output_stays_in_range_and_survives_nonsense() {
        stays_in_range(Kind::QUANT, RANGE_OCTAVES + 1.0);
        survives_nonsense(Kind::QUANT);
    }
}
