use super::VOICE_HOP;

/// Pick transients retain high frequencies that a ringing string has lost.
#[derive(Debug)]
pub struct Attack {
    b: [f32; 3],
    a: [f32; 2],
    state: [f32; 2],
    power: f32,
    input_power: f32,
    valley_db: f32,
    valley_rise: f32,
    level: f32,
    samples: usize,
    crossing: Option<usize>,
    behind: usize,
    trigger_power: f32,
}

impl Attack {
    pub fn new(rate: u32) -> Self {
        let rate = rate.max(1) as f32;
        let omega = std::f32::consts::TAU * 6000.0 / rate.max(12_001.0);
        let cosine = omega.cos();
        let alpha = omega.sin() * std::f32::consts::FRAC_1_SQRT_2;
        let normal = 1.0 / (1.0 + alpha);
        // Below this Nyquist there is no representable pick band.
        let b0 = if rate > 12_000.0 {
            (1.0 + cosine) * 0.5 * normal
        } else {
            0.0
        };
        Self {
            b: [b0, -2.0 * b0, b0],
            a: [-2.0 * cosine * normal, (1.0 - alpha) * normal],
            state: [0.0; 2],
            power: 0.0,
            input_power: 0.0,
            valley_db: -120.0,
            valley_rise: (-1000.0 * VOICE_HOP as f32 / (80.0 * rate)).exp(),
            level: 0.0,
            samples: 0,
            crossing: None,
            behind: 0,
            trigger_power: 64.0 * 1e-12 * 10.0_f32.powf(0.8),
        }
    }

    pub fn push(&mut self, sample: f32) {
        self.input_power = sample.mul_add(sample, self.input_power);
        let filtered = self.b[0].mul_add(sample, self.state[0]);
        self.state[0] = self.b[1].mul_add(sample, self.a[0].mul_add(-filtered, self.state[1]));
        self.state[1] = self.b[2].mul_add(sample, -self.a[1] * filtered);
        self.power = filtered.mul_add(filtered, self.power);
        self.samples += 1;
        if self.crossing.is_none() && self.power >= self.trigger_power {
            self.crossing = Some(self.samples);
        }
    }

    pub fn frame(&mut self, release: f32) -> bool {
        // The broadband follower also bridges troughs within a cycle; raw
        // hop minima would count a ringing string's corners as fresh picks.
        self.level = (self.power / VOICE_HOP as f32)
            .sqrt()
            .max(self.level * release.powf(VOICE_HOP as f32));
        let db = (20.0 * self.level.max(1e-6).log10()).max(-120.0);
        self.power = 0.0;
        // A filter's tail after the input stops is not a new excitation.
        let attack = self.input_power > 0.0 && db - self.valley_db >= 8.0;
        self.input_power = 0.0;
        self.valley_db = if attack || db < self.valley_db {
            db
        } else {
            self.valley_rise
                .mul_add(self.valley_db, (1.0 - self.valley_rise) * db)
        };
        self.behind = self.samples - self.crossing.unwrap_or(self.samples);
        self.samples = 0;
        self.crossing = None;
        self.trigger_power = VOICE_HOP as f32 * 10.0_f32.powf((self.valley_db + 8.0) / 10.0);
        attack
    }

    pub const fn behind(&self) -> usize {
        self.behind
    }

    pub const fn reset(&mut self) {
        self.state = [0.0; 2];
        self.power = 0.0;
        self.input_power = 0.0;
        self.valley_db = -120.0;
        self.level = 0.0;
        self.samples = 0;
        self.crossing = None;
        self.behind = 0;
        self.trigger_power = 64.0 * 1e-12 * 6.309_573_7;
    }
}
