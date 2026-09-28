//! Second-order filters that keep their analogue shape all the way up to
//! Nyquist.
//!
//! The usual digital biquad (the RBJ cookbook, built with the bilinear
//! transform) squeezes the whole analogue frequency axis into the digital
//! one, so a bell or a shelf set high in the treble comes out narrower and
//! lopsided: "cramping". These designs follow Martin Vicanek's *Matched
//! Second Order Digital Filters* (2016) instead. The poles come from the
//! impulse-invariant mapping of the analogue poles, and the zeros are then
//! solved so the digital magnitude equals the analogue magnitude exactly at
//! three frequencies: DC, Nyquist and the filter's own centre. The response
//! in between follows the analogue curve closely, with no cramping.
//!
//! The analogue prototypes are the familiar ones from Robert
//! Bristow-Johnson's cookbook (the same bells and shelves every mixing desk
//! EQ is built on). Coefficients and state are `f64`: a low shelf at 20 Hz
//! and 192 kHz puts its poles a hair from the unit circle, where `f32`
//! rounding would add audible noise and drift.

use std::f64::consts::PI;

/// Biquad coefficients, normalised so `a0` is 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coeffs {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Coeffs {
    /// A filter that passes everything unchanged.
    pub const IDENTITY: Self = Self {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    /// The magnitude of the response at `angle`.
    #[must_use]
    pub fn magnitude_at(&self, angle: Angle) -> f64 {
        let Angle { s1, c1, s2, c2 } = angle;
        let num_re = self.b2.mul_add(c2, self.b1.mul_add(c1, self.b0));
        let num_im = -self.b2.mul_add(s2, self.b1 * s1);
        let den_re = self.a2.mul_add(c2, self.a1.mul_add(c1, 1.0));
        let den_im = -self.a2.mul_add(s2, self.a1 * s1);
        (num_re.hypot(num_im) / den_re.hypot(den_im).max(1e-300)).max(0.0)
    }

    /// The phase of the response at `angle`, in radians (negative for a
    /// lag), between -π and π.
    #[must_use]
    pub fn phase_at(&self, angle: Angle) -> f64 {
        let Angle { s1, c1, s2, c2 } = angle;
        let num_re = self.b2.mul_add(c2, self.b1.mul_add(c1, self.b0));
        let num_im = -self.b2.mul_add(s2, self.b1 * s1);
        let den_re = self.a2.mul_add(c2, self.a1.mul_add(c1, 1.0));
        let den_im = -self.a2.mul_add(s2, self.a1 * s1);
        let phase = num_im.atan2(num_re) - den_im.atan2(den_re);
        if phase > PI {
            2.0f64.mul_add(-PI, phase)
        } else if phase < -PI {
            2.0f64.mul_add(PI, phase)
        } else {
            phase
        }
    }
}

/// A frequency's sine and cosine, and its double's, for evaluating many
/// responses at the same point.
#[derive(Debug, Clone, Copy, Default)]
pub struct Angle {
    s1: f64,
    c1: f64,
    s2: f64,
    c2: f64,
}

impl Angle {
    /// The angle `w` radians per sample.
    #[must_use]
    pub fn new(w: f64) -> Self {
        let (s1, c1) = w.sin_cos();
        let (s2, c2) = (2.0 * w).sin_cos();
        Self { s1, c1, s2, c2 }
    }
}

/// A biquad in transposed direct form II, with `f64` state.
#[derive(Debug, Clone, Copy)]
pub struct Biquad {
    coeffs: Coeffs,
    s1: f64,
    s2: f64,
}

impl Default for Biquad {
    fn default() -> Self {
        Self {
            coeffs: Coeffs::IDENTITY,
            s1: 0.0,
            s2: 0.0,
        }
    }
}

impl Biquad {
    /// Use new coefficients, keeping the state (so a gliding design does not
    /// click). Non-finite coefficients are refused.
    pub fn set(&mut self, coeffs: Coeffs) {
        let all = [coeffs.b0, coeffs.b1, coeffs.b2, coeffs.a1, coeffs.a2];
        if all.iter().all(|c| c.is_finite()) {
            self.coeffs = coeffs;
        }
    }

