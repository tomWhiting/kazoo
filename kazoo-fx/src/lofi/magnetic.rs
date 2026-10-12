//! Magnetic tape recording with adaptive explicit midpoint integration.
//!
//! Field jumps already choose one to eight substeps. The half coercivity
//! target keeps the midpoint stability margin, so that rule and cap stay.
//! The shared Langevin values and slopes are prepared at construction.
//! Hermite interpolation replaces the transcendental work in each substep.

use std::sync::OnceLock;

use super::parts::finite;

const Q_MAX: f64 = 24.0;
const LANGEVIN_POINTS: usize = 8_193;
const LANGEVIN_STEP: f64 = 2.0 * Q_MAX / (LANGEVIN_POINTS - 1) as f64;
const LANGEVIN_SCALE: f64 = (LANGEVIN_POINTS - 1) as f64 / (2.0 * Q_MAX);
static LANGEVIN: OnceLock<Box<[(f64, f64)]>> = OnceLock::new();

#[derive(Clone, Copy)]
enum Langevin {
    Table,
    #[cfg(test)]
    Exact,
}

fn build_langevin() -> Box<[(f64, f64)]> {
    (0..LANGEVIN_POINTS)
        .map(|index| langevin_exact((index as f64).mul_add(LANGEVIN_STEP, -Q_MAX)))
        .collect()
}

#[inline]
fn langevin(q: f64) -> (f64, f64) {
    if !q.is_finite() {
        return (0.0, 1.0 / 3.0);
    }
    if q.abs() > Q_MAX {
        let inverse = 1.0 / q;
        return (q.signum() - inverse, inverse * inverse);
    }
    let Some(table) = LANGEVIN.get() else {
        panic!("magnetic construction must prepare the Langevin table");
    };
    let position = (q + Q_MAX) * LANGEVIN_SCALE;
    let index = (position as usize).min(LANGEVIN_POINTS - 2);
    let t = position - index as f64;
    let (left, left_slope) = table[index];
    let (right, right_slope) = table[index + 1];
    let change = right - left;
    let first = LANGEVIN_STEP * left_slope;
    let last = LANGEVIN_STEP * right_slope;
    let second = (-2.0f64).mul_add(first, 3.0 * change) - last;
    let third = (-2.0f64).mul_add(change, first + last);
    let value = t.mul_add(t.mul_add(t.mul_add(third, second), first), left);
    let slope = t.mul_add(right_slope - left_slope, left_slope);
    (value, slope)
}

/// Chowdhury's Jiles–Atherton constants for audio tape, divided through by
/// the saturation magnetisation so the magnetisation runs from -1 to 1:
/// the anhysteretic shape `a`, the loop width `k` (coercivity), the
/// reversible share `c` and the domain coupling `alpha`.
const JA_A: f64 = 2.2e4 / 3.5e5;
const JA_K: f64 = 2.7e4 / 3.5e5;
const JA_C: f64 = 1.7e-1;
const JA_ALPHA: f64 = 1.6e-3;

/// The Langevin function `coth q − 1/q` and its slope.
fn langevin_exact(q: f64) -> (f64, f64) {
    if q.abs() < 1e-2 {
        let q2 = q * q;
        (q * (1.0 / 3.0 - q2 / 45.0), 1.0 / 3.0 - q2 / 15.0)
    } else {
        let t = q.tanh();
        let coth = 1.0 / t;
        // 1/sinh² = coth² − 1.
        (coth - 1.0 / q, 1.0 / (q * q) - coth.mul_add(coth, -1.0))
    }
}

/// Where [`langevin_single`] changes from its series to the closed form.
const SERIES_JOIN: f32 = 0.5;

/// The Langevin function in single precision. Near zero,
/// `coth q − 1/q` cancels badly in single precision, so below
/// [`SERIES_JOIN`] it is the series `q/3 − q³/45 + 2q⁵/945 − q⁷/4725`,
/// whose first missing term is under 3e-9 there.
fn langevin_single(q: f32) -> f32 {
    if q.abs() < SERIES_JOIN {
        let q2 = q * q;
        let inner = q2.mul_add(2.0 / 945.0 - q2 / 4_725.0, -1.0 / 45.0);
        q * q2.mul_add(inner, 1.0 / 3.0)
    } else {
        1.0 / q.tanh() - 1.0 / q
    }
}

