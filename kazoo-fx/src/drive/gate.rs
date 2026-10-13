//! A stereo-linked gate whose short peak detector leaves bridging to the hold.

use super::kit::{DEFAULT_RATE, Knobs, Retune, ceiling, for_each_frame};
use crate::dsp::{OnePole, sane_rate};
use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

/// Pickup hum is normally below -60 dBFS, while soft picking exceeds -40.
const THRESHOLD: usize = 0;
/// Closing six decibels below opening keeps a decaying note from fluttering.
const HYSTERESIS: usize = 1;
/// A 0.3 ms ramp preserves the pick while avoiding an instantaneous gain jump.
const ATTACK: usize = 2;
/// Fifty milliseconds bridges the lowest bass note's 32.4 ms period.
const HOLD: usize = 3;
/// A 150 ms fade lets a guitar tail decay instead of stopping abruptly.
const RELEASE: usize = 4;
/// Ninety decibels of attenuation makes the closed gate effectively silent.
const RANGE: usize = 5;
/// A 20 Hz detector high-pass preserves bass; raising it rejects more hum.
const KEY: usize = 6;

static PARAMS: [ParamSpec; 7] = [
    ParamSpec {
        name: "threshold",
        min: -90.0,
        max: 0.0,
        default: -55.0,
        unit: "dB",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "hysteresis",
        min: 0.0,
        max: 20.0,
        default: 6.0,
        unit: "dB",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "attack",
        min: 0.05,
        max: 20.0,
        default: 0.3,
        unit: "ms",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "hold",
        min: 0.0,
        max: 500.0,
        default: 50.0,
        unit: "ms",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "release",
        min: 5.0,
        max: 2_000.0,
        default: 150.0,
        unit: "ms",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "range",
        min: -90.0,
        max: 0.0,
        default: -90.0,
        unit: "dB",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "key",
        min: 10.0,
        max: 500.0,
        default: 20.0,
        unit: "Hz",
        curve: Curve::Log,
    },
];

/// The gate's catalogue entry.
pub const KIND: EffectKind = EffectKind {
    id: "gate",
    name: "Noise gate",
    description: "A stereo-linked noise gate with hysteresis, hold, a detector high-pass and no lookahead.",
    params: &PARAMS,
    build: || Box::new(Gate::new()),
};

/// Convert decibels to the exponent of a base-two gain.
const DB_EXPONENT: f64 = std::f64::consts::LOG2_10 / 20.0;

#[derive(Debug, Clone, Copy, Default)]
struct Settings {
    opening: f64,
    closing: f64,
    attack_step: f64,
    release_step: f64,
    hold_samples: usize,
    range: f64,
}

#[derive(Debug)]
struct Gate {
    knobs: Knobs<7>,
    retune: Retune<7>,
    highpass: [OnePole; 2],
    rate: f32,
    detector_release: f64,
    envelope: f64,
    settings: Settings,
    open: bool,
    hold_left: usize,
    gain_db: f64,
}

impl Gate {
    fn new() -> Self {
        let knobs = Knobs::new(&PARAMS);
        let retune = Retune::new(knobs.values());
        let mut gate = Self {
            knobs,
            retune,
            highpass: [OnePole::default(); 2],
            rate: DEFAULT_RATE,
            detector_release: 0.0,
            envelope: 0.0,
            settings: Settings::default(),
            open: false,
            hold_left: 0,
            gain_db: -90.0,
        };
        gate.prepare(DEFAULT_RATE);
        gate
    }

    fn tune(&mut self, values: [f32; 7]) {
        let range = f64::from(values[RANGE]);
        let samples_per_ms = f64::from(self.rate) / 1_000.0;
        self.settings = Settings {
            opening: (f64::from(values[THRESHOLD]) * DB_EXPONENT).exp2(),
            closing: (f64::from(values[THRESHOLD] - values[HYSTERESIS]) * DB_EXPONENT).exp2(),
            attack_step: -range / (f64::from(values[ATTACK]) * samples_per_ms),
            release_step: -range / (f64::from(values[RELEASE]) * samples_per_ms),
            hold_samples: (f64::from(values[HOLD]) * samples_per_ms).round() as usize,
            range,
        };
        for highpass in &mut self.highpass {
            highpass.set_cutoff(values[KEY], self.rate);
        }
    }