    /// One sample through.
    pub fn process(&mut self, input: f64) -> f64 {
        let c = self.coeffs;
        let out = c.b0.mul_add(input, self.s1);
        self.s1 = c.b1.mul_add(input, (-c.a1).mul_add(out, self.s2));
        self.s2 = c.b2.mul_add(input, -c.a2 * out);
        if !(self.s1.is_finite() && self.s2.is_finite()) {
            self.s1 = 0.0;
            self.s2 = 0.0;
            return 0.0;
        }
        if self.s1.abs() < 1e-30 {
            self.s1 = 0.0;
        }
        if self.s2.abs() < 1e-30 {
            self.s2 = 0.0;
        }
        out
    }

    /// Forget the past.
    pub const fn reset(&mut self) {
        self.s1 = 0.0;
        self.s2 = 0.0;
    }
}

/// Radians per sample for `hz` at `sample_rate`, held just below Nyquist so
/// the three matching points stay distinct.
#[must_use]
pub fn omega(hz: f64, sample_rate: f64) -> f64 {
    (2.0 * PI * hz / sample_rate).clamp(1e-6, 0.98 * PI)
}

/// The denominator of the impulse-invariant image of an analogue pole pair
/// at `w` radians per sample with damping `zeta`.
fn poles(w: f64, zeta: f64) -> (f64, f64) {
    let decay = (-zeta * w).exp();
    let a1 = if zeta <= 1.0 {
        // Past Nyquist the ringing frequency would fold back; hold it there.
        let ring = (w * zeta.mul_add(-zeta, 1.0).sqrt()).min(PI);
        -2.0 * decay * ring.cos()
    } else {
        -2.0 * decay * (w * zeta.mul_add(zeta, -1.0).sqrt()).cosh()
    };
    (a1, decay * decay)
}

/// The three weights of Vicanek's magnitude basis at `w`.
fn basis(w: f64) -> (f64, f64, f64) {
    let s = (0.5 * w).sin();
    let phi1 = s * s;
    let phi0 = 1.0 - phi1;
    (phi0, phi1, 4.0 * phi0 * phi1)
}

/// Coefficients with poles `a1`, `a2` whose squared magnitude is `dc` at
/// DC, `nyquist` at Nyquist and `centre` at `w` (Vicanek's equations 20 to
/// 24).
fn match_magnitude(a1: f64, a2: f64, dc: f64, nyquist: f64, w: f64, centre: f64) -> Coeffs {
    // The denominator's squared magnitude in Vicanek's basis...
    let pole_dc = (1.0 + a1 + a2).powi(2);
    let pole_nyquist = (1.0 - a1 + a2).powi(2);
    let pole_cross = -4.0 * a2;
    let (phi0, phi1, phi2) = basis(w);
    // ...and the numerator's, from the three targets.
    let zero_dc = pole_dc * dc;
    let zero_nyquist = pole_nyquist * nyquist;
    let at_centre = pole_cross.mul_add(phi2, pole_dc.mul_add(phi0, pole_nyquist * phi1));
    let zero_cross =
        centre.mul_add(at_centre, -zero_dc.mul_add(phi0, zero_nyquist * phi1)) / phi2.max(1e-12);
    // Back from squared magnitudes to the minimum-phase numerator.
    let root_dc = zero_dc.max(0.0).sqrt();
    let root_nyquist = zero_nyquist.max(0.0).sqrt();
    let mean = 0.5 * (root_dc + root_nyquist);
    let b0 = 0.5 * (mean + mean.mul_add(mean, zero_cross).max(0.0).sqrt());
    let b1 = 0.5 * (root_dc - root_nyquist);
    let b2 = if b0.abs() > 1e-300 {
        -zero_cross / (4.0 * b0)
    } else {
        0.0
    };
    Coeffs { b0, b1, b2, a1, a2 }
}

/// The analogue frequency, as a ratio of the filter's centre, that sits at
/// Nyquist.
fn nyquist_ratio(w0: f64) -> f64 {
    PI / w0
}

