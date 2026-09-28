//! Pieces every drive effect shares: knobs that glide, the stereo frame
//! loop, the input guard and the output ceiling.

use crate::ParamSpec;
use crate::dsp::Smoothed;

/// How long a knob takes to glide to a new setting.
const GLIDE_SECONDS: f32 = 0.02;

/// The loudest input a model is given, +24 dBFS. Louder samples are held
/// here so the circuit solvers always work on sane voltages.
const INPUT_LIMIT: f32 = 16.0;

/// The sample rate assumed before [`crate::Effect::prepare`] is called, or
/// when it is given something that is not a rate.
pub const DEFAULT_RATE: f32 = 48_000.0;

/// An effect's knobs. Every value glides, stepped ones included, so a
/// stepped switch can crossfade between its positions instead of clicking.
#[derive(Debug, Clone)]
pub struct Knobs<const N: usize> {
    specs: &'static [ParamSpec; N],
    values: [Smoothed; N],
}

impl<const N: usize> Knobs<N> {
    /// Knobs for `specs`, each settled at its default.
    #[must_use]
    pub fn new(specs: &'static [ParamSpec; N]) -> Self {
        Self {
            specs,
            values: specs.map(|spec| Smoothed::new(spec.default)),
        }
    }

    /// Glide at `sample_rate`, and settle every knob where it is headed.
    pub fn prepare(&mut self, sample_rate: f32) {
        for value in &mut self.values {
            value.set_time(GLIDE_SECONDS, sample_rate);
        }
        self.settle();
    }

    /// Jump every knob to where it is headed.
    pub fn settle(&mut self) {
        for value in &mut self.values {
            value.snap(value.target());
        }
    }

    /// Turn knob `index` toward `value`, held inside its range. A NaN or an
    /// index there is no knob for is ignored.
    pub fn set(&mut self, index: usize, value: f32) {
        if value.is_nan() {
            return;
        }
        if let (Some(spec), Some(knob)) = (self.specs.get(index), self.values.get_mut(index)) {
            knob.set(spec.clamp(value));
        }
    }

    /// Where every knob is now, without gliding.
    #[must_use]
    pub fn values(&self) -> [f32; N] {
        self.values.map(|value| value.value())
    }

    /// One sample's glide on every knob; returns where they all are now.
    ///
    /// Near its target a glide's steps fall below f32's resolution and it
    /// would stall a hair short for ever, and a glide to zero would crawl
    /// on through the slow subnormal floats. A knob whose step changed
    /// nothing, or that is within a few units in the last place of its
    /// target (or of its range, for a target near zero), is snapped the
    /// rest of the way, so it always arrives exactly and never jumps
    /// audibly to get there.
    pub fn step(&mut self) -> [f32; N] {
        let values = &mut self.values;
        let specs = self.specs;
        std::array::from_fn(|index| {
            let knob = &mut values[index];
            let before = knob.value();
            let now = knob.step();
            let target = knob.target();
            let range = specs[index].max - specs[index].min;
            let stalled = now.to_bits() == before.to_bits();
            let close = 4.0 * f32::EPSILON * target.abs().max(range);
            let arrived = (target - now).abs() <= close;
            if stalled || arrived {
                knob.snap(target);
            }
            knob.value()
        })
    }
}

/// Decides when a filter must be redesigned for knobs that feed it.
///
/// A filter is redesigned in every sample in which a knob that feeds it
/// has moved, and never while they rest. Every sample, because the
/// family's click test shows that even every fourth sample puts a kink in
/// a fast sweep of a steep shelf (the Klon's treble); so a knob at rest
/// costs nothing and a moving one (or a cabled one) costs a design a
/// sample. A finished glide leaves the knob still, so the last design
/// always lands on its exact final value. Measured with a knob swept as a
/// cable would sweep it, a design a sample costs the klon, comp, amp,
/// ringmod and screamer 10 to 36 % over rest; only the crusher's filters,
/// whose designs keep their state's share continuous, redesign every few
/// samples instead (see its `REDESIGN_EVERY`).
#[derive(Debug, Clone, Copy)]
pub struct Retune<const N: usize> {
    tuned: [f32; N],
}

impl<const N: usize> Retune<N> {
    /// Tracking knobs last designed at `tuned`.
    #[must_use]
    pub const fn new(tuned: [f32; N]) -> Self {
        Self { tuned }
    }

    /// The values last designed for.
    #[must_use]
    pub const fn tuned(&self) -> [f32; N] {
        self.tuned
    }

    /// Take `now` as designed: for after a prepare or a reset, which design
    /// at once.
    pub const fn settle(&mut self, now: [f32; N]) {
        self.tuned = now;
    }

