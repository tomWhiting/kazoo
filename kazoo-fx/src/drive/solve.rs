//! Solving the nonlinear parts of a circuit.
//!
//! A diode or a transistor makes a circuit's equation implicit: the voltage
//! across the part sets the current through it, which sets the voltage.
//! Every such equation in this family is reduced to one unknown with a
//! residual that only ever rises, so it is solved by Newton's method kept
//! inside a bracket that bisection shrinks whenever a Newton step would
//! leave it. That always converges, never divides by zero into the output,
//! and costs a fixed worst case, which is what an audio thread needs.

/// Room temperature thermal voltage, kT/q, in volts.
pub const THERMAL_VOLTAGE: f64 = 0.025_85;

/// The most steps a solve may take.
const ITERATIONS: usize = 48;

/// A step smaller than this (in volts or amps) ends the solve.
const TOLERANCE: f64 = 1e-9;

/// Exponents are held below this so nothing overflows f64.
const MAX_EXPONENT: f64 = 80.0;

/// The root of `residual` between `low` and `high`, starting from `guess`.
/// `residual` returns the value and slope at a point, and must rise across
/// the bracket (negative at `low`, positive at `high`). Whatever it
/// returns, the answer is finite and inside the bracket.
///
/// Newton steps are taken while they land inside the bracket and at least
/// halve the step before; otherwise the step bisects. That guard matters
/// for diodes: from far up an exponential, plain Newton creeps down by one
/// thermal voltage a step.
pub fn rising_root(low: f64, high: f64, guess: f64, residual: impl Fn(f64) -> (f64, f64)) -> f64 {
    rising_root_within(low, high, guess, TOLERANCE, residual)
}

/// [`rising_root`] to a chosen `tolerance`: the solve ends once a step is
/// smaller than it. Antiderivative anti-aliasing divides differences of
/// the answer by small input steps, so it asks for a much finer tolerance
/// than the default.
pub fn rising_root_within(
    low: f64,
    high: f64,
    guess: f64,
    tolerance: f64,
    residual: impl Fn(f64) -> (f64, f64),
) -> f64 {
    let (mut low, mut high) = if low <= high {
        (low, high)
    } else {
        (high, low)
    };
    if !low.is_finite() || !high.is_finite() {
        return 0.0;
    }
    let mut x = if guess.is_finite() {
        guess.clamp(low, high)
    } else {
        0.5 * (low + high)
    };
    let mut last_step = high - low;
    for _ in 0..ITERATIONS {
        let (value, slope) = residual(x);
        if value > 0.0 {
            high = x;
        } else if value < 0.0 {
            low = x;
        } else if !value.is_nan() {
            return x;
        }
        let newton = x - value / slope;
        let converging = (2.0 * value).abs() <= (last_step * slope).abs();
        let next = if newton > low && newton < high && converging {
            newton
        } else {
            0.5 * (low + high)
        };
        last_step = next - x;
        if last_step.abs() < tolerance {
            return next;
        }
        x = next;
    }
    x
}

/// A junction obeying Shockley's equation, `I = Is (e^(V / n Vt) - 1)`.
#[derive(Debug, Clone, Copy)]
pub struct Diode {
    saturation: f64,
    thermal: f64,
}

impl Diode {
    /// A diode with saturation current `saturation` amps and emission
    /// coefficient `emission`.
    #[must_use]
    pub const fn new(saturation: f64, emission: f64) -> Self {
        Self {
            saturation,
            thermal: emission * THERMAL_VOLTAGE,
        }
    }

    /// The current through two of these back to back (antiparallel) at
    /// `volts` across them, `2 Is sinh(V / n Vt)`, and its slope.
    #[must_use]
    pub fn pair(&self, volts: f64) -> (f64, f64) {
        // One exponential serves both: sinh and cosh are its odd and even
        // halves.
        let rising = (volts / self.thermal)
            .clamp(-MAX_EXPONENT, MAX_EXPONENT)
            .exp();
        let falling = 1.0 / rising;
        let current = self.saturation * (rising - falling);
        let slope = self.saturation * (rising + falling) / self.thermal;
        (current, slope)
    }

    /// The integral of [`Self::pair`]'s current from 0 to `volts`,
    /// `2 Is n Vt (cosh(V / n Vt) - 1)`: what antiderivative anti-aliasing
    /// of a diode clipper needs.
    #[must_use]
    pub fn pair_integral(&self, volts: f64) -> f64 {
        let exponent = (volts / self.thermal).clamp(-MAX_EXPONENT, MAX_EXPONENT);
        2.0 * self.saturation * self.thermal * (exponent.cosh() - 1.0)
    }
}

/// `e^x` with the exponent held where f64 cannot overflow.
#[must_use]
pub fn safe_exp(x: f64) -> f64 {
    x.clamp(-MAX_EXPONENT, MAX_EXPONENT).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_root_is_found_and_bounded() {
        let root = rising_root(-10.0, 10.0, 7.0, |x| {
            ((x * x).mul_add(x, -2.0), 3.0 * x * x)
        });
        assert!((root - 2f64.cbrt()).abs() < 1e-9, "{root}");
        let wild = rising_root(-1.0, 1.0, f64::NAN, |_| (f64::NAN, f64::NAN));
        assert!(wild.is_finite() && wild.abs() <= 1.0);
    }

    #[test]
    fn a_silicon_pair_conducts_around_six_tenths_of_a_volt() {
        let silicon = Diode::new(2.52e-9, 1.752);
        let volts = rising_root(0.0, 2.0, 0.5, |v| {
            let (i, g) = silicon.pair(v);
            (i - 1e-3, g)
        });
        assert!((0.55..0.65).contains(&volts), "{volts}");
    }
}