/// A resonant lowpass: 12 dB per octave above `hz`, with `q` (0.7071 is
/// Butterworth, no peak).
#[must_use]
pub fn lowpass(hz: f64, q: f64, sample_rate: f64) -> Coeffs {
    let w0 = omega(hz, sample_rate);
    let q = q.max(0.1);
    let (a1, a2) = poles(w0, 0.5 / q);
    let magnitude = |x: f64| 1.0 / (x / q).mul_add(x / q, x.mul_add(-x, 1.0).powi(2));
    match_magnitude(
        a1,
        a2,
        1.0,
        magnitude(nyquist_ratio(w0)),
        w0,
        magnitude(1.0),
    )
}

/// A resonant highpass: 12 dB per octave below `hz`.
#[must_use]
pub fn highpass(hz: f64, q: f64, sample_rate: f64) -> Coeffs {
    let w0 = omega(hz, sample_rate);
    let q = q.max(0.1);
    let (a1, a2) = poles(w0, 0.5 / q);
    let magnitude = |x: f64| x.powi(4) / (x / q).mul_add(x / q, x.mul_add(-x, 1.0).powi(2));
    match_magnitude(
        a1,
        a2,
        0.0,
        magnitude(nyquist_ratio(w0)),
        w0,
        magnitude(1.0),
    )
}

/// A bell: `db` of boost or cut centred on `hz`, `q` wide.
#[must_use]
pub fn peaking(hz: f64, db: f64, q: f64, sample_rate: f64) -> Coeffs {
    let w0 = omega(hz, sample_rate);
    let q = q.max(0.05);
    let a = 10f64.powf(db / 40.0);
    let (a1, a2) = poles(w0, 0.5 / (a * q));
    let magnitude = |x: f64| {
        let base = x.mul_add(-x, 1.0).powi(2);
        (a * x / q).mul_add(a * x / q, base) / (x / (a * q)).mul_add(x / (a * q), base)
    };
    match_magnitude(
        a1,
        a2,
        1.0,
        magnitude(nyquist_ratio(w0)),
        w0,
        magnitude(1.0),
    )
}

/// The damping of a shelf's poles for slope `q`.
const fn shelf_zeta(q: f64) -> f64 {
    0.5 / q
}

/// A low shelf: `db` below `hz` (the half-gain point), `q` 0.7071 for the
/// steepest slope with no overshoot.
#[must_use]
pub fn low_shelf(hz: f64, db: f64, q: f64, sample_rate: f64) -> Coeffs {
    let w0 = omega(hz, sample_rate);
    let q = q.max(0.1);
    let a = 10f64.powf(db / 40.0);
    // The analogue poles sit at w0 / sqrt(A).
    let (a1, a2) = poles((w0 / a.sqrt()).min(0.98 * PI), shelf_zeta(q));
    let magnitude = |x: f64| {
        let x2 = x * x;
        let damping = a * x2 / q.powi(2);
        let num = (a - x2).mul_add(a - x2, damping);
        let den = a.mul_add(-x2, 1.0).mul_add(a.mul_add(-x2, 1.0), damping);
        a * a * num / den
    };
    match_magnitude(
        a1,
        a2,
        a.powi(4),
        magnitude(nyquist_ratio(w0)),
        w0,
        magnitude(1.0),
    )
}

/// A high shelf: `db` above `hz` (the half-gain point).
#[must_use]
pub fn high_shelf(hz: f64, db: f64, q: f64, sample_rate: f64) -> Coeffs {
    let w0 = omega(hz, sample_rate);
    let q = q.max(0.1);
    let a = 10f64.powf(db / 40.0);
    // The analogue poles sit at w0 * sqrt(A).
    let (a1, a2) = poles((w0 * a.sqrt()).min(0.98 * PI), shelf_zeta(q));
    let magnitude = |x: f64| {
        let x2 = x * x;
        let damping = a * x2 / q.powi(2);
        let num = a.mul_add(-x2, 1.0).mul_add(a.mul_add(-x2, 1.0), damping);
        let den = (a - x2).mul_add(a - x2, damping);
        a * a * num / den
    };
    match_magnitude(
        a1,
        a2,
        1.0,
        magnitude(nyquist_ratio(w0)),
        w0,
        magnitude(1.0),
    )
}

