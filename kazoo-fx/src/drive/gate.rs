//! Gate: a stereo-linked noise gate with no lookahead, for heavy gain.
//!
//! Through heavy distortion the hum and hiss of a pickup and its preamp
//! between notes come up as loud as the notes. In front of the drives this
//! silences them between notes.
//!
//! The detector hears the louder of the two channels through the `key`,
//! two one-pole high-passes in series (12 dB an octave) on the detector
//! only, so mains hum can be ignored without thinning the sound. A peak
//! follower with a 10 ms release reads it; the release is short because
//! the hold does the bridging. The gate opens when the follower reaches
//! the `threshold`; while open, every sample at or above the threshold
//! less the `hysteresis` restarts the `hold`. When the hold runs out the
//! gain falls towards `range`, and the gate opens again the moment the
//! follower reaches the threshold. The gain moves in decibels at a fixed
//! rate, so `attack` and `release` are the times for the whole travel
//! between 0 dB and `range`, and a release sounds like a fade.
//!
//! There is no lookahead and the latency is zero: a lookahead would cost
//! its length on every input's round trip. The 0.3 ms attack is the price.

use super::kit::{DEFAULT_RATE, Knobs, Retune, ceiling, for_each_frame};
use crate::dsp::{OnePole, sane_rate};
use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

/// `threshold`, -55 dB by default: a single-coil's hum through a preamp at
/// a sensible gain sits around -60 to -70 dBFS, and a soft pick attack is
/// above -40.
const THRESHOLD: usize = 0;
/// `hysteresis`, 6 dB by default: the gate closes 6 dB under where it
/// opened, so a note decaying through the threshold does not flutter.
const HYSTERESIS: usize = 1;
/// `attack`, 0.3 ms by default (about 15 samples): fast enough not to
/// blunt a pick, slow enough not to click.
const ATTACK: usize = 2;
/// `hold`, 50 ms by default: longer than one period of the lowest bass
/// note (B0, 32.4 ms), so the gaps between its peaks never start a release.
const HOLD: usize = 3;
/// `release`, 150 ms by default: a guitar's tail fades rather than stops.
const RELEASE: usize = 4;
/// `range`, -90 dB by default: closed means silent; less is a downward
/// expander, for those who want the room.
const RANGE: usize = 5;
/// `key`, 20 Hz by default: a high-pass on the detector only, two poles;
/// raise it to ignore mains hum (50 or 100 Hz) without thinning the sound.
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
    /// The detector's key, per channel: two one-pole high-passes in series.
    highpass: [[OnePole; 2]; 2],
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
            highpass: [[OnePole::default(); 2]; 2],
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
        for pole in self.highpass.iter_mut().flatten() {
            pole.set_cutoff(values[KEY], self.rate);
        }
    }

    fn frame(&mut self, left: f32, right: f32) -> [f32; 2] {
        let values = self.knobs.step();
        if self.retune.due(values) {
            self.tune(values);
        }
        let [key_left, key_right] = &mut self.highpass;
        let detected = keyed(key_left, left)
            .abs()
            .max(keyed(key_right, right).abs());
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

/// `sample` through one channel's key: two one-pole high-passes in series,
/// 12 dB an octave, so a key at 120 Hz takes 16.6 dB off 50 Hz mains hum.
fn keyed(poles: &mut [OnePole; 2], sample: f32) -> f32 {
    let [first, second] = poles;
    second.highpass(first.highpass(sample))
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
        for pole in self.highpass.iter_mut().flatten() {
            pole.reset();
        }
        self.envelope = 0.0;
        self.open = false;
        self.hold_left = 0;
        self.gain_db = self.settings.range;
    }

    fn set_param(&mut self, index: usize, value: f32) {
        self.knobs.set(index, value);
    }

    fn process(&mut self, _context: &Context, input: [&[f32]; 2], output: [&mut [f32]; 2]) {
        for_each_frame(input, output, |left, right| self.frame(left, right));
    }
}

#[cfg(test)]
mod tests {
    use super::super::testkit::{CONTEXT, Limits, RATE, conformance, render, render_mono, sine};
    use super::{KIND, THRESHOLD};
    use crate::dsp::OnePole;
    use crate::{Curve, Effect};

