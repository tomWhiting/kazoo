use kazoo_core::sanitize_sample;
use kazoo_fx::{Context, Curve, Effect, EffectKind, ParamSpec};

use super::{MAX_PARAMS, RAMP_SECONDS, Ramp};

pub const PRESETS: [&str; 9] = [
    "octave down",
    "octave up",
    "octaves",
    "two down",
    "fifth",
    "power",
    "major third",
    "minor third",
    "custom",
];
pub const DEFAULT: usize = 2;
pub const CUSTOM: usize = 8;
pub const KNOBS: [[f32; 7]; 8] = [
    [0.0, -12.0, 0.0, 12.0, -60.0, 7.0, -60.0],
    [0.0, 12.0, -3.0, 12.0, -60.0, 7.0, -60.0],
    [0.0, -12.0, 0.0, 12.0, -3.0, 7.0, -60.0],
    [0.0, -12.0, 0.0, -24.0, -3.0, 7.0, -60.0],
    [0.0, 7.0, -3.0, 12.0, -60.0, 7.0, -60.0],
    [0.0, -12.0, 0.0, 7.0, -3.0, 7.0, -60.0],
    [0.0, 4.0, -3.0, 7.0, -6.0, 7.0, -60.0],
    [0.0, 3.0, -3.0, 7.0, -6.0, 7.0, -60.0],
];

const fn level(name: &'static str, default: f32) -> ParamSpec {
    ParamSpec {
        name,
        min: -60.0,
        max: 6.0,
        default,
        unit: "dB",
        curve: Curve::Linear,
    }
}
const fn shift(name: &'static str, default: f32) -> ParamSpec {
    ParamSpec {
        name,
        min: -24.0,
        max: 24.0,
        default,
        unit: "st",
        curve: Curve::Linear,
    }
}
static PARAMS: [ParamSpec; 9] = [
    ParamSpec {
        name: "preset",
        min: 0.0,
        max: 8.0,
        default: DEFAULT as f32,
        unit: "",
        curve: Curve::Stepped { labels: &PRESETS },
    },
    level("dry", 0.0),
    shift("voice 1 shift", -12.0),
    level("voice 1 level", 0.0),
    shift("voice 2 shift", 12.0),
    level("voice 2 level", -3.0),
    shift("voice 3 shift", 7.0),
    level("voice 3 level", -60.0),
    ParamSpec {
        name: "formant",
        min: 0.0,
        max: 1.0,
        default: 1.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["move", "keep"],
        },
    },
];

pub static KIND: EffectKind = EffectKind {
    id: "pitch",
    name: "Harmony",
    description: "Three shifted copies beside the dry voice. Copies arrive 13 to 25 ms late within an octave; up to 50 ms two octaves up. Dry has no added latency. Enter shows this help.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Effect> {
    Box::new(Harmony::new(128))
}

pub fn adjust(values: &mut [f32; MAX_PARAMS], custom: &mut [f32; 7], index: usize, value: f32) {
    values[index] = value;
    if index == 0 {
        let preset = value as usize;
        values[1..8].copy_from_slice(if preset == CUSTOM {
            custom
        } else {
            &KNOBS[preset]
        });
    } else if (1..8).contains(&index) {
        values[0] = CUSTOM as f32;
        custom.copy_from_slice(&values[1..8]);
    }
}

pub fn default_value(values: &[f32; MAX_PARAMS], index: usize) -> f32 {
    if (1..8).contains(&index) {
        KNOBS.get(values[0] as usize).unwrap_or(&KNOBS[DEFAULT])[index - 1]
    } else {
        PARAMS[index].default
    }
}

fn gain(db: f32) -> f32 {
    if db <= -60.0 {
        0.0
    } else {
        10.0_f32.powf(db / 20.0)
    }
}

#[derive(Debug)]
struct Voice {
    effect: Box<dyn Effect>,
    gain: Ramp,
    asleep: bool,
    ready: bool,
    wanted: f32,
    warmup: usize,
    #[cfg(test)]
    calls: usize,
}