/// A first-order section, `(b0 + b1 z⁻¹) / (1 + a1 z⁻¹)`.
#[derive(Debug, Clone, Copy)]
pub struct OnePoleCoeffs {
    b0: f64,
    b1: f64,
    a1: f64,
}

impl OnePoleCoeffs {
    /// Passes everything unchanged.
    pub const IDENTITY: Self = Self {
        b0: 1.0,
        b1: 0.0,
        a1: 0.0,
    };
}

/// A first-order filter with `f64` state.
#[derive(Debug, Clone, Copy)]
pub struct FirstOrder {
    coeffs: OnePoleCoeffs,
    last_in: f64,
    last_out: f64,
}

impl Default for FirstOrder {
    fn default() -> Self {
        Self {
            coeffs: OnePoleCoeffs::IDENTITY,
            last_in: 0.0,
            last_out: 0.0,
        }
    }
}

impl FirstOrder {
    /// Use new coefficients, keeping the state. Non-finite ones are refused.
    pub const fn set(&mut self, coeffs: OnePoleCoeffs) {
        if coeffs.b0.is_finite() && coeffs.b1.is_finite() && coeffs.a1.is_finite() {
            self.coeffs = coeffs;
        }
    }

    /// One sample through.
    pub fn process(&mut self, input: f64) -> f64 {
        let c = self.coeffs;
        let out =
            c.b0.mul_add(input, c.b1.mul_add(self.last_in, -c.a1 * self.last_out));
        if !out.is_finite() {
            self.reset();
            return 0.0;
        }
        self.last_in = input;
        self.last_out = if out.abs() < 1e-30 { 0.0 } else { out };
        out
    }

    /// Forget the past.
    pub const fn reset(&mut self) {
        self.last_in = 0.0;
        self.last_out = 0.0;
    }
}

/// A gentle 6 dB-per-octave lowpass at `hz`. The pole is the
/// impulse-invariant image of the analogue one and the zero is matched so
/// the gain at Nyquist is the analogue filter's there, so it sounds the same
/// at every sample rate (the plain impulse-invariant one-pole lets through
/// several dB more in the top octave at 44.1 kHz than at 192 kHz).
#[must_use]
pub fn lowpass_first_order(hz: f64, sample_rate: f64) -> OnePoleCoeffs {
    let wc = 2.0 * PI * hz / sample_rate;
    let pole = (-wc).exp();
    let nyquist = 1.0 / (PI / wc).mul_add(PI / wc, 1.0).sqrt();
    let sum = 1.0 - pole;
    let difference = (1.0 + pole) * nyquist;
    OnePoleCoeffs {
        b0: 0.5 * (sum + difference),
        b1: 0.5 * (sum - difference),
        a1: -pole,
    }
}

