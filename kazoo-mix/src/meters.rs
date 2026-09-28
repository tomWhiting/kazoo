//! Meter ballistics and scales for the desk.
//!
//! The audio callback accumulates peak and RMS into shared meters, which the
//! desk drains once per frame. These types turn those into what a console
//! shows: a VU needle with VU-style ballistics (a one-pole approximation of
//! the standard VU response, reaching 99% of a step in about 300 ms, driven by
//! RMS rather than a rectified average and without the mechanical overshoot of
//! a real movement), and peak meters with instant attack, a held peak marker
//! and a steady fall. They run on the UI thread and are driven by elapsed wall
//! time, so the look does not change with frame rate.

/// dBFS that reads 0 VU on the needle meters (the common −18 dBFS alignment).
pub const VU_REFERENCE_DBFS: f32 = -18.0;

/// Lowest VU mark on the needle scale.
pub const VU_MIN: f32 = -20.0;

/// Highest VU mark on the needle scale.
pub const VU_MAX: f32 = 3.0;

/// Lowest dBFS shown on the bar meters.
pub const METER_FLOOR_DBFS: f32 = -60.0;

/// VU integration time constant: 99% of a step in ~300 ms (300 ms / ln 100).
const VU_TAU_SECONDS: f32 = 0.065;

/// How long a peak marker holds before falling.
const PEAK_HOLD_SECONDS: f32 = 1.5;

/// Fall rate of the peak bar and marker, in dB per second.
const PEAK_FALL_DB_PER_SECOND: f32 = 24.0;

/// Bar-meter scale as `(dBFS, fraction of height)` breakpoints: more room near
/// the top where mixing decisions happen.
const METER_SCALE: [(f32, f32); 8] = [
    (METER_FLOOR_DBFS, 0.0),
    (-40.0, 0.15),
    (-30.0, 0.28),
    (-20.0, 0.45),
    (-12.0, 0.62),
    (-6.0, 0.78),
    (-3.0, 0.87),
    (0.0, 1.0),
];

/// Linear amplitude to dBFS, with silence (and garbage) at `f32::NEG_INFINITY`.
#[must_use]
pub fn linear_to_db(linear: f32) -> f32 {
    if linear.is_finite() && linear > 0.0 {
        20.0 * linear.log10()
    } else {
        f32::NEG_INFINITY
    }
}

/// Position of a dBFS value on a bar meter, 0 (floor) to 1 (0 dBFS and up).
#[must_use]
pub fn meter_fraction(dbfs: f32) -> f32 {
    if dbfs.is_nan() || dbfs <= METER_FLOOR_DBFS {
        return 0.0;
    }
    for pair in METER_SCALE.windows(2) {
        let (d0, f0) = pair[0];
        let (d1, f1) = pair[1];
        if dbfs <= d1 {
            let t = (dbfs - d0) / (d1 - d0);
            return (f1 - f0).mul_add(t, f0);
        }
    }
    1.0
}

/// Needle position (0 at −20 VU, 1 at +3 VU) for a VU reading.
///
/// A mechanical VU meter deflects in proportion to voltage, so the scale is
/// linear in amplitude, not in decibels: 0 VU lands at about 69% of the arc
/// and the top of the scale is stretched, as on the real thing.
#[must_use]
pub fn vu_needle_fraction(vu: f32) -> f32 {
    if vu.is_nan() {
        return 0.0;
    }
    let volts = |db: f32| 10.0_f32.powf(db / 20.0);
    let low = volts(VU_MIN);
    let high = volts(VU_MAX);
    let v = volts(vu.clamp(VU_MIN - 40.0, VU_MAX + 6.0));
    ((v - low) / (high - low)).clamp(0.0, 1.05)
}

/// A usable time step: non-finite or negative steps count as no time.
///
/// Any length of step is exact: the ballistics below are closed-form in
/// `dt`, so one long step (a large device buffer, a stalled terminal) lands
/// exactly where many short ones would.
const fn clamp_dt(dt_seconds: f32) -> f32 {
    if dt_seconds.is_finite() {
        dt_seconds.max(0.0)
    } else {
        0.0
    }
}

/// VU needle ballistics for one channel.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct VuMeter {
    level: f32,
}

