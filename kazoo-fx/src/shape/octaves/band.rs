//! One analytic band and its continuous half-angle dividers.

use std::f64::consts::{PI, SQRT_2, TAU};

use crate::dsp::flush;

#[derive(Debug, Clone, Copy, Default)]
struct Complex {
    real: f32,
    imag: f32,
}

impl Complex {
    fn rotated(scale: f64, angle: f64) -> Self {
        let (sin, cos) = angle.sin_cos();
        Self {
            real: (scale * cos) as f32,
            imag: (scale * sin) as f32,
        }
    }

    fn product(self, other: Self) -> Self {
        Self {
            real: self.real.mul_add(other.real, -self.imag * other.imag),
            imag: self.real.mul_add(other.imag, self.imag * other.real),
        }
    }

    fn twice_phase(self, inverse_magnitude: f32) -> Self {
        Self {
            real: self.real.mul_add(self.real, -self.imag * self.imag) * inverse_magnitude,
            imag: 2.0 * self.real * self.imag * inverse_magnitude,
        }
    }

    fn flush(&mut self) {
        flush(&mut self.real);
        flush(&mut self.imag);
    }
}

#[derive(Debug, Clone, Copy)]
struct Divider {
    negative: bool,
    sign: f32,
}

impl Divider {
    const fn new() -> Self {
        Self {
            negative: false,
            sign: 1.0,
        }
    }

    const fn track(&mut self, input: Complex) {
        let negative = input.imag.is_sign_negative();
        if input.real < 0.0 && negative != self.negative {
            self.sign = -self.sign;
        }
        self.negative = negative;
    }

    fn half_phase(&self, input: Complex, inverse_magnitude: f32) -> Complex {
        let fraction = (0.5 * input.real * inverse_magnitude).clamp(-0.5, 0.5);
        let cosine = (0.5 + fraction).sqrt();
        let sine = (0.5 - fraction).sqrt().copysign(input.imag);
        Complex {
            real: self.sign * input.real.mul_add(cosine, input.imag * sine),
            imag: self.sign * input.imag.mul_add(cosine, -input.real * sine),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Band {
    d0: f32,
    d1: Complex,
    d2: Complex,
    c1: Complex,
    c2: Complex,
    s1: Complex,
    s2: Complex,
    down1: Divider,
    down2: Divider,
}

impl Band {
    pub(super) const fn new() -> Self {
        let zero = Complex {
            real: 0.0,
            imag: 0.0,
        };
        Self {
            d0: 0.0,
            d1: zero,
            d2: zero,
            c1: zero,
            c2: zero,
            s1: zero,
            s2: zero,
            down1: Divider::new(),
            down2: Divider::new(),
        }
    }

    pub(super) fn design(&mut self, centre: f64, bandwidth: f64, rate: f64) {
        let omega = PI * bandwidth.min(rate * 0.9) / rate;
        let (sin, cos) = omega.sin_cos();
        let alpha = SQRT_2 * sin * 0.5;
        let a0 = 1.0 + alpha;
        // Half-angle evaluation avoids losing the numerator at high host rates.
        let gain = (omega * 0.5).sin().powi(2) / a0;
        let rotation = TAU * centre / rate;
        self.d0 = gain as f32;
        self.d1 = Complex::rotated(2.0 * gain, rotation);
        self.d2 = Complex::rotated(gain, 2.0 * rotation);
        self.c1 = Complex::rotated(-2.0 * cos / a0, rotation);
        self.c2 = Complex::rotated((1.0 - alpha) / a0, 2.0 * rotation);
    }

    pub(super) fn reset(&mut self) {
        self.s1 = Complex::default();
        self.s2 = Complex::default();
        self.down1 = Divider::new();
        self.down2 = Divider::new();
    }

    fn filter(&mut self, input: f32) -> Complex {
        let y = Complex {
            real: self.d0.mul_add(input, self.s2.real),
            imag: self.s2.imag,
        };
        let c1y = self.c1.product(y);
        let c2y = self.c2.product(y);
        self.s2 = Complex {
            real: self.d1.real.mul_add(input, self.s1.real - c1y.real),
            imag: self.d1.imag.mul_add(input, self.s1.imag - c1y.imag),
        };
        self.s1 = Complex {
            real: self.d2.real.mul_add(input, -c2y.real),
            imag: self.d2.imag.mul_add(input, -c2y.imag),
        };
        self.s1.flush();
        self.s2.flush();
        y
    }

    pub(super) fn sample(&mut self, input: f32, enabled: [bool; 4], audible: bool) -> [f32; 4] {
        let y = self.filter(input);
        let power = y.real.mul_add(y.real, y.imag * y.imag);
        if !power.is_finite() {
            self.reset();
            return [0.0; 4];
        }
        let up = enabled[0] || enabled[1];
        let down = enabled[2] || enabled[3];
        if power < 1e-9 || (!down && (!audible || !up)) {
            return [0.0; 4];
        }
        let inverse = 1.0 / power.sqrt();
        let mut paths = [0.0; 4];
        if audible && (enabled[0] || enabled[1]) {
            let up1 = y.twice_phase(inverse);
            if enabled[0] {
                paths[0] = up1.real;
            }
            if enabled[1] {
                let up1_power = up1.real.mul_add(up1.real, up1.imag * up1.imag);
                paths[1] = up1.twice_phase(1.0 / up1_power.max(1e-9).sqrt()).real;
            }
        }
        if enabled[2] || enabled[3] {
            self.down1.track(y);
            let down1 = self.down1.half_phase(y, inverse);
            if audible && enabled[2] {
                paths[2] = down1.real;
            }
            if enabled[3] {
                self.down2.track(down1);
                if audible {
                    let power = down1.real.mul_add(down1.real, down1.imag * down1.imag);
                    paths[3] = self
                        .down2
                        .half_phase(down1, 1.0 / power.max(1e-9).sqrt())
                        .real;
                }
            }
        }
        paths
    }

    /// Evaluate the stored coefficients, including their f32 rounding, off thread.
    pub(super) fn response(&self, frequency: f64, rate: f64) -> (f64, f64) {
        let angle = -TAU * frequency / rate;
        let rotate = |value: Complex, angle: f64| {
            let (sin, cos) = angle.sin_cos();
            (
                f64::from(value.real).mul_add(cos, -f64::from(value.imag) * sin),
                f64::from(value.real).mul_add(sin, f64::from(value.imag) * cos),
            )
        };
        let d1 = rotate(self.d1, angle);
        let d2 = rotate(self.d2, 2.0 * angle);
        let c1 = rotate(self.c1, angle);
        let c2 = rotate(self.c2, 2.0 * angle);
        let numerator = (f64::from(self.d0) + d1.0 + d2.0, d1.1 + d2.1);
        let denominator = (1.0 + c1.0 + c2.0, c1.1 + c2.1);
        let power = denominator
            .0
            .mul_add(denominator.0, denominator.1 * denominator.1);
        (
            numerator
                .0
                .mul_add(denominator.0, numerator.1 * denominator.1)
                / power,
            numerator
                .1
                .mul_add(denominator.0, -numerator.0 * denominator.1)
                / power,
        )
    }
}