/// A tilt around `pivot` Hz: `db` of treble up and the same of bass down
/// (half each side of the pivot), 6 dB per octave between. The pole is the
/// impulse-invariant image of the analogue one and the zero is matched so
/// the gains at DC and Nyquist are the analogue ones (Vicanek's matched
/// one-pole design).
#[must_use]
pub fn tilt(pivot: f64, db: f64, sample_rate: f64) -> OnePoleCoeffs {
    let w0 = 2.0 * PI * pivot / sample_rate;
    let g = 10f64.powf(db / 40.0);
    // H(s) = g (s + wz) / (s + wp), wz = w0 / g, wp = w0 g: 1/g at DC, g at
    // infinity and unity at the pivot.
    let wz = w0 / g;
    let wp = w0 * g;
    let pole = (-wp).exp();
    let dc = 1.0 / g;
    let nyquist = g * (PI.mul_add(PI, wz * wz) / PI.mul_add(PI, wp * wp)).sqrt();
    let sum = (1.0 - pole) * dc;
    let difference = (1.0 + pole) * nyquist;
    OnePoleCoeffs {
        b0: 0.5 * (sum + difference),
        b1: 0.5 * (sum - difference),
        a1: -pole,
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::FRAC_1_SQRT_2;

    use super::*;

    const RATE: f64 = 48_000.0;

    /// A one-sample delay's phase is minus the angle, wrapped into -π..π.
    #[test]
    fn phase_at_reads_a_delay() {
        let delay = Coeffs {
            b0: 0.0,
            b1: 1.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
        };
        for w in [0.01, 0.5, 1.0, 2.0, 3.0] {
            let phase = delay.phase_at(Angle::new(w));
            assert!((phase + w).abs() < 1e-12, "{w}: {phase}");
        }
        assert!(Coeffs::IDENTITY.phase_at(Angle::new(1.0)).abs() < 1e-12);
    }

    fn db_at(coeffs: &Coeffs, hz: f64) -> f64 {
        20.0 * coeffs
            .magnitude_at(Angle::new(2.0 * PI * hz / RATE))
            .log10()
    }

    #[test]
    fn a_bell_hits_its_gain_at_the_centre_even_near_nyquist() {
        for &(hz, db, q) in &[
            (100.0, 12.0, 1.0),
            (1_000.0, -9.0, 2.0),
            (15_000.0, 6.0, 0.7),
            (19_000.0, -12.0, 3.0),
        ] {
            let coeffs = peaking(hz, db, q, RATE);
            let got = db_at(&coeffs, hz);
            assert!((got - db).abs() < 0.05, "{hz} Hz: {got} dB, wanted {db}");
            assert!(db_at(&coeffs, 1.0).abs() < 0.2, "{hz} Hz bell moved DC");
        }
    }

    #[test]
    fn a_bell_at_zero_gain_is_flat() {
        let coeffs = peaking(3_000.0, 0.0, 1.0, RATE);
        for hz in [20.0, 300.0, 3_000.0, 12_000.0, 23_000.0] {
            assert!(db_at(&coeffs, hz).abs() < 1e-6);
        }
    }

    #[test]
    fn shelves_reach_their_gain_at_the_far_end_and_half_at_the_corner() {
        let low = low_shelf(200.0, 9.0, FRAC_1_SQRT_2, RATE);
        assert!((db_at(&low, 5.0) - 9.0).abs() < 0.05);
        assert!((db_at(&low, 200.0) - 4.5).abs() < 0.05);
        assert!(db_at(&low, 20_000.0).abs() < 0.05);
        let high = high_shelf(8_000.0, -6.0, FRAC_1_SQRT_2, RATE);
        assert!(db_at(&high, 50.0).abs() < 0.05);
        assert!((db_at(&high, 8_000.0) + 3.0).abs() < 0.05);
    }

    #[test]
    fn cuts_are_three_decibels_down_at_the_corner() {
        let hp = highpass(100.0, FRAC_1_SQRT_2, RATE);
        assert!((db_at(&hp, 100.0) + 3.01).abs() < 0.05);
        assert!(db_at(&hp, 5_000.0).abs() < 0.05);
        let lp = lowpass(12_000.0, FRAC_1_SQRT_2, RATE);
        assert!((db_at(&lp, 12_000.0) + 3.01).abs() < 0.05);
        assert!(db_at(&lp, 100.0).abs() < 0.05);
    }

    #[test]
    fn the_first_order_lowpass_is_three_decibels_down_at_its_corner() {
        for rate in [44_100.0, 192_000.0] {
            let mut filter = FirstOrder::default();
            filter.set(lowpass_first_order(1_000.0, rate));
            let (mut peak, mut n) = (0.0f64, 0);
            while n < rate as usize {
                let x = (2.0 * PI * 1_000.0 * n as f64 / rate).sin();
                let y = filter.process(x);
                if n > rate as usize / 2 {
                    peak = peak.max(y.abs());
                }
                n += 1;
            }
            assert!(
                20.0f64.mul_add(peak.log10(), 3.01).abs() < 0.1,
                "{rate}: {peak}"
            );
        }
    }

    #[test]
    fn the_tilt_is_flat_at_zero_and_pivots_at_its_centre() {
        let flat = tilt(1_000.0, 0.0, RATE);
        let mut filter = FirstOrder::default();
        filter.set(flat);
        let out: Vec<f64> = [1.0, -0.5, 0.25, 0.0]
            .iter()
            .map(|&x| filter.process(x))
            .collect();
        assert!(
            out.iter()
                .zip([1.0, -0.5, 0.25, 0.0])
                .all(|(a, b)| (a - b).abs() < 1e-12)
        );
    }
}