impl VuMeter {
    /// Integrate a new RMS reading (linear amplitude) over `dt_seconds`.
    pub fn update(&mut self, rms: f32, dt_seconds: f32) {
        let rms = if rms.is_finite() { rms.max(0.0) } else { 0.0 };
        let dt = clamp_dt(dt_seconds);
        let alpha = 1.0 - (-dt / VU_TAU_SECONDS).exp();
        self.level = (rms - self.level).mul_add(alpha, self.level);
        if !self.level.is_finite() {
            self.level = 0.0;
        }
    }

    /// Current reading in VU (0 VU = [`VU_REFERENCE_DBFS`]).
    #[must_use]
    pub fn vu(&self) -> f32 {
        linear_to_db(self.level) - VU_REFERENCE_DBFS
    }

    /// Needle position for drawing.
    #[must_use]
    pub fn needle(&self) -> f32 {
        vu_needle_fraction(self.vu())
    }
}

/// Peak-program ballistics for one bar meter: instant attack, steady fall,
/// and a held peak marker.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PeakMeter {
    level_db: f32,
    hold_db: f32,
    hold_remaining: f32,
}

impl Default for PeakMeter {
    fn default() -> Self {
        Self {
            level_db: f32::NEG_INFINITY,
            hold_db: f32::NEG_INFINITY,
            hold_remaining: 0.0,
        }
    }
}

impl PeakMeter {
    /// Let `dt_seconds` pass, then take a new peak reading (linear
    /// amplitude): the bar and marker fall (the marker after its hold) over
    /// the elapsed time, and the reading lifts them instantly.
    pub fn update(&mut self, peak: f32, dt_seconds: f32) {
        let dt = clamp_dt(dt_seconds);
        let incoming = linear_to_db(peak);
        let fallen = PEAK_FALL_DB_PER_SECOND.mul_add(-dt, self.level_db);
        self.level_db = incoming.max(fallen);

        // The marker holds for what is left of its hold time, then falls for
        // the rest of the step.
        let falling = (dt - self.hold_remaining).max(0.0);
        self.hold_remaining = (self.hold_remaining - dt).max(0.0);
        self.hold_db = PEAK_FALL_DB_PER_SECOND
            .mul_add(-falling, self.hold_db)
            .max(fallen);
        if incoming >= self.hold_db {
            self.hold_db = incoming;
            self.hold_remaining = PEAK_HOLD_SECONDS;
        }

        if self.level_db < METER_FLOOR_DBFS - 20.0 {
            self.level_db = f32::NEG_INFINITY;
        }
        if self.hold_db < METER_FLOOR_DBFS - 20.0 {
            self.hold_db = f32::NEG_INFINITY;
        }
    }

    /// Current bar level in dBFS.
    #[must_use]
    pub const fn level_db(&self) -> f32 {
        self.level_db
    }