#[derive(Debug)]
pub struct Harmony {
    voices: [Voice; 3],
    dry: Ramp,
    values: [f32; 9],
    scratch: [Box<[f32]>; 2],
}

impl Harmony {
    pub fn new(block: usize) -> Self {
        Self {
            voices: std::array::from_fn(|_| Voice {
                effect: (kazoo_fx::time::pitch::KIND.build)(),
                gain: Ramp::new(0.0, 1),
                asleep: true,
                ready: false,
                wanted: 0.0,
                warmup: 0,
                #[cfg(test)]
                calls: 0,
            }),
            dry: Ramp::new(1.0, 1),
            values: std::array::from_fn(|index| PARAMS[index].default),
            scratch: std::array::from_fn(|_| vec![0.0; block.max(1)].into_boxed_slice()),
        }
    }
}

impl Effect for Harmony {
    fn prepare(&mut self, rate: f32) {
        let frames = (rate * RAMP_SECONDS).round() as usize;
        self.dry = Ramp::new(gain(self.values[1]), frames);
        for (index, voice) in self.voices.iter_mut().enumerate() {
            voice.effect.prepare(rate);
            voice.effect.set_param(0, self.values[2 + 2 * index]);
            voice.effect.set_param(2, self.values[8]);
            voice.effect.set_param(3, 1.0);
            let target = gain(self.values[3 + 2 * index]);
            voice.gain = Ramp::new(0.0, frames);
            voice.wanted = target;
            voice.asleep = target == 0.0;
        }
        self.reset();
    }

    fn reset(&mut self) {
        self.dry.snap(gain(self.values[1]));
        for (index, voice) in self.voices.iter_mut().enumerate() {
            voice.effect.reset();
            let target = gain(self.values[3 + 2 * index]);
            voice.gain.snap(0.0);
            voice.wanted = target;
            voice.ready = false;
            voice.warmup = voice.gain.frames * 5;
            voice.asleep = target == 0.0;
        }
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let Some(spec) = PARAMS.get(index) else {
            return;
        };
        if !value.is_finite() {
            return;
        }
        let value = if matches!(index, 2 | 4 | 6) {
            spec.clamp(value).round()
        } else {
            spec.clamp(value)
        };
        if self.values[index].to_bits() == value.to_bits() {
            return;
        }
        self.values[index] = value;
        match index {
            1 => self.dry.aim(gain(value)),
            2 | 4 | 6 => self.voices[(index - 2) / 2].effect.set_param(0, value),
            3 | 5 | 7 => {
                let voice = &mut self.voices[(index - 3) / 2];
                let target = gain(value);
                if target > 0.0 && voice.asleep {
                    voice.effect.reset();
                    voice.asleep = false;
                    voice.ready = false;
                    voice.warmup = voice.gain.frames * 5;
                    voice.gain.snap(0.0);
                }
                voice.wanted = target;
                if voice.ready {
                    voice.gain.aim(target);
                }
            }
            8 => {
                for voice in &mut self.voices {
                    voice.effect.set_param(2, value);
                }
            }
            _ => {}
        }
    }

