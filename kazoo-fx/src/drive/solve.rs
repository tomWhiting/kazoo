//! Solving the nonlinear parts of a circuit.
//!
//! A diode or a transistor makes a circuit's equation implicit: the voltage
//! across the part sets the current through it, which sets the voltage.
//! Every such equation in this family is reduced to one unknown with a
//! residual that only ever rises. The general solver keeps Newton steps
//! inside a bracket that bisection shrinks whenever a step would leave it.
//! Antiparallel diode pairs can instead use a prepared table seed with a
//! fixed Newton budget, bounding the cost on the audio thread.

/// Room temperature thermal voltage, kT/q, in volts.
pub const THERMAL_VOLTAGE: f64 = 0.025_85;

/// The most steps a solve may take.
const ITERATIONS: usize = 48;

/// A step smaller than this (in volts or amps) ends the solve.
const TOLERANCE: f64 = 1e-9;

/// Exponents are held below this so nothing overflows f64.
const MAX_EXPONENT: f64 = 80.0;

const T_POINTS: usize = 1025;
const MU_POINTS: usize = 17;
const T_MAX: f64 = 12.0;
const SINH_T_MAX: f64 = 81_377.395_706_429_86;
const Y_MAX: f64 = 40.0;

/// Seeds for `y + mu sinh(y) = xi`, refined with a fixed Newton budget.
#[derive(Debug, Default)]
pub struct SinhTable {
    values: Vec<f32>,
    log_low: f64,
    log_high: f64,
    log_scale: f64,
    cached_mu: f64,
    cached_log: f64,
}

impl SinhTable {
    /// Build the seeds off the audio thread for a positive range of `mu`.
    #[must_use]
    pub fn new(mu_low: f64, mu_high: f64) -> Self {
        assert!(
            mu_low.is_finite() && mu_high.is_finite() && mu_low > 0.0 && mu_high > mu_low,
            "the clipper table needs an increasing positive finite mu range"
        );
        let log_low = mu_low.ln();
        let log_high = mu_high.ln();
        assert!(
            log_high > log_low,
            "the clipper table needs distinct logarithmic endpoints"
        );
        let mut values = Vec::with_capacity(MU_POINTS * T_POINTS);
        for row in 0..MU_POINTS {
            let mu = (log_low + row as f64 * (log_high - log_low) / (MU_POINTS - 1) as f64).exp();
            let mut previous = -Y_MAX;
            for column in 0..T_POINTS {
                let t = -T_MAX + column as f64 * 2.0 * T_MAX / (T_POINTS - 1) as f64;
                let xi = t.sinh();
                previous = rising_root_within(-Y_MAX, Y_MAX, previous, 1e-12, |y| {
                    (mu.mul_add(y.sinh(), y) - xi, mu.mul_add(y.cosh(), 1.0))
                });
                values.push(previous as f32);
            }
        }
        Self {
            values,
            log_low,
            log_high,
            log_scale: (MU_POINTS - 1) as f64 / (log_high - log_low),
            cached_mu: 0.0,
            cached_log: 0.0,
        }
    }

    /// Share a frame's feedback strength across its oversampled solves.
    pub fn cache_mu(&mut self, mu: f64) {
        if mu.to_bits() != self.cached_mu.to_bits() {
            self.cached_mu = mu;
            self.cached_log = mu.ln();
        }
    }

    #[inline]
    fn log_mu(&self, mu: f64) -> f64 {
        if mu.to_bits() == self.cached_mu.to_bits() {
            self.cached_log
        } else {
            mu.ln()
        }
    }

    #[inline]
    fn seed(&self, xi: f64, mu: f64) -> f64 {
        self.seed_at(xi, self.log_mu(mu).clamp(self.log_low, self.log_high))
    }

    #[inline]
    fn seed_at(&self, xi: f64, log_mu: f64) -> f64 {
        let column =
            (xi.asinh().clamp(-T_MAX, T_MAX) + T_MAX) * (T_POINTS - 1) as f64 / (2.0 * T_MAX);
        let row = (log_mu - self.log_low) * self.log_scale;
        let low_column = (column as usize).min(T_POINTS - 2);
        let low_row = (row as usize).min(MU_POINTS - 2);
        let across = column - low_column as f64;
        let along = row - low_row as f64;
        let at = |r: usize| {
            let start = r * T_POINTS + low_column;
            let low = f64::from(self.values[start]);
            across.mul_add(f64::from(self.values[start + 1]) - low, low)
        };
        let low = at(low_row);
        along.mul_add(at(low_row + 1) - low, low)
    }