    conformance!(
        KIND,
        Limits {
            // Open, the gate multiplies by exactly one: nothing it adds.
            worst_spur_db: Some(-80.0),
            hot: &[],
            hot_spur_db: None,
            unity: true,
            // The threshold's way from one end to the other crosses the
            // tone's level, where the gate opens at its attack or closes at
            // its release: what it does to every note, rougher than either
            // end at rest, not a click in the knob's glide.
            rough_glides: &[THRESHOLD],
            latency_knobs: &[],
        }
    );

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
        let Some(first) = input
            .iter()
            .position(|s| s.abs() >= 10f32.powf(-55.0 / 20.0))
        else {
            panic!("the test tone never crosses the threshold");
        };
        let opened = input
            .iter()
            .zip(&output)
            .enumerate()
            .skip(first)
            .find(|(_, (x, y))| x.abs() > 1e-6 && gain_db(**x, **y) >= -1.0)
            .map_or(input.len(), |(i, _)| i);
        assert!(
            opened - first <= 24,
            "the gain took {} samples to reach -1 dB (the tone lasts {})",
            opened - first,
            input.len() - first
        );
    }

    #[test]
    fn it_holds_then_releases_to_its_range() {
        let mut gate = built();
        let mut input = sine(1_000.0, 0.1, 0.2);
        input.extend(std::iter::repeat_n(1e-6, 24_000));
        let output = render_mono(&mut *gate, &input);
        let mut key = [OnePole::default(); 2];
        for pole in &mut key {
            pole.set_cutoff(20.0, RATE);
        }
        let mut env = 0.0f64;
        let decay = (-1.0 / (0.01 * f64::from(RATE))).exp();
        let closing = 10f64.powf(-61.0 / 20.0);
        let mut last_active = 0;
        for (i, x) in input.iter().enumerate() {
            let once = key[0].highpass(*x);
            let keyed = key[1].highpass(once);
            env = f64::from(keyed.abs()).max(env * decay);
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
            .map_or(input.len(), |(i, _)| i);
        assert!(
            first_fade.abs_diff(release + 1) <= 1,
            "last active {last_active}, first fade {first_fade}, expected {}",
            release + 1
        );
        assert!(
            output[9_600..=release]
                .iter()
                .all(|y| gain_db(1e-6, *y).abs() < 0.001),
            "the gain left 0 dB during the hold"
        );
        let floor = output
            .iter()
            .enumerate()
            .skip(first_fade)
            .find(|(i, y)| gain_db(input[*i], **y) <= -89.999)
            .map_or(input.len(), |(i, _)| i);
        assert!(
            floor.abs_diff(release + 7_200) <= 2,
            "the release reached -90 dB at {floor}, expected {} (input ends at {})",
            release + 7_200,
            input.len()
        );
    }

    /// At the default hold the hold alone would bridge the detector's
    /// ripple; with no hold (`hold` 0) only the hysteresis keeps the 2 dB
    /// ripple between a 220 Hz tone's peaks from reopening the gate, so the
    /// test runs both.
    #[test]
    fn hysteresis_stops_flutter() {
        for hold in [50.0, 0.0] {
            decays_through_the_threshold_once(hold);
        }
    }

    fn decays_through_the_threshold_once(hold: f32) {
        let mut gate = built();
        gate.set_param(0, -30.0);
        gate.set_param(3, hold);
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
            assert!(
                now >= stage,
                "hold {hold} ms: gate reopened from {stage} to {now}"
            );
            if now != stage {
                transitions.push(now);
                stage = now;
            }
        }
        assert_eq!(transitions, [1, 2], "hold {hold} ms");
    }

    /// At -40 dBFS the B0's peak follower stays above the closing level
    /// even without hysteresis. At -46 dBFS with no hysteresis it dips
    /// below between peaks, so only the 50 ms hold keeps the gate open.
    /// A no-hold control must attenuate every settled cycle.
    #[test]
    fn the_lowest_bass_note_does_not_chatter() {
        for (hysteresis, level_db) in [(6.0, -40.0), (0.0, -40.0), (0.0, -46.0)] {
            let mut gate = built();
            gate.set_param(1, hysteresis);
            gate.reset();
            let input = sine(30.87, 10f32.powf(level_db / 20.0), 2.0);
            let output = render_mono(&mut *gate, &input);
            for (i, (x, y)) in input.iter().zip(&output).enumerate().skip(480) {
                if x.abs() > 1e-8 {
                    assert!(
                        gain_db(*x, *y) >= -0.1,
                        "hysteresis {hysteresis} dB, level {level_db} dBFS: B0 gain at sample {i}: {}",
                        gain_db(*x, *y)
                    );
                }
            }
        }

        let mut no_hold = built();
        no_hold.set_param(1, 0.0);
        no_hold.set_param(3, 0.0);
        no_hold.reset();
        let input = sine(30.87, 10f32.powf(-46.0 / 20.0), 2.0);
        let output = render_mono(&mut *no_hold, &input);
        let period = (f64::from(RATE) / 30.87).ceil() as usize;
        // Leave the key's startup transient out of the complete-cycle check.
        for (cycle, (input, output)) in input
            .chunks_exact(period)
            .zip(output.chunks_exact(period))
            .enumerate()
            .skip(3)
        {
            assert!(
                input
                    .iter()
                    .zip(output)
                    .any(|(x, y)| x.abs() > 1e-8 && gain_db(*x, *y) < -0.1),
                "B0 cycle {cycle} never attenuated without the hold"
            );
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

    /// As every knob in the family takes it (`Knobs::set`), an infinite
    /// setting is held at the knob's end: a threshold of +Inf is 0 dB, and
    /// a -20 dBFS tone stays shut out.
    #[test]
    fn an_infinite_setting_is_held_at_the_knobs_end() {
        let mut gate = built();
        gate.set_param(0, f32::INFINITY);
        gate.reset();
        let tone = sine(1_000.0, 0.1, 0.1);
        let output = render_mono(&mut *gate, &tone);
        for (x, y) in tone.iter().zip(&output).filter(|(x, _)| x.abs() > 1e-6) {
            assert!(
                (gain_db(*x, *y) + 90.0).abs() < 0.01,
                "gain {} dB with the threshold at +Inf",
                gain_db(*x, *y)
            );
        }
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