    fn process(&mut self, context: &Context, input: [&[f32]; 2], output: [&mut [f32]; 2]) {
        let [left, right] = output;
        let count = input[0]
            .len()
            .min(input[1].len())
            .min(left.len())
            .min(right.len());
        left.fill(0.0);
        right.fill(0.0);
        let mut start = 0;
        while start < count {
            let size = (count - start).min(self.scratch[0].len());
            let end = start + size;
            for i in start..end {
                let dry = self.dry.next();
                left[i] = dry * sanitize_sample(input[0][i]);
                right[i] = dry * sanitize_sample(input[1][i]);
            }
            for voice in &mut self.voices {
                if voice.asleep {
                    continue;
                }
                let [scratch_l, scratch_r] = &mut self.scratch;
                voice.effect.process(
                    context,
                    [&input[0][start..end], &input[1][start..end]],
                    [&mut scratch_l[..size], &mut scratch_r[..size]],
                );
                #[cfg(test)]
                {
                    voice.calls += 1;
                }
                for i in 0..size {
                    // Prime the longest shifted delay before fading in its startup transient.
                    voice.warmup = voice.warmup.saturating_sub(1);
                    if !voice.ready && voice.warmup == 0 {
                        voice.ready = true;
                        voice.gain.aim(voice.wanted);
                    }
                    let level = voice.gain.next();
                    left[start + i] = level.mul_add(sanitize_sample(scratch_l[i]), left[start + i]);
                    right[start + i] =
                        level.mul_add(sanitize_sample(scratch_r[i]), right[start + i]);
                }
                if !voice.gain.moving() && voice.wanted == 0.0 && voice.gain.value == 0.0 {
                    voice.asleep = true;
                }
            }
            start = end;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Harmony, RAMP_SECONDS};
    use kazoo_fx::{Context, Effect};

    #[test]
    fn harmony_off_voices_never_process_and_wake_without_stale_audio() {
        let mut harmony = Harmony::new(128);
        harmony.prepare(48_000.0);
        harmony.set_param(5, -60.0);
        harmony.reset();
        let mut left = [0.0; 128];
        let mut right = [0.0; 128];
        for _ in 0..1000 {
            harmony.process(
                &Context { bpm: 120.0 },
                [&[0.1; 128]; 2],
                [&mut left, &mut right],
            );
        }
        assert_eq!(harmony.voices[0].calls, 1000);
        assert_eq!(harmony.voices[1].calls, 0);
        assert_eq!(harmony.voices[2].calls, 0);
        harmony.set_param(5, 0.0);
        for _ in 0..100 {
            harmony.process(
                &Context { bpm: 120.0 },
                [&[0.1; 128]; 2],
                [&mut left, &mut right],
            );
        }
        harmony.set_param(5, -60.0);
        for _ in 0..4 {
            harmony.process(
                &Context { bpm: 120.0 },
                [&[0.0; 128]; 2],
                [&mut left, &mut right],
            );
        }
        let calls = harmony.voices[1].calls;
        harmony.process(
            &Context { bpm: 120.0 },
            [&[0.0; 128]; 2],
            [&mut left, &mut right],
        );
        assert_eq!(harmony.voices[1].calls, calls);
        harmony.set_param(5, 0.0);
        assert!(!harmony.voices[1].ready);
        harmony.set_param(3, -60.0);
        harmony.set_param(1, -60.0);
        for _ in 0..20 {
            harmony.process(
                &Context { bpm: 120.0 },
                [&[0.0; 128]; 2],
                [&mut left, &mut right],
            );
        }
        assert!(left.iter().chain(&right).all(|x| x.abs() < 1e-9));
    }

    #[test]
    fn harmony_level_changes_follow_the_sample_ramp() {
        let mut harmony = Harmony::new(128);
        harmony.prepare(48_000.0);
        harmony.set_param(3, -60.0);
        harmony.set_param(5, -60.0);
        harmony.reset();
        harmony.set_param(1, 6.0);
        let mut left = [0.0; 480];
        let mut right = [0.0; 480];
        harmony.process(
            &Context { bpm: 120.0 },
            [&[0.1; 480]; 2],
            [&mut left, &mut right],
        );
        let step = (10.0_f32.powf(6.0 / 20.0) - 1.0) / (48_000.0 * RAMP_SECONDS);
        assert!(
            left.windows(2)
                .all(|pair| (pair[1] - pair[0]).abs() <= step * 0.11)
        );
        assert!(
            left.chunks_exact(48)
                .zip(left[48..].chunks_exact(48))
                .all(|(a, b)| {
                    let rms =
                        |x: &[f32]| (x.iter().map(|x| x * x).sum::<f32>() / x.len() as f32).sqrt();
                    20.0 * (rms(b) / rms(a)).log10() <= 1.0
                })
        );
    }
}
