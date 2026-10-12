//! The valve curve and its integral, prepared away from the audio thread.

// The 12AX7, after Koren.
const MU: f64 = 100.0;
const EX: f64 = 1.4;
const KG1: f64 = 1060.0;
const KP: f64 = 600.0;
const KVB: f64 = 300.0;

// The gain stage around it.
const SUPPLY: f64 = 250.0;
const R_PLATE: f64 = 100e3;
const R_CATHODE: f64 = 1.5e3;
const GRID_LOW: f64 = -12.0;
const GRID_HIGH: f64 = 10.0;
const TABLE_SIZE: usize = 4_097;

/// Koren's plate current for the 12AX7, in amps.
fn plate_current(vgk: f64, vpk: f64) -> f64 {
    if vpk <= 0.0 {
        return 0.0;
    }
    let inner = KP * (1.0 / MU + vgk / vpk.mul_add(vpk, KVB).sqrt());
    let soft = if inner > 30.0 {
        inner
    } else {
        inner.exp().ln_1p()
    };
    let e1 = vpk / KP * soft;
    if e1 > 0.0 {
        2.0 * e1.powf(EX) / KG1
    } else {
        0.0
    }
}

/// The root of a falling function between `low` and `high`, by bisection.
fn falling_root(mut low: f64, mut high: f64, residual: impl Fn(f64) -> f64) -> f64 {
    for _ in 0..80 {
        let middle = 0.5 * (low + high);
        if residual(middle) > 0.0 {
            low = middle;
        } else {
            high = middle;
        }
    }
    0.5 * (low + high)
}

/// The cubic (Catmull-Rom) through a table segment, as coefficients of
/// `t`, `t²` and `t³` after the constant, for the segment from `x1` to
/// `x2` with neighbours `x0` and `x3`.
fn segment(x0: f64, x1: f64, x2: f64, x3: f64) -> [f64; 4] {
    [
        x1,
        0.5 * (x2 - x0),
        (-0.5f64).mul_add(x3, (-2.5f64).mul_add(x1, 2.0f64.mul_add(x2, x0))),
        0.5f64.mul_add(x3 - x0, 1.5 * (x1 - x2)),
    ]
}

/// A 12AX7 gain stage's plate swing for every grid voltage, and its
/// integral, for antiderivative anti-aliasing.
#[derive(Debug, Clone)]
pub(super) struct Triode {
    /// The plate's swing away from rest at each table step.
    table: Vec<f64>,
    /// The integral of the swing from the table's start to each step.
    integrals: Vec<f64>,
    /// Cubics prepared once so audio reads only evaluate the polynomial.
    segments: Vec<[f64; 4]>,
    /// The grid volts between table steps.
    step: f64,
    /// The cathode's resting voltage: the grid conducts above it.
    pub(super) cathode: f64,
    /// The table's slope at its low and high ends, in plate volts per grid
    /// volt, to carry it on past them.
    ends: [f64; 2],
}

impl Triode {
    /// Solve the stage. Allocates the tables.
    pub(super) fn new() -> Self {
        let current = falling_root(0.0, SUPPLY / (R_PLATE + R_CATHODE), |ip| {
            plate_current(-ip * R_CATHODE, ip.mul_add(-(R_PLATE + R_CATHODE), SUPPLY)) - ip
        });
        let cathode = current * R_CATHODE;
        let rest = current.mul_add(-R_PLATE, SUPPLY);
        let step = (GRID_HIGH - GRID_LOW) / (TABLE_SIZE - 1) as f64;
        let table: Vec<f64> = (0..TABLE_SIZE)
            .map(|i| {
                let grid = (i as f64).mul_add(step, GRID_LOW);
                let plate = falling_root(cathode, SUPPLY, |vp| {
                    (SUPPLY - vp) / R_PLATE - plate_current(grid - cathode, vp - cathode)
                });
                plate - rest
            })
            .collect();
        let last = TABLE_SIZE - 1;
        let ends = [
            (table[1] - table[0]) / step,
            (table[last] - table[last - 1]) / step,
        ];
        let segments: Vec<[f64; 4]> = (0..last)
            .map(|index| Self::coefficients(&table, index))
            .collect();
        let mut integrals = Vec::with_capacity(TABLE_SIZE);
        let mut total = 0.0;
        integrals.push(total);
        for &[c0, c1, c2, c3] in &segments {
            total += step * (c0 + c1 / 2.0 + c2 / 3.0 + c3 / 4.0);
            integrals.push(total);
        }
        Self {
            table,
            integrals,
            segments,
            step,
            cathode,
            ends,
        }
    }

    fn coefficients(table: &[f64], index: usize) -> [f64; 4] {
        let last = table.len() - 1;
        let at = |i: usize| table[i.min(last)];
        segment(
            at(index.saturating_sub(1)),
            at(index),
            at(index + 1),
            at(index + 2),
        )
    }

    /// Where `grid` falls in the table: the segment and how far along it.
    fn place(&self, grid: f64) -> (usize, f64) {
        let last = self.table.len() - 1;
        let place = ((grid - GRID_LOW) / self.step).clamp(0.0, last as f64);
        let index = (place.floor() as usize).min(last - 1);
        (index, place - index as f64)
    }