    /// Whether `now` differs from what was last designed, without taking
    /// it as designed.
    fn differs(&self, now: [f32; N]) -> bool {
        now.iter()
            .zip(&self.tuned)
            .any(|(a, b)| a.to_bits() != b.to_bits())
    }

    /// Whether `now` differs from what was last designed. When it does it
    /// remembers `now` as designed.
    pub fn due(&mut self, now: [f32; N]) -> bool {
        let moved = self.differs(now);
        if moved {
            self.tuned = now;
        }
        moved
    }
}

/// How much of stepped position `position` a gliding stepped value holds:
/// 1 on the step, falling to 0 one step away. While a switch glides between
/// two neighbouring steps their weights always add up to 1.
#[must_use]
pub fn weight(value: f32, position: f32) -> f32 {
    (1.0 - (value - position).abs()).max(0.0)
}

/// Call `frame` with every left and right input sample in turn and write
/// what it returns to the outputs. Only the shortest of the four slices'
/// lengths is processed; the rest of each output is silenced.
pub fn for_each_frame(
    input: [&[f32]; 2],
    output: [&mut [f32]; 2],
    mut frame: impl FnMut(f32, f32) -> [f32; 2],
) {
    let [in_left, in_right] = input;
    let [out_left, out_right] = output;
    let frames = in_left
        .len()
        .min(in_right.len())
        .min(out_left.len())
        .min(out_right.len());
    let (out_left, rest_left) = out_left.split_at_mut(frames);
    let (out_right, rest_right) = out_right.split_at_mut(frames);
    rest_left.fill(0.0);
    rest_right.fill(0.0);
    let inputs = in_left.iter().zip(in_right);
    let outputs = out_left.iter_mut().zip(out_right.iter_mut());
    for ((left, right), (to_left, to_right)) in inputs.zip(outputs) {
        let [new_left, new_right] = frame(guard(*left), guard(*right));
        *to_left = new_left;
        *to_right = new_right;
    }
}

/// An input sample made safe: a NaN or infinity becomes silence and
/// anything beyond +24 dBFS is held there.
#[must_use]
pub fn guard(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(-INPUT_LIMIT, INPUT_LIMIT)
    } else {
        0.0
    }
}

/// The last thing every drive effect does. Up to full scale it passes the
/// sample untouched; beyond, it bends smoothly toward 2.0 (+6 dBFS) and
/// never passes it, so the wall's limiter always has something sane. A
/// non-finite sample comes out as silence, and so does a tail that has
/// decayed below -400 dB, so nothing downstream is handed denormals.
#[must_use]
pub fn ceiling(sample: f32) -> f32 {
    if !sample.is_finite() {
        return 0.0;
    }
    let size = sample.abs();
    if size < 1e-20 {
        0.0
    } else if size <= 1.0 {
        sample
    } else {
        (1.0 + (size - 1.0).tanh()).copysign(sample)
    }
}

/// A log-taper ("audio") pot: how much of its resistance is in circuit at
/// `travel` (0 to 1). About nine per cent at half travel, near the ten per
/// cent of the real part.
#[must_use]
pub fn audio_taper(travel: f64) -> f64 {
    (100f64.powf(travel.clamp(0.0, 1.0)) - 1.0) / 99.0
}

/// A percentage knob as a fraction, 0 to 1.
#[must_use]
pub fn fraction(percent: f32) -> f64 {
    (f64::from(percent) / 100.0).clamp(0.0, 1.0)
}

/// Decibels to linear gain, in f64.
#[must_use]
pub fn gain(db: f32) -> f64 {
    10f64.powf(f64::from(db) / 20.0)
}

/// A C1-smooth hard limit at `limit`: exact below `limit - knee`, a
/// quadratic bend across the knee, flat beyond. The op-amp and transistor
/// rails in the circuit models use it.
#[must_use]
pub fn rail(x: f64, limit: f64, knee: f64) -> f64 {
    let size = x.abs();
    let start = limit - knee;
    if size <= start {
        x
    } else if size >= limit + knee {
        limit.copysign(x)
    } else {
        let over = size - start;
        (size - over * over / (4.0 * knee)).copysign(x)
    }
}

/// A one-pole smoother's coefficient for a time constant of `seconds` at
/// `rate`: the share of the way to its target it moves each sample. A time
/// shorter than one sample moves all the way.
#[must_use]
pub fn one_pole(seconds: f64, rate: f64) -> f64 {
    let samples = seconds * rate;
    if samples > 1.0 {
        1.0 - (-1.0 / samples).exp()
    } else {
        1.0
    }
}