    fn frame(&mut self, left: f32, right: f32) -> [f32; 2] {
        let values = self.knobs.step();
        if self.retune.due(values) {
            self.tune(values);
        }
        let detected = self.highpass[0]
            .highpass(left)
            .abs()
            .max(self.highpass[1].highpass(right).abs());
        self.envelope = f64::from(detected).max(self.envelope * self.detector_release);
        if self.envelope < 1e-20 {
            self.envelope = 0.0;
        }
        let settings = self.settings;
        if !self.open && self.envelope >= settings.opening {
            self.open = true;
            self.hold_left = settings.hold_samples;
        }
        if self.open {
            if self.envelope >= settings.closing {
                self.hold_left = settings.hold_samples;
            } else if self.hold_left > 0 {
                self.hold_left -= 1;
            } else {
                self.open = false;
            }
        }
        self.gain_db = if self.open {
            (self.gain_db + settings.attack_step).min(0.0)
        } else {
            (self.gain_db - settings.release_step).max(settings.range)
        }
        .clamp(settings.range, 0.0);
        let gain = (self.gain_db * DB_EXPONENT).exp2() as f32;
        [ceiling(left * gain), ceiling(right * gain)]
    }
}

impl Effect for Gate {
    fn prepare(&mut self, sample_rate: f32) {
        self.rate = sane_rate(sample_rate);
        self.knobs.prepare(self.rate);
        self.detector_release = (-1.0 / (0.01 * f64::from(self.rate))).exp();
        self.reset();
    }

    fn reset(&mut self) {
        self.knobs.settle();
        let values = self.knobs.values();
        self.retune.settle(values);
        self.tune(values);
        for highpass in &mut self.highpass {
            highpass.reset();
        }
        self.envelope = 0.0;
        self.open = false;
        self.hold_left = 0;
        self.gain_db = self.settings.range;
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if value.is_finite() {
            self.knobs.set(index, value);
        }
    }

    fn process(&mut self, _context: &Context, input: [&[f32]; 2], output: [&mut [f32]; 2]) {
        for_each_frame(input, output, |left, right| self.frame(left, right));
    }
}

#[cfg(test)]
mod tests {
    use super::super::testkit::{CONTEXT, RATE, render, render_mono, sine};
    use crate::dsp::OnePole;
    use crate::{Curve, Effect};

    fn built() -> Box<dyn Effect> {
        let Some(kind) = crate::find("gate") else {
            panic!("the catalogue has no gate");
        };
        let mut gate = (kind.build)();
        gate.prepare(RATE);
        gate
    }

    fn gain_db(input: f32, output: f32) -> f64 {
        20.0 * (f64::from(output / input).abs().max(1e-12)).log10()
    }

    #[test]
    fn it_opens_within_its_attack() {
        let mut gate = built();
        assert_eq!(gate.latency(), 0);
        let silence = vec![0.0; 480];
        assert!(render_mono(&mut *gate, &silence).iter().all(|s| *s == 0.0));
        let input = sine(1_000.0, 0.1, 0.01);
        let output = render_mono(&mut *gate, &input);
        let first = input
            .iter()
            .position(|s| s.abs() >= 10f32.powf(-55.0 / 20.0))
            .unwrap();
        let opened = input
            .iter()
            .zip(&output)
            .enumerate()
            .skip(first)
            .find(|(_, (x, y))| x.abs() > 1e-6 && gain_db(**x, **y) >= -1.0)
            .map(|(i, _)| i)
            .unwrap();
        assert!(
            opened - first <= 24,
            "attack took {} samples",
            opened - first
        );
    }