/// Recording onto magnetic tape.
///
/// The magnetisation follows the Jiles–Atherton model (Jiles and Atherton,
/// "Theory of ferromagnetic hysteresis", 1986), in the form Jatin
/// Chowdhury used for real-time tape (Digital Audio Effects conference,
/// 2019): the anhysteretic
/// (Langevin) curve the domains relax toward, an irreversible part that
/// only moves when the field changes direction, and a reversible share.
/// It is solved with the explicit midpoint rule in field steps, split
/// finer when the field jumps, which keeps it stable at any drive.
///
/// Ideal AC bias linearises recording onto the anhysteretic curve, so the
/// output blends that curve (what a well-biased machine records) with the
/// raw hysteresis loop (what bias leaves behind when it is too low): the
/// loop brings the level-dependent lag, low-level grit and odd harmonics of
/// an under-biased machine. Both are scaled for unity gain on small
/// signals.
#[derive(Debug, Clone, Copy)]
pub struct Magnetic {
    m: f64,
    h: f64,
}

impl Default for Magnetic {
    fn default() -> Self {
        LANGEVIN.get_or_init(build_langevin);
        Self { m: 0.0, h: 0.0 }
    }
}

impl Magnetic {
    /// One sample recorded with `drive` (the field per unit of signal, 0.05
    /// up to 64; 1.5 saturates full scale gently) and `loop_share` of the
    /// hysteresis loop (0 for ideal bias, up to 1 for none).
    pub fn process(&mut self, input: f32, drive: f32, loop_share: f32) -> f32 {
        self.process_with(input, drive, loop_share, Langevin::Table)
    }

    fn process_with(
        &mut self,
        input: f32,
        drive: f32,
        loop_share: f32,
        curve_kind: Langevin,
    ) -> f32 {
        let drive = drive.clamp(0.05, 64.0);
        let input = finite(input).clamp(-64.0, 64.0);
        let curve = Self::curve(input, drive);
        self.step(f64::from(drive * input) * JA_A, curve_kind);
        let looped = (3.0 * self.m) as f32 / drive;
        (looped - curve).mul_add(loop_share.clamp(0.0, 1.0), curve)
    }

    /// The anhysteretic curve alone, `3 L(k x) / k`: what ideally biased
    /// tape records, with no memory. Unity gain for small signals; `drive`
    /// as for [`Self::process`].
    #[must_use]
    pub fn curve(input: f32, drive: f32) -> f32 {
        let k = drive.clamp(0.05, 64.0);
        let q = k * finite(input).clamp(-64.0, 64.0);
        3.0 * langevin_single(q) / k
    }

    /// The slope of magnetisation against field at `m`, `h`, for a field
    /// moving in direction `delta`.
    #[inline]
    fn slope(m: f64, h: f64, delta: f64, curve_kind: Langevin) -> f64 {
        let q = (JA_ALPHA.mul_add(m, h)) / JA_A;
        let (anhysteretic, rise) = match curve_kind {
            Langevin::Table => langevin(q),
            #[cfg(test)]
            Langevin::Exact => langevin_exact(q),
        };
        let diff = anhysteretic - m;
        let irreversible = if delta * diff > 0.0 {
            let den = ((1.0 - JA_C) * delta).mul_add(JA_K, -JA_ALPHA * diff);
            if den.abs() > 1e-9 {
                (1.0 - JA_C) * diff / den
            } else {
                0.0
            }
        } else {
            0.0
        };
        let reversible = JA_C * rise / JA_A;
        let den = 1.0 - JA_C * JA_ALPHA * rise / JA_A;
        (irreversible + reversible) / den
    }

    /// Move the field to `h`, integrating the magnetisation along the way.
    fn step(&mut self, h: f64, curve_kind: Langevin) {
        let change = h - self.h;
        if change.abs() < 1e-12 {
            self.h = h;
            return;
        }
        let delta = change.signum();
        let pieces = (change.abs() / (JA_K * 0.5)).ceil().clamp(1.0, 8.0);
        let dh = change / pieces;
        let mut at = self.h;
        let mut m = self.m;
        for _ in 0..pieces as u32 {
            let first = Self::slope(m, at, delta, curve_kind) * dh;
            let second = Self::slope(
                0.5f64.mul_add(first, m),
                0.5f64.mul_add(dh, at),
                delta,
                curve_kind,
            ) * dh;
            m = (m + second).clamp(-1.0, 1.0);
            at += dh;
        }
        self.m = if m.is_finite() { m } else { 0.0 };
        self.h = h;
    }

    #[cfg(test)]
    const fn magnetisation(&self) -> f64 {
        self.m
    }