    /// Held peak marker in dBFS.
    #[must_use]
    pub const fn hold_db(&self) -> f32 {
        self.hold_db
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{assert_float_eq, floats_equal};

    #[test]
    fn meter_fraction_hits_every_breakpoint_and_clamps() {
        for (db, fraction) in METER_SCALE {
            assert!((meter_fraction(db) - fraction).abs() < 1e-5);
        }
        assert_float_eq(meter_fraction(f32::NEG_INFINITY), 0.0);
        assert_float_eq(meter_fraction(f32::NAN), 0.0);
        assert_float_eq(meter_fraction(12.0), 1.0);
    }

    #[test]
    fn meter_fraction_is_monotonic() {
        let mut last = -1.0;
        for step in 0..=700 {
            let f = meter_fraction((step as f32).mul_add(0.1, -70.0));
            assert!(f >= last);
            last = f;
        }
    }

    #[test]
    fn vu_needle_scale_matches_a_real_meter() {
        assert!(vu_needle_fraction(VU_MIN).abs() < 1e-5);
        assert!((vu_needle_fraction(VU_MAX) - 1.0).abs() < 1e-5);
        let zero = vu_needle_fraction(0.0);
        assert!((0.66..0.72).contains(&zero), "0 VU at {zero}");
        assert_float_eq(vu_needle_fraction(-80.0), 0.0);
        assert_float_eq(vu_needle_fraction(f32::NAN), 0.0);
    }

    #[test]
    fn vu_reaches_99_percent_in_about_300ms() {
        let mut vu = VuMeter::default();
        let step = 1.0 / 120.0;
        let mut updates = 0_u32;
        for _ in 0..120 {
            if vu.level >= 0.99 {
                break;
            }
            vu.update(1.0, step);
            updates += 1;
        }
        let t = updates as f32 * step;
        assert!(vu.level >= 0.99, "never settled");
        assert!((0.25..0.36).contains(&t), "settled at {t}s");
    }

    #[test]
    fn vu_reads_zero_at_reference_level() {
        let mut vu = VuMeter::default();
        let reference = 10.0_f32.powf(VU_REFERENCE_DBFS / 20.0);
        for _ in 0..200 {
            vu.update(reference, 0.02);
        }
        assert!(vu.vu().abs() < 0.05, "{}", vu.vu());
    }

    #[test]
    fn vu_ignores_garbage_input() {
        let mut vu = VuMeter::default();
        vu.update(f32::NAN, 0.1);
        vu.update(0.5, f32::INFINITY);
        assert!(vu.needle().is_finite());
    }

    #[test]
    fn peak_attacks_instantly_holds_then_falls() {
        let mut meter = PeakMeter::default();
        meter.update(1.0, 0.03);
        assert!(meter.level_db().abs() < 1e-5);
        assert!(meter.hold_db().abs() < 1e-5);

        // Bar falls at the fall rate while the marker holds.
        meter.update(0.0, 0.25);
        meter.update(0.0, 0.25);
        assert!((meter.level_db() + 12.0).abs() < 1e-3);
        assert!(meter.hold_db().abs() < 1e-5);

        // After the hold time the marker falls too, never below the bar.
        for _ in 0..10 {
            meter.update(0.0, 0.2);
        }
        assert!(meter.hold_db() < 0.0);
        assert!(meter.hold_db() >= meter.level_db());
    }

    #[test]
    fn peak_decays_to_silence() {
        let mut meter = PeakMeter::default();
        meter.update(0.5, 0.03);
        for _ in 0..100 {
            meter.update(0.0, 0.25);
        }
        assert_float_eq(meter.level_db(), f32::NEG_INFINITY);
        assert_float_eq(meter.hold_db(), f32::NEG_INFINITY);
        assert_float_eq(meter_fraction(meter.level_db()), 0.0);
    }

    /// One long step lands where many short ones do, so meters read the
    /// same whatever the device buffer or desk frame rate.
    #[test]
    fn long_steps_match_short_ones() {
        for total in [0.4_f32, 1.0, 2.2, 6.0] {
            let mut short_peak = PeakMeter::default();
            let mut long_peak = PeakMeter::default();
            let mut short_vu = VuMeter::default();
            let mut long_vu = VuMeter::default();
            short_peak.update(0.5, 0.0);
            long_peak.update(0.5, 0.0);
            short_vu.update(0.3, 1.0);
            long_vu.update(0.3, 1.0);
            let steps = 200;
            for _ in 0..steps {
                short_peak.update(0.0, total / steps as f32);
                short_vu.update(0.05, total / steps as f32);
            }
            long_peak.update(0.0, total);
            long_vu.update(0.05, total);
            let close = |a: f32, b: f32| floats_equal(a, b) || (a - b).abs() < 1e-2;
            assert!(
                close(short_peak.level_db(), long_peak.level_db()),
                "{total}"
            );
            assert!(
                close(short_peak.hold_db(), long_peak.hold_db()),
                "{total}: {} vs {}",
                short_peak.hold_db(),
                long_peak.hold_db()
            );
            assert!(close(short_vu.vu(), long_vu.vu()), "{total}");
        }
    }

    #[test]
    fn marker_falls_at_the_fall_rate_after_its_hold() {
        let mut meter = PeakMeter::default();
        meter.update(1.0, 0.0);
        meter.update(0.0, PEAK_HOLD_SECONDS);
        assert!(meter.hold_db().abs() < 1e-4);
        meter.update(0.0, 0.5);
        assert!((meter.hold_db() + 12.0).abs() < 1e-3, "{}", meter.hold_db());
    }
}
