//! Power matching follows with an 80 ms time constant. Gain falls at most
//! 60 dB per 100 ms and rises at most 12 dB per 100 ms, leaving the
//! dividers' continuous phase untouched.

#[derive(Debug, Clone, Copy)]
pub(super) struct LevelMatch {
    input_power: f64,
    output_power: f64,
    coefficient: f64,
    slew: f64,
    fall_slew: f64,
    gain: f64,
}

impl Default for LevelMatch {
    fn default() -> Self {
        Self {
            input_power: 0.0,
            output_power: 0.0,
            coefficient: 0.0,
            slew: 1.0,
            fall_slew: 1.0,
            gain: 1.0,
        }
    }
}

impl LevelMatch {
    pub(super) fn prepare(&mut self, rate: f32) {
        self.coefficient = -(-1.0 / (0.080 * f64::from(rate))).exp_m1();
        self.slew = 10.0_f64.powf(6.0 / f64::from(rate));
        self.fall_slew = 10.0_f64.powf(30.0 / f64::from(rate));
        self.reset();
    }

    pub(super) const fn reset(&mut self) {
        self.input_power = 0.0;
        self.output_power = 0.0;
        self.gain = 1.0;
    }

    #[cfg(test)]
    pub(super) fn gain_db(&self) -> f64 {
        20.0 * self.gain.log10()
    }

    pub(super) fn sample(&mut self, input: f64, output: f64) -> f64 {
        self.input_power = self
            .coefficient
            .mul_add(input - self.input_power, self.input_power);
        self.output_power = self
            .coefficient
            .mul_add(output - self.output_power, self.output_power);
        if !self.input_power.is_finite() || !self.output_power.is_finite() {
            self.reset();
            return self.gain;
        }
        // Hold the correction through silence rather than amplifying a note's tail.
        if input < 1e-24 {
            return self.gain;
        }
        let target = if self.output_power > 0.0 {
            (self.input_power / self.output_power).sqrt()
        } else {
            3.981_071_705_534_972_2
        };
        self.gain = target
            .clamp(self.gain / self.fall_slew, self.gain * self.slew)
            .clamp(0.501_187_233_627_272_2, 3.981_071_705_534_972_2);
        self.gain
    }
}

#[cfg(test)]
mod tests {
    use super::LevelMatch;

    #[test]
    fn gain_falls_fast_and_rises_slow() {
        for rate in [48_000.0, 96_000.0] {
            let mut matcher = LevelMatch::default();
            matcher.prepare(rate);
            let frames = (rate * 0.010) as usize;
            let fall_step = 60.0 / (0.100 * f64::from(rate));
            for _ in 0..frames {
                let before = matcher.gain_db();
                matcher.sample(1.0, 1000.0);
                let drop = before - matcher.gain_db();
                assert!(drop >= 0.0 && drop <= fall_step + 1e-10, "drop: {drop}");
            }
            let low = matcher.gain_db();
            assert!((low + 6.0).abs() < 1e-8, "10 ms fall: {low} dB");

            matcher.input_power = 1.0;
            matcher.output_power = 0.001;
            let rise_step = 12.0 / (0.100 * f64::from(rate));
            for _ in 0..frames {
                let before = matcher.gain_db();
                matcher.sample(1.0, 0.001);
                let rise = matcher.gain_db() - before;
                assert!(rise >= 0.0 && rise <= rise_step + 1e-10, "rise: {rise}");
            }
            let rise = matcher.gain_db() - low;
            assert!((rise - 1.2).abs() < 1e-8, "10 ms rise: {rise} dB");
        }
    }

    #[test]
    fn matching_is_bounded_slewed_and_held_in_silence() {
        let mut matcher = LevelMatch::default();
        matcher.prepare(48_000.0);
        for _ in 0..2400 {
            matcher.sample(1.0, 0.001);
        }
        let gain_db = 20.0 * matcher.gain.log10();
        assert!((gain_db - 6.0).abs() < 1e-8, "slew: {gain_db} dB");
        for _ in 0..9600 {
            matcher.sample(1.0, 0.001);
        }
        let gain_db = 20.0 * matcher.gain.log10();
        assert!((gain_db - 12.0).abs() < 1e-8, "ceiling: {gain_db} dB");
        let held = matcher.gain;
        for _ in 0..48_000 {
            assert_eq!(matcher.sample(0.0, 1e-20).to_bits(), held.to_bits());
        }
        for _ in 0..48_000 {
            matcher.sample(1.0, 1000.0);
        }
        let gain_db = 20.0 * matcher.gain.log10();
        assert!((gain_db + 6.0).abs() < 1e-8, "floor: {gain_db} dB");
    }

    #[test]
    fn either_nonfinite_follower_resets_the_gain() {
        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for input_invalid in [false, true] {
                let mut matcher = LevelMatch::default();
                matcher.prepare(48_000.0);
                for _ in 0..4800 {
                    matcher.sample(1.0, 0.1);
                }
                let (input, output) = if input_invalid {
                    (invalid, 1.0)
                } else {
                    (1.0, invalid)
                };
                assert_eq!(matcher.sample(input, output).to_bits(), 1.0_f64.to_bits());
                assert!(matcher.sample(1.0, 1.0).is_finite());
            }
        }
    }
}