    /// Solve with exactly `STEPS` Newton updates and bounded exponents.
    /// The steps are written out, not looped, so the cost is fixed: a
    /// count of 0 to 3 compiles, and any other count is refused when
    /// the code is compiled rather than quietly doing fewer steps. Three
    /// serves a clipper whose antiderivative needs the root to 1e-13.
    /// A non-finite input or nonpositive `mu` produces silence.
    #[must_use]
    #[inline]
    pub fn solve<const STEPS: u32>(&self, xi: f64, mu: f64) -> f64 {
        const {
            assert!(STEPS <= 3, "the clipper solve has 0 to 3 Newton steps");
        }
        if !xi.is_finite() || !mu.is_finite() || mu <= 0.0 || self.values.is_empty() {
            return 0.0;
        }
        // Outside the axes, clamping the seed alone leaves too much error
        // for two steps. Continue the edge slope in log mu, and start
        // beyond the input axis from the exponential's inverse.
        let log_mu = self.log_mu(mu);
        let y = if xi.abs() > SINH_T_MAX {
            (xi / mu).asinh().clamp(-Y_MAX, Y_MAX)
        } else if log_mu < self.log_low || log_mu > self.log_high {
            self.seed_at(xi, log_mu).clamp(-Y_MAX, Y_MAX)
        } else {
            self.seed(xi, mu)
        };
        let update = |y: f64| {
            let rising = y.exp();
            let falling = 1.0 / rising;
            let sinh = 0.5 * (rising - falling);
            let cosh = 0.5 * (rising + falling);
            let (value, slope) = if mu >= 1.0 {
                ((y - xi) / mu + sinh, 1.0 / mu + cosh)
            } else {
                (mu.mul_add(sinh, y) - xi, mu.mul_add(cosh, 1.0))
            };
            (y - value / slope).clamp(-Y_MAX, Y_MAX)
        };
        match STEPS {
            0 => y,
            1 => update(y),
            2 => update(update(y)),
            _ => update(update(update(y))),
        }
    }
}

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

    /// Dimensionless feedback strength for a resistance of `reach` ohms.
    #[must_use]
    pub fn clip_mu(&self, reach: f64) -> f64 {
        2.0 * self.saturation * reach / self.thermal
    }

    /// Voltage satisfying `v + reach I(v) = open` for the antiparallel pair.
    #[must_use]
    #[inline]
    pub fn clip_volts<const STEPS: u32>(&self, table: &SinhTable, reach: f64, open: f64) -> f64 {
        self.thermal * table.solve::<STEPS>(open / self.thermal, self.clip_mu(reach))
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
    fn the_input_axis_bound_matches_its_transform() {
        assert_eq!(SINH_T_MAX.to_bits(), T_MAX.sinh().to_bits());
    }

    const MU_LOW: f64 = 1.5e-3;
    const MU_HIGH: f64 = 2.6e-3;

    fn loop_solution(xi: f64, mu: f64) -> f64 {
        rising_root_within(-40.0, 40.0, xi.asinh(), 1e-12, |y| {
            (mu.mul_add(y.sinh(), y) - xi, mu.mul_add(y.cosh(), 1.0))
        })
    }

    fn grid(mut check: impl FnMut(f64, f64)) {
        for row in 0..9 {
            let mu = (f64::from(row) / 8.0)
                .mul_add((MU_HIGH / MU_LOW).ln(), MU_LOW.ln())
                .exp();
            for column in 0..2001 {
                let t = -12.0 + f64::from(column) * 24.0 / 2000.0;
                check(t.sinh(), mu);
            }
        }
    }

    #[test]
    fn the_table_seed_is_within_a_thousandth() {
        let table = SinhTable::new(MU_LOW, MU_HIGH);
        grid(|xi, mu| {
            let reference = loop_solution(xi, mu);
            let error = (table.seed(xi, mu) - reference).abs();
            assert!(
                error <= 1e-3 * reference.abs().max(1.0),
                "xi {xi}, mu {mu}, error {error}"
            );
        });
    }

    #[test]
    fn two_newton_steps_reach_the_loop() {
        let table = SinhTable::new(MU_LOW, MU_HIGH);
        grid(|xi, mu| {
            let error = (table.solve::<2>(xi, mu) - loop_solution(xi, mu)).abs();
            assert!(error <= 2e-8, "xi {xi}, mu {mu}, error {error}");
            for outside in [MU_LOW / 2.0, MU_HIGH * 2.0] {
                let error = (table.solve::<2>(xi, outside) - loop_solution(xi, outside)).abs();
                assert!(error <= 1e-6, "xi {xi}, mu {outside}, error {error}");
            }
        });
        for mu in [MU_LOW / 2.0, MU_LOW, MU_HIGH, MU_HIGH * 2.0] {
            for t in [-14.0f64, -12.0, -6.0, 0.0, 6.0, 12.0, 14.0] {
                let xi = t.sinh();
                let value = table.solve::<2>(xi, mu);
                let error = (value - loop_solution(xi, mu)).abs();
                assert!(
                    value.is_finite() && error <= 1e-6,
                    "t {t}, mu {mu}, error {error}"
                );
            }
        }
    }

    #[test]
    fn nonsense_in_the_solve_is_silence() {
        let table = SinhTable::new(MU_LOW, MU_HIGH);
        for (xi, mu) in [
            (f64::NAN, MU_LOW),
            (1.0, f64::INFINITY),
            (f64::INFINITY, MU_LOW),
        ] {
            assert_eq!(table.solve::<2>(xi, mu).to_bits(), 0.0f64.to_bits());
        }
        for xi in [f64::MIN, -1.0, 0.0, 1.0, f64::MAX] {
            for mu in [f64::MIN_POSITIVE, MU_LOW, f64::MAX] {
                assert!(table.solve::<2>(xi, mu).is_finite());
            }
        }
    }

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
