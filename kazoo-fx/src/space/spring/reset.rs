use super::{GLIDES, PARAMS, SPRING_COUNT, Spring, TENSION};

impl Spring {
    /// Settle every control as a freshly prepared tank holds it: each knob on
    /// its target, the third spring in or out as the springs knob says, and
    /// the derived settings worked out again. Allocates nothing.
    pub(super) fn reset_controls(&mut self) {
        self.knobs.prepare(self.rate, &GLIDES);
        // A tank has always started its tension at the knob's default and
        // slewed to the knob from there, under the speed limit; a reset
        // starts it from the same place, so a reset tank sounds like a fresh
        // one and a fresh one sounds as it always has.
        self.tension.snap(PARAMS[TENSION].default);
        self.third.snap(if self.knobs.target(SPRING_COUNT) >= 2.5 {
            1.0
        } else {
            0.0
        });
        if self.prepared {
            self.update();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PARAMS, Spring};
    use crate::Effect;
    use crate::dsp::Noise;
    use crate::space::testing::run_mono;

    fn settings() -> [[f32; 6]; 6] {
        let mut settings = [
            PARAMS.map(|spec| spec.default),
            PARAMS.map(|spec| spec.min),
            PARAMS.map(|spec| spec.max),
            [0.0; 6],
            [0.0; 6],
            [0.0; 6],
        ];
        for (knobs, seed) in settings[3..].iter_mut().zip([1, 2, 3]) {
            let mut noise = Noise::new(seed);
            for (knob, spec) in knobs.iter_mut().zip(&PARAMS) {
                *knob = (spec.max - spec.min).mul_add(0.5 * (noise.sample() + 1.0), spec.min);
            }
        }
        settings
    }

    fn build(rate: f32, knobs: &[f32; 6], seed: Option<u64>) -> Spring {
        let mut spring = Spring::new();
        for (index, &value) in knobs.iter().enumerate() {
            spring.set_param(index, value);
        }
        spring.prepare(rate);
        if let Some(seed) = seed {
            spring.set_seed(seed);
        }
        spring
    }

    fn input(frames: usize) -> Vec<f32> {
        let mut noise = Noise::new(31);
        let mut input: Vec<f32> = (0..frames).map(|_| noise.sample() * 0.2).collect();
        input[0] = 1.0;
        input
    }

    fn bits(render: (Vec<f32>, Vec<f32>)) -> Vec<u32> {
        render
            .0
            .into_iter()
            .chain(render.1)
            .map(f32::to_bits)
            .collect()
    }

    #[test]
    fn a_reset_spring_renders_like_a_fresh_one() {
        for (rate, frames) in [(44_100.0, 44_100), (48_000.0, 48_000)] {
            let input = input(frames);
            for (setting, knobs) in settings().iter().enumerate() {
                for seed in [Some(5), None] {
                    // The first render is a freshly built spring's.
                    let mut spring = build(rate, knobs, seed);
                    let fresh = bits(run_mono(&mut spring, &input));
                    assert_no_alloc::assert_no_alloc(|| spring.reset());
                    let restarted = bits(run_mono(&mut spring, &input));
                    assert_eq!(restarted.len(), fresh.len());
                    let mismatch = restarted.iter().zip(&fresh).position(|(a, b)| a != b);
                    assert_eq!(
                        mismatch, None,
                        "reset: rate {rate}, setting {setting}, seed {seed:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn reset_settles_knobs_changed_while_running() {
        let input = input(48_000);
        let knobs = PARAMS.map(|spec| spec.max);
        let mut spring = build(48_000.0, &PARAMS.map(|spec| spec.default), Some(5));
        run_mono(&mut spring, &input[..256]);
        for (index, &value) in knobs.iter().enumerate() {
            spring.set_param(index, value);
        }
        assert_no_alloc::assert_no_alloc(|| spring.reset());
        let restarted = bits(run_mono(&mut spring, &input));
        let fresh = bits(run_mono(&mut build(48_000.0, &knobs, Some(5)), &input));
        assert_eq!(restarted.iter().zip(fresh).position(|(a, b)| *a != b), None);
    }

    #[test]
    fn unseeded_first_render_hashes() {
        let input = input(48_000);
        for (name, knobs, expected) in [
            (
                "default",
                PARAMS.map(|spec| spec.default),
                0x6134_a287_545a_3de0,
            ),
            (
                "maximum",
                PARAMS.map(|spec| spec.max),
                0x4e7c_74cb_52c0_51ed,
            ),
        ] {
            let render = bits(run_mono(&mut build(48_000.0, &knobs, None), &input));
            let hash = render
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
                    (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3)
                });
            println!("unseeded spring {name} 48000 Hz FNV-1a64 {hash:016x}");
            assert_eq!(hash, expected, "first render at {name} knobs");
        }
    }
}