    #[test]
    fn it_holds_then_releases_to_its_range() {
        let mut gate = built();
        let mut input = sine(1_000.0, 0.1, 0.2);
        input.extend(std::iter::repeat_n(1e-6, 24_000));
        let output = render_mono(&mut *gate, &input);
        let mut hp = OnePole::default();
        hp.set_cutoff(20.0, RATE);
        let mut env = 0.0f64;
        let decay = (-1.0 / (0.01 * f64::from(RATE))).exp();
        let closing = 10f64.powf(-61.0 / 20.0);
        let mut last_active = 0;
        for (i, x) in input.iter().enumerate() {
            env = f64::from(hp.highpass(*x).abs()).max(env * decay);
            if env >= closing {
                last_active = i;
            }
        }
        let release = last_active + 2_400;
        let first_fade = output
            .iter()
            .enumerate()
            .skip(9_600)
            .find(|(i, y)| gain_db(input[*i], **y) < -0.001)
            .map(|(i, _)| i)
            .unwrap();
        assert!(
            first_fade.abs_diff(release + 1) <= 1,
            "last active {last_active}, first fade {first_fade}, expected {}",
            release + 1
        );
        assert!(
            output[9_600..=release]
                .iter()
                .all(|y| gain_db(1e-6, *y).abs() < 0.001)
        );
        let floor = output
            .iter()
            .enumerate()
            .skip(first_fade)
            .find(|(i, y)| gain_db(input[*i], **y) <= -89.999)
            .map(|(i, _)| i)
            .unwrap();
        assert!(
            floor.abs_diff(release + 7_200) <= 2,
            "release ended at {floor}"
        );
    }

    #[test]
    fn hysteresis_stops_flutter() {
        let mut gate = built();
        gate.set_param(0, -30.0);
        gate.reset();
        let input: Vec<f32> = sine(220.0, 0.1, 12.0)
            .into_iter()
            .enumerate()
            .map(|(i, x)| x * 10f32.powf(-3.0 * i as f32 / RATE / 20.0))
            .collect();
        let output = render_mono(&mut *gate, &input);
        let mut stage = 0;
        let mut transitions = Vec::new();
        for (x, y) in input
            .iter()
            .zip(&output)
            .skip(1_000)
            .filter(|(x, _)| x.abs() > 1e-8)
        {
            let gain = gain_db(*x, *y);
            let now = if gain > -0.001 {
                0
            } else if gain > -89.999 {
                1
            } else {
                2
            };
            assert!(now >= stage, "gate reopened from {stage} to {now}");
            if now != stage {
                transitions.push(now);
                stage = now;
            }
        }
        assert_eq!(transitions, [1, 2]);
    }

    #[test]
    fn the_lowest_bass_note_does_not_chatter() {
        let mut gate = built();
        let input = sine(30.87, 0.01, 2.0);
        let output = render_mono(&mut *gate, &input);
        for (i, (x, y)) in input.iter().zip(&output).enumerate().skip(480) {
            if x.abs() > 1e-8 {
                assert!(
                    gain_db(*x, *y) >= -0.1,
                    "B0 gain at sample {i}: {}",
                    gain_db(*x, *y)
                );
            }
        }
    }

    #[test]
    fn the_key_ignores_hum() {
        let mut gate = built();
        let input = sine(50.0, 10f32.powf(-45.0 / 20.0), 1.0);
        gate.set_param(6, 120.0);
        gate.reset();
        let high_key = render_mono(&mut *gate, &input);
        gate.set_param(6, 20.0);
        gate.reset();
        let low_key = render_mono(&mut *gate, &input);
        let high_gain = input
            .iter()
            .zip(&high_key)
            .skip(9_600)
            .filter(|(x, _)| x.abs() > 1e-8)
            .map(|(x, y)| gain_db(*x, *y))
            .fold(f64::NEG_INFINITY, f64::max);
        assert!(low_key[9_600..] == input[9_600..], "20 Hz key did not open");
        assert!(
            high_gain <= -89.99,
            "120 Hz key opened: gain {high_gain} dB"
        );
    }

    #[test]
    fn range_is_the_floor() {
        let mut gate = built();
        gate.set_param(5, -20.0);
        gate.reset();
        let input = sine(1_000.0, 1e-5, 0.02);
        let output = render_mono(&mut *gate, &input);
        for (x, y) in input.iter().zip(&output).filter(|(x, _)| x.abs() > 1e-9) {
            assert!((gain_db(*x, *y) + 20.0).abs() < 0.01);
        }
    }

    #[test]
    fn a_glide_and_a_vibrato_pass_untouched() {
        let mut gate = built();
        let mut phase = 0.0f64;
        let input: Vec<f32> = (0..192_000)
            .map(|i| {
                let t = f64::from(i) / f64::from(RATE);
                let hz = if t < 2.0 {
                    (293.66 - 220.0_f64).mul_add(t / 2.0, 220.0)
                } else {
                    293.66 * (30.0 * (std::f64::consts::TAU * 6.0 * t).sin() / 1_200.0).exp2()
                };
                phase += std::f64::consts::TAU * hz / f64::from(RATE);
                0.1 * phase.sin() as f32
            })
            .collect();
        let output = render_mono(&mut *gate, &input);
        assert_eq!(&output[48..], &input[48..]);
    }