    /// Demagnetise.
    pub const fn reset(&mut self) {
        self.m = 0.0;
        self.h = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use super::*;

    #[test]
    fn the_langevin_table_matches_the_function() {
        let tape = Magnetic::default();
        assert!(tape.magnetisation().abs() < f64::EPSILON);
        for index in 0..=200_000 {
            let q = f64::from(index).mul_add(60.0 / 200_000.0, -30.0);
            compare(q);
        }
        for edge in [-Q_MAX, Q_MAX] {
            for q in [
                f64::from_bits(edge.to_bits() - 1),
                edge,
                f64::from_bits(edge.to_bits() + 1),
            ] {
                compare(q);
            }
        }
        assert_eq!(langevin(f64::NAN), (0.0, 1.0 / 3.0));
        assert_eq!(langevin(f64::INFINITY), (0.0, 1.0 / 3.0));
        assert_eq!(langevin(f64::NEG_INFINITY), (0.0, 1.0 / 3.0));
    }

    fn compare(q: f64) {
        let (value, slope) = langevin(q);
        let (exact, exact_slope) = langevin_exact(q);
        assert!((value - exact).abs() <= 1e-9, "L({q}): {value} vs {exact}");
        assert!(
            (slope - exact_slope).abs() <= 1e-6,
            "L'({q}): {slope} vs {exact_slope}"
        );
    }

    #[test]
    fn the_magnetics_null_against_the_exact_curve() {
        let input = crate::bench::fixture(48_000.0);
        for drive in [1.5, 16.0] {
            let mut tape = Magnetic::default();
            let mut exact = Magnetic::default();
            assert!(
                LANGEVIN.get().is_some(),
                "construction must prepare the table before processing"
            );
            let mut error = 0.0;
            let mut reference = 0.0;
            for &sample in &input {
                let value = f64::from(tape.process(sample, drive, 0.15));
                let old = f64::from(exact.process_with(sample, drive, 0.15, Langevin::Exact));
                error += (value - old).powi(2);
                reference += old * old;
            }
            let db = 10.0 * (error / reference).log10();
            eprintln!("drive {drive}: null {db} dB");
            assert!(db < -80.0, "drive {drive}: {db} dB");
        }
    }

    #[test]
    fn hard_drive_stays_bounded_and_finite() {
        let mut tape = Magnetic::default();
        assert!(
            LANGEVIN.get().is_some(),
            "construction must prepare the table before processing"
        );
        for frame in 0..48_000 {
            let input = if frame % 48 < 24 { 64.0 } else { -64.0 };
            let output = tape.process(input, 64.0, 1.0);
            assert!(output.is_finite(), "frame {frame}: {output}");
            let m = tape.magnetisation();
            assert!(m.is_finite() && m.abs() <= 1.0, "frame {frame}: m={m}");
        }
        for input in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.25] {
            assert!(tape.process(input, 64.0, 1.0).is_finite());
        }
    }

    #[test]
    fn biased_tape_is_unity_for_small_signals_and_compresses_large_ones() {
        let mut tape = Magnetic::default();
        let small: f32 = (0..4_800)
            .map(|n| {
                let x = 0.01 * (TAU * 100.0 * n as f32 / 48_000.0).sin();
                (tape.process(x, 1.5, 0.0) - x).abs()
            })
            .fold(0.0, f32::max);
        assert!(small < 1e-4, "{small}");
        let loud = tape.process(1.0, 1.5, 0.0);
        assert!(loud < 0.95 && loud > 0.8, "{loud}");
    }

    #[test]
    fn the_hysteresis_loop_lags_and_stays_bounded() {
        let mut tape = Magnetic::default();
        let mut rising = 0.0f32;
        let mut falling = 0.0f32;
        for n in 0..9_600 {
            let phase = TAU * 50.0 * n as f32 / 48_000.0;
            let x = phase.sin();
            let y = tape.process(x, 4.0, 1.0);
            assert!(y.is_finite() && y.abs() < 1.0, "{y}");
            // Where the field crosses zero, the loop is open: the output
            // still carries the sign of where it came from.
            if n > 4_800 && x.abs() < 0.01 {
                if phase.cos() > 0.0 {
                    rising = y;
                } else {
                    falling = y;
                }
            }
        }
        assert!(rising < -0.01 && falling > 0.01, "{rising} {falling}");
        for x in [1e9, -1e9, f32::NAN, 0.3] {
            assert!(tape.process(x, 64.0, 1.0).is_finite());
        }
    }

    #[test]
    fn the_curve_is_smooth_across_its_series_join() {
        // Both branches agree with the double-precision Langevin function
        // on either side of the join, so there is no step there.
        for q in [0.3f32, 0.45, 0.499_99, 0.5, 0.500_01, 0.55, 0.8, 3.0] {
            let exact = langevin_exact(f64::from(q)).0 as f32;
            let single = langevin_single(q);
            assert!((single - exact).abs() < 2e-7, "{q}: {single} vs {exact}");
            assert!((langevin_single(-q) + single).abs() < 1e-9, "odd at {q}");
        }
        let small = Magnetic::curve(1e-3, 1.5);
        assert!((small - 1e-3).abs() < 1e-8, "{small}");
        let big = Magnetic::curve(64.0, 64.0);
        assert!(big.is_finite() && big < 3.0 / 64.0 + 1e-6);
    }
}