/// `ln cosh x`, the antiderivative of `tanh`, without overflow:
/// `|x| + ln(1 + e^(-2|x|)) - ln 2`.
#[must_use]
pub fn log_cosh(x: f64) -> f64 {
    let size = x.abs();
    (-2.0 * size).exp().ln_1p() + size - std::f64::consts::LN_2
}

/// Zero an f64 state that has gone non-finite or decayed into the denormal
/// range. (The f32 twin is [`crate::dsp::flush`].)
pub fn flush64(state: &mut f64) {
    if !state.is_finite() || state.abs() < 1e-30 {
        *state = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Curve;

    static SPECS: [ParamSpec; 2] = [
        ParamSpec {
            name: "amount",
            min: 0.0,
            max: 10.0,
            default: 5.0,
            unit: "",
            curve: Curve::Linear,
        },
        ParamSpec {
            name: "mode",
            min: 0.0,
            max: 2.0,
            default: 0.0,
            unit: "",
            curve: Curve::Stepped {
                labels: &["a", "b", "c"],
            },
        },
    ];

    #[test]
    fn knobs_glide_clamp_and_ignore_nonsense() {
        let mut knobs = Knobs::new(&SPECS);
        knobs.prepare(48_000.0);
        knobs.set(0, 100.0);
        knobs.set(0, f32::NAN);
        knobs.set(7, 1.0);
        knobs.set(1, 1.4);
        let first = knobs.step();
        assert!(first[0] > 5.0 && first[0] < 10.0, "{first:?}");
        for _ in 0..48_000 {
            knobs.step();
        }
        let settled = knobs.step();
        assert!((settled[0] - 10.0).abs() < f32::EPSILON);
        assert!((settled[1] - 1.0).abs() < f32::EPSILON);
    }

    /// A glide to zero arrives exactly, long before it could crawl down
    /// through the subnormal floats.
    #[test]
    fn a_glide_to_zero_arrives() {
        let mut knobs = Knobs::new(&SPECS);
        knobs.prepare(48_000.0);
        knobs.set(0, 0.0);
        let mut arrived = None;
        for n in 0..40_000 {
            let [now, _] = knobs.step();
            assert!(now == 0.0 || now.abs() >= f32::MIN_POSITIVE, "{n}: {now}");
            if now == 0.0 && arrived.is_none() {
                arrived = Some(n);
            }
        }
        assert!(arrived.is_some_and(|n| n < 20_000), "{arrived:?}");
    }

    /// A retune asks for a design when, and only when, its knobs move.
    #[test]
    fn a_retune_follows_its_knobs() {
        let mut retune = Retune::new([0.0f32]);
        assert!(!retune.due([0.0]));
        assert!(retune.due([1.0]));
        assert!(retune.due([1.5]));
        assert!(!retune.due([1.5]));
        retune.settle([2.0]);
        assert_eq!(retune.tuned()[0].to_bits(), 2.0f32.to_bits());
        assert!(!retune.due([2.0]));
    }

    #[test]
    fn stepped_weights_add_up_while_gliding() {
        for tenth in 0..=20 {
            let value = tenth as f32 / 10.0;
            let total: f32 = (0..3).map(|step| weight(value, step as f32)).sum();
            assert!((total - 1.0).abs() < 1e-5, "{value}: {total}");
        }
    }

    #[test]
    fn the_ceiling_is_transparent_below_full_scale_and_never_passes_six_db() {
        assert!((ceiling(0.7) - 0.7).abs() < f32::EPSILON);
        assert!((ceiling(-1.0) + 1.0).abs() < f32::EPSILON);
        assert!(ceiling(1e9) <= 2.0);
        assert!(ceiling(-1e9) >= -2.0);
        assert!(ceiling(1.5) > ceiling(1.2));
        assert!(ceiling(f32::NAN).abs() < f32::EPSILON);
    }

    #[test]
    fn the_rail_is_smooth_and_bounded() {
        assert!((rail(1.0, 4.0, 0.5) - 1.0).abs() < 1e-12);
        assert!((rail(-100.0, 4.0, 0.5) + 4.0).abs() < 1e-12);
        let below = rail(3.999, 4.0, 0.5);
        let above = rail(4.001, 4.0, 0.5);
        assert!(below < above && above < 4.0);
    }

    #[test]
    fn frames_follow_the_shortest_slice() {
        let input = [0.5f32; 8];
        let short = [0.5f32; 3];
        let mut left = [9.0f32; 8];
        let mut right = [9.0f32; 5];
        for_each_frame([&input, &short], [&mut left, &mut right], |l, r| [l, r]);
        assert_eq!(&left[..3], &[0.5; 3]);
        assert!(left[3..].iter().all(|s| s.abs() < f32::EPSILON));
        assert!(right[3..].iter().all(|s| s.abs() < f32::EPSILON));
    }
}