    #[test]
    fn survives_nonsense() {
        let mut gate = built();
        for i in 0..7 {
            gate.set_param(i, f32::NAN);
        }
        gate.set_param(usize::MAX, f32::NAN);
        let input = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY];
        let output = render_mono(&mut *gate, &input);
        assert_eq!(output, [0.0; 3]);
        let tone = sine(1_000.0, 0.1, 0.5);
        let output = render_mono(&mut *gate, &tone);
        assert_eq!(&output[480..], &tone[480..]);
        gate.reset();
        let mut fresh = built();
        assert_eq!(
            render_mono(&mut *gate, &tone),
            render_mono(&mut *fresh, &tone)
        );
    }

    #[test]
    fn stays_bounded() {
        let mut gate = built();
        for i in 0..7 {
            gate.set_param(i, f32::INFINITY);
            gate.set_param(i, f32::NEG_INFINITY);
            gate.set_param(i, f32::NAN);
        }
        let input = vec![1e30; 48_000];
        let (left, right) = render(&mut *gate, &input, &input);
        assert!(
            left.iter()
                .chain(&right)
                .all(|x| x.is_finite() && x.abs() <= 2.0)
        );
        let silence = vec![0.0; 48_000];
        let output = render_mono(&mut *gate, &silence);
        assert!(output.iter().all(|x| *x == 0.0));
        let quiet = sine(1_000.0, 1e-5, 0.02);
        let output = render_mono(&mut *gate, &quiet);
        for (x, y) in quiet.iter().zip(&output).filter(|(x, _)| x.abs() > 1e-9) {
            assert!((gain_db(*x, *y) + 90.0).abs() < 0.01);
        }
        let clean = sine(1_000.0, 0.1, 0.02);
        let output = render_mono(&mut *gate, &clean);
        assert_eq!(&output[48..], &clean[48..]);
        gate.prepare(f32::NAN);
        let input = sine(1_000.0, 0.1, 0.02);
        assert!(
            render_mono(&mut *gate, &input)
                .iter()
                .all(|x| x.is_finite())
        );
    }

    #[test]
    fn processing_never_allocates() {
        let mut gate = built();
        let input = sine(220.0, 0.1, 10.0);
        let mut left = vec![0.0; input.len()];
        let mut right = vec![0.0; input.len()];
        assert_no_alloc::assert_no_alloc(|| {
            gate.process(&CONTEXT, [&input, &input], [&mut left, &mut right]);
            for i in 0..7 {
                gate.set_param(i, f32::NAN);
            }
            gate.reset();
        });
    }

    #[test]
    fn the_catalogue_offers_it() {
        let gate = built();
        assert_eq!(gate.latency(), 0);
        let kind = crate::find("gate").unwrap();
        let names: Vec<&str> = kind.params.iter().map(|p| p.name).collect();
        assert_eq!(
            names,
            [
                "threshold",
                "hysteresis",
                "attack",
                "hold",
                "release",
                "range",
                "key"
            ]
        );
        for (p, (min, max, default, curve)) in kind.params.iter().zip([
            (-90.0, 0.0, -55.0, Curve::Linear),
            (0.0, 20.0, 6.0, Curve::Linear),
            (0.05, 20.0, 0.3, Curve::Log),
            (0.0, 500.0, 50.0, Curve::Linear),
            (5.0, 2_000.0, 150.0, Curve::Log),
            (-90.0, 0.0, -90.0, Curve::Linear),
            (10.0, 500.0, 20.0, Curve::Log),
        ]) {
            assert_eq!(
                (p.min, p.max, p.default, p.curve),
                (min, max, default, curve)
            );
        }
        let signal = sine(1_000.0, 0.1, 0.02);
        let quiet = vec![1e-5; signal.len()];
        for input in [[&quiet[..], &signal[..]], [&signal[..], &quiet[..]]] {
            let mut gate = built();
            let (left, right) = render(&mut *gate, input[0], input[1]);
            assert_eq!(&left[48..], &input[0][48..]);
            assert_eq!(&right[48..], &input[1][48..]);
        }
    }
}