    /// The plate's swing for `grid` volts.
    pub(super) fn plate(&self, grid: f64) -> f64 {
        let last = self.table.len() - 1;
        if grid < GRID_LOW {
            return self.ends[0].mul_add(grid - GRID_LOW, self.table[0]);
        }
        if grid > GRID_HIGH {
            return self.ends[1].mul_add(grid - GRID_HIGH, self.table[last]);
        }
        let (index, t) = self.place(grid);
        let [c0, c1, c2, c3] = self.segments[index];
        c3.mul_add(t, c2).mul_add(t, c1).mul_add(t, c0)
    }

    /// The integral of the plate's swing from the table's start to
    /// `grid`.
    pub(super) fn integral(&self, grid: f64) -> f64 {
        let last = self.table.len() - 1;
        if grid < GRID_LOW {
            let over = grid - GRID_LOW;
            return (0.5 * self.ends[0] * over).mul_add(over, self.table[0] * over);
        }
        if grid > GRID_HIGH {
            let over = grid - GRID_HIGH;
            let ramp = (0.5 * self.ends[1] * over).mul_add(over, self.table[last] * over);
            return self.integrals[last] + ramp;
        }
        let (index, t) = self.place(grid);
        let [c0, c1, c2, c3] = self.segments[index];
        let within = (c3 / 4.0)
            .mul_add(t, c2 / 3.0)
            .mul_add(t, c1 / 2.0)
            .mul_add(t, c0)
            * t;
        self.step.mul_add(within, self.integrals[index])
    }

    /// The plate's mean swing over the step from `last` to `grid` volts:
    /// its antiderivative anti-aliased form.
    pub(super) fn smoothed(&self, grid: f64, last: f64) -> f64 {
        let step = grid - last;
        if step.abs() > 1e-6 {
            (self.integral(grid) - self.integral(last)) / step
        } else {
            self.plate(0.5 * (grid + last))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precomputed_segments_match_the_cubic() {
        let triode = Triode::new();
        assert_eq!(triode.segments.len(), TABLE_SIZE - 1);
        for i in 0..10_000 {
            let grid = (f64::from(i) / 9_999.0).mul_add(GRID_HIGH - GRID_LOW + 2.0, GRID_LOW - 1.0);
            let last = TABLE_SIZE - 1;
            let (plate, integral) = if grid < GRID_LOW {
                let over = grid - GRID_LOW;
                (
                    triode.ends[0].mul_add(over, triode.table[0]),
                    (0.5 * triode.ends[0] * over).mul_add(over, triode.table[0] * over),
                )
            } else if grid > GRID_HIGH {
                let over = grid - GRID_HIGH;
                (
                    triode.ends[1].mul_add(over, triode.table[last]),
                    triode.integrals[last]
                        + (0.5 * triode.ends[1] * over).mul_add(over, triode.table[last] * over),
                )
            } else {
                let (index, t) = triode.place(grid);
                let [c0, c1, c2, c3] = Triode::coefficients(&triode.table, index);
                let plate = c3.mul_add(t, c2).mul_add(t, c1).mul_add(t, c0);
                let within = (c3 / 4.0)
                    .mul_add(t, c2 / 3.0)
                    .mul_add(t, c1 / 2.0)
                    .mul_add(t, c0)
                    * t;
                (plate, triode.step.mul_add(within, triode.integrals[index]))
            };
            if (GRID_LOW..=GRID_HIGH).contains(&grid) {
                assert_eq!(
                    triode.plate(grid).to_bits(),
                    plate.to_bits(),
                    "plate at {grid}"
                );
                assert_eq!(
                    triode.integral(grid).to_bits(),
                    integral.to_bits(),
                    "integral at {grid}"
                );
            } else {
                assert!(
                    (triode.plate(grid) - plate).abs() <= 1e-12,
                    "plate at {grid}"
                );
                assert!(
                    (triode.integral(grid) - integral).abs() <= 1e-12,
                    "integral at {grid}"
                );
            }
        }
    }

    #[test]
    fn the_valve_stage_has_gain_and_rests_sensibly() {
        let triode = Triode::new();
        assert!((0.5..2.0).contains(&triode.cathode), "{}", triode.cathode);
        let slope = (triode.plate(0.01) - triode.plate(-0.01)) / 0.02;
        assert!((-80.0..-40.0).contains(&slope), "gain {slope}");
        assert!(triode.plate(GRID_LOW) > 50.0);
        assert!(triode.plate(GRID_HIGH) < -50.0);
        // By the table's top the plate has bottomed out, so carrying it on
        // in a straight line past the end leaves no corner.
        let top = (triode.plate(GRID_HIGH) - triode.plate(GRID_HIGH - 0.1)) / 0.1;
        let past = (triode.plate(GRID_HIGH + 1.0) - triode.plate(GRID_HIGH)) / 1.0;
        assert!(
            top.abs() < 1.0 && (top - past).abs() < 0.1,
            "{top} then {past}"
        );
        assert!((triode.plate(GRID_LOW - 1.0) - triode.plate(GRID_LOW)).abs() < 1e-3);
    }

    #[test]
    fn the_valve_integral_matches_its_curve() {
        let triode = Triode::new();
        let h = 1e-4;
        for i in -150..=150 {
            let grid = f64::from(i) * 0.1;
            let slope = (triode.integral(grid + h) - triode.integral(grid - h)) / (2.0 * h);
            let plate = triode.plate(grid);
            assert!(
                (slope - plate).abs() < 1e-3 * plate.abs().max(1.0),
                "{grid}: {slope} vs {plate}"
            );
        }
    }
}
