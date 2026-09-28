//! The parts every voice is built from.
//!
//! Parameter glides, decay envelopes, a damped sinusoidal mode (the
//! resonator every drum and bar is made of), a state-variable filter, the
//! drum machines' six-square metal source, and [`Finish`], the last stage of
//! every voice: declick on restrike, choke, a safety ceiling and the NaN
//! guard. Everything here is real-time safe.

use std::f32::consts::{PI, TAU};

use kazoo_fx::dsp::{Smoothed, flush};
use kazoo_fx::{Curve, ParamSpec};

/// `ln(1000)`: an exponential decay falls 60 dB over this many time
/// constants.
const LN_1000: f32 = 6.907_755;

/// Below this an envelope is called finished and set to exact zero.
const FLOOR: f32 = 1.0e-5;

/// How long a continuous parameter takes to glide to a new value (the time
/// constant of its one-pole curve).
pub const GLIDE_SECONDS: f32 = 0.01;

/// Labels for stepped count parameters: `NUMBERS[n]` is `n` written out.
/// A parameter from `a` to `b` uses `numbers(a, b)`.
pub const NUMBERS: [&str; 65] = [
    "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16",
    "17", "18", "19", "20", "21", "22", "23", "24", "25", "26", "27", "28", "29", "30", "31", "32",
    "33", "34", "35", "36", "37", "38", "39", "40", "41", "42", "43", "44", "45", "46", "47", "48",
    "49", "50", "51", "52", "53", "54", "55", "56", "57", "58", "59", "60", "61", "62", "63", "64",
];

/// The labels `from` up to `to` from [`NUMBERS`], for a stepped count.
#[must_use]
pub const fn numbers(from: usize, to: usize) -> &'static [&'static str] {
    NUMBERS.split_at(to + 1).0.split_at(from).1
}

/// A usable sample rate: a non-finite rate or one below 8 kHz becomes
/// 48 kHz, one above 768 kHz is held there.
#[must_use]
pub fn sane_rate(sample_rate: f32) -> f32 {
    if sample_rate.is_finite() && sample_rate >= 8_000.0 {
        sample_rate.min(768_000.0)
    } else {
        48_000.0
    }
}

/// The frequency ratio of `semitones`.
#[must_use]
pub fn ratio(semitones: f32) -> f32 {
    (semitones / 12.0).exp2()
}

/// The per-sample multiplier that makes a decay fall 60 dB in `seconds`.
/// Zero for a time too short to last a sample.
#[must_use]
pub fn t60_coeff(seconds: f32, sample_rate: f32) -> f32 {
    let samples = seconds * sample_rate;
    if samples.is_finite() && samples > 1.0 {
        (-LN_1000 / samples).exp()
    } else {
        0.0
    }
}

/// A smooth saturator, close to `tanh`: unity gain at small levels, easing
/// into ±1 at an input of ±3 and flat beyond.
#[must_use]
pub fn saturate(x: f32) -> f32 {
    let x = x.clamp(-3.0, 3.0);
    let x2 = x * x;
    x * (27.0 + x2) / 9.0f32.mul_add(x2, 27.0)
}

/// The safety ceiling on every voice's output: untouched up to ±1, then a
/// soft knee that never passes ±1.5 (+3.5 dBFS).
#[must_use]
pub fn safety(x: f32) -> f32 {
    let size = x.abs();
    if size <= 1.0 {
        x
    } else {
        let over = size - 1.0;
        (1.0 + over / 2.0f32.mul_add(over, 1.0)).copysign(x)
    }
}

/// What a trigger asks for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hit {
    /// Strike with this velocity, above 0 up to 1.
    Strike(f32),
    /// Velocity 0: choke what rings.
    Choke,
    /// A NaN: do nothing.
    Ignore,
}

/// Read a trigger's velocity as [`crate::Voice::trigger`] defines it.
#[must_use]
pub fn hit(velocity: f32) -> Hit {
    if velocity.is_nan() {
        return Hit::Ignore;
    }
    let velocity = velocity.clamp(0.0, 1.0);
    if velocity > 0.0 {
        Hit::Strike(velocity)
    } else {
        Hit::Choke
    }
}

/// A voice's parameters: each held in its range, the continuous ones
/// gliding, the stepped ones jumping.
#[derive(Debug, Clone, Copy)]
pub struct Params<const N: usize> {
    specs: &'static [ParamSpec; N],
    values: [Smoothed; N],
}

impl<const N: usize> Params<N> {
    /// Every parameter at its default.
    #[must_use]
    pub fn new(specs: &'static [ParamSpec; N]) -> Self {
        Self {
            specs,
            values: std::array::from_fn(|index| Smoothed::new(specs[index].default)),
        }
    }

    /// Set the glide for `sample_rate` and settle every value on its target.
    pub fn prepare(&mut self, sample_rate: f32) {
        for (value, spec) in self.values.iter_mut().zip(self.specs.iter()) {
            let time = match spec.curve {
                Curve::Stepped { .. } => 0.0,
                Curve::Linear | Curve::Log => GLIDE_SECONDS,
            };
            value.set_time(time, sample_rate);
        }
        self.snap();
    }

    /// Aim parameter `index` at `value`, clamped; NaN and unknown indices
    /// are ignored. A stepped parameter changes at once.
    pub fn set(&mut self, index: usize, value: f32) {
        if value.is_nan() {
            return;
        }
        let (Some(spec), Some(slot)) = (self.specs.get(index), self.values.get_mut(index)) else {
            return;
        };
        let held = spec.clamp(value);
        match spec.curve {
            Curve::Stepped { .. } => slot.snap(held),
            Curve::Linear | Curve::Log => slot.set(held),
        }
    }

    /// Advance every glide by a sample.
    pub fn step(&mut self) {
        for value in &mut self.values {
            value.step();
        }
    }

    /// Settle every value on its target at once.
    pub fn snap(&mut self) {
        for value in &mut self.values {
            value.snap(value.target());
        }
    }

    /// The current value of parameter `index` (0 for an index out of
    /// range, which the voices never ask for).
    #[must_use]
    pub fn get(&self, index: usize) -> f32 {
        self.values.get(index).map_or(0.0, Smoothed::value)
    }

    /// The current value of a stepped parameter as a whole number.
    #[must_use]
    pub fn step_index(&self, index: usize) -> usize {
        self.get(index).round().max(0.0) as usize
    }
}

/// An exponential decay envelope.
#[derive(Debug, Clone, Copy, Default)]
pub struct Decay {
    level: f32,
    coeff: f32,
    seconds: f32,
}

impl Decay {
    /// Fall 60 dB in `seconds`. Cheap when the time has not changed.
    pub fn set_time(&mut self, seconds: f32, sample_rate: f32) {
        if (seconds - self.seconds).abs() > self.seconds * 1.0e-4 || self.coeff == 0.0 {
            self.seconds = seconds;
            self.coeff = t60_coeff(seconds, sample_rate);
        }
    }

    /// Jump to `level`.
    pub const fn strike(&mut self, level: f32) {
        self.level = level;
    }

    /// The level now, then one sample's fall.
    pub fn tick(&mut self) -> f32 {
        let now = self.level;
        self.level *= self.coeff;
        if self.level < FLOOR {
            self.level = 0.0;
        }
        now
    }

    /// The level now.
    #[must_use]
    pub const fn level(&self) -> f32 {
        self.level
    }

    /// Whether it has fallen to silence.
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.level <= 0.0
    }

    /// Silence it.
    pub const fn clear(&mut self) {
        self.level = 0.0;
    }
}

/// One resonant mode: a damped sinusoid, computed as a complex number
/// turning a little and shrinking a little every sample.
///
/// This is the impulse-invariant form of a two-pole resonator (and of the
/// drum machines' bridged-T filter pinged into ringing). Striking adds to
/// the mode's velocity (its real part) while the output is its displacement
/// (the imaginary part), so a strike never makes the output jump, and a
/// strike on a ringing mode adds to it as it would on a real drum. Retuning
/// turns the rotation without touching the stored energy, so a pitch sweep
/// neither clicks nor changes the level.
#[derive(Debug, Clone, Copy, Default)]
pub struct Mode {
    re: f32,
    im: f32,
    cos: f32,
    sin: f32,
}

impl Mode {
    /// Ring at `hz`, falling 60 dB in `t60` seconds. A frequency at or past
    /// 0.48 of the sample rate (or not above 0) silences the mode rather
    /// than alias.
    pub fn tune(&mut self, hz: f32, t60: f32, sample_rate: f32) {
        if !(hz > 0.0 && hz < 0.48 * sample_rate) {
            self.cos = 0.0;
            self.sin = 0.0;
            return;
        }
        let (sin, cos) = (TAU * hz / sample_rate).sin_cos();
        let radius = t60_coeff(t60, sample_rate);
        self.cos = radius * cos;
        self.sin = radius * sin;
    }

    /// Add a strike of `amount`: the mode then rings with about that
    /// amplitude (more if it was already ringing in step).
    pub fn strike(&mut self, amount: f32) {
        if amount.is_finite() {
            self.re += amount;
        }
    }

    /// One sample: the displacement.
    pub fn tick(&mut self) -> f32 {
        let re = self.cos.mul_add(self.re, -self.sin * self.im);
        self.im = self.sin.mul_add(self.re, self.cos * self.im);
        self.re = re;
        flush(&mut self.re);
        flush(&mut self.im);
        self.im
    }

    /// The squared amplitude it rings with.
    #[must_use]
    pub fn energy(&self) -> f32 {
        self.re.mul_add(self.re, self.im * self.im)
    }

    /// Stop it ringing.
    pub const fn clear(&mut self) {
        self.re = 0.0;
        self.im = 0.0;
    }
}

/// The three outputs of a state-variable filter.
#[derive(Debug, Clone, Copy, Default)]
pub struct SvfOut {
    /// Lowpass.
    pub low: f32,
    /// Bandpass, with unity gain at the centre.
    pub band: f32,
    /// Highpass.
    pub high: f32,
}

/// A trapezoidal state-variable filter (Simper's form): stable however fast
/// its cutoff moves, with lowpass, bandpass and highpass at once.
#[derive(Debug, Clone, Copy, Default)]
pub struct Svf {
    ic1: f32,
    ic2: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    k: f32,
    hz: f32,
    q: f32,
}

impl Svf {
    /// Set the cutoff (held between 10 Hz and 0.45 of the rate) and the
    /// resonance `q` (held between 0.5 and 40). Cheap when nothing changed.
    pub fn set(&mut self, hz: f32, q: f32, sample_rate: f32) {
        let hz = if hz.is_finite() {
            hz.clamp(10.0, 0.45 * sample_rate)
        } else {
            1_000.0
        };
        let q = if q.is_finite() {
            q.clamp(0.5, 40.0)
        } else {
            0.7
        };
        if (hz - self.hz).abs() <= self.hz * 1.0e-5 && (q - self.q).abs() <= 1.0e-5 && self.k > 0.0
        {
            return;
        }
        self.hz = hz;
        self.q = q;
        let g = (PI * hz / sample_rate).tan();
        self.k = 1.0 / q;
        self.a1 = 1.0 / g.mul_add(g + self.k, 1.0);
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }

    /// One sample through the filter.
    pub fn process(&mut self, input: f32) -> SvfOut {
        let v3 = input - self.ic2;
        let v1 = self.a1.mul_add(self.ic1, self.a2 * v3);
        let v2 = self.a2.mul_add(self.ic1, self.a3.mul_add(v3, self.ic2));
        self.ic1 = 2.0f32.mul_add(v1, -self.ic1);
        self.ic2 = 2.0f32.mul_add(v2, -self.ic2);
        flush(&mut self.ic1);
        flush(&mut self.ic2);
        SvfOut {
            low: v2,
            band: self.k * v1,
            high: self.k.mul_add(-v1, input) - v2,
        }
    }

    /// Forget the past.
    pub const fn clear(&mut self) {
        self.ic1 = 0.0;
        self.ic2 = 0.0;
    }
}

/// The correction that takes the step out of a naive square wave's edge
/// (a polynomial band-limited step): `t` is the phase since the edge, `dt`
/// the phase step per sample.
#[must_use]
pub fn blep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let x = t / dt;
        2.0f32.mul_add(x, -(x * x)) - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x.mul_add(x, 2.0f32.mul_add(x, 1.0))
    } else {
        0.0
    }
}

/// The frequencies of the TR-808's six metal oscillators, in hertz, as
/// measured from the circuit: detuned squares with no common harmonic.
pub const METAL_HZ: [f32; 6] = [205.3, 304.4, 369.6, 522.7, 540.0, 800.0];

/// Six band-limited squares at the 808's inharmonic frequencies, summed:
/// the metal the hats, cymbals and (with two of them) the cowbell are
/// filtered from.
#[derive(Debug, Clone, Copy, Default)]
pub struct MetalSource {
    phases: [f32; 6],
}

impl MetalSource {
    /// One sample of all six squares at `tune` times their frequencies, as
    /// weighted by `weights`, divided by six.
    pub fn next(&mut self, tune: f32, sample_rate: f32, weights: [f32; 6]) -> f32 {
        let mut sum = 0.0;
        for ((phase, hz), weight) in self.phases.iter_mut().zip(METAL_HZ).zip(weights) {
            let dt = (hz * tune / sample_rate).clamp(0.0, 0.45);
            sum = weight.mul_add(square(phase, dt), sum);
        }
        sum / 6.0
    }

    /// Put every oscillator back to the start of its cycle.
    pub const fn clear(&mut self) {
        self.phases = [0.0; 6];
    }
}

/// One sample of a band-limited square wave at phase `phase`, which then
/// advances by `dt`.
pub fn square(phase: &mut f32, dt: f32) -> f32 {
    let t = *phase;
    let mut value = if t < 0.5 { 1.0 } else { -1.0 };
    value += blep(t, dt);
    value -= blep((t + 0.5).fract(), dt);
    *phase = (t + dt).fract();
    value
}

/// The last stage of every voice.
///
/// - **Declick.** When an analogue voice is struck again while ringing it
///   restarts its circuit from rest, which on its own would jump. The last
///   output is held as an offset that fades away over about 1.5 ms, so the
///   restart is continuous.
/// - **Choke.** A choke fades the voice out by 60 dB in about 14 ms, then
///   asks the voice to clear.
/// - **Safety.** The [`safety`] ceiling.
/// - **Poison.** A non-finite sample is replaced by silence and the voice
///   is asked to clear.
#[derive(Debug, Clone, Copy)]
pub struct Finish {
    offset: f32,
    declick: f32,
    gain: f32,
    fade: f32,
    choking: bool,
    last: f32,
}

impl Default for Finish {
    fn default() -> Self {
        Self::new()
    }
}

impl Finish {
    /// A finish for 48 kHz until [`Self::prepare`] says otherwise.
    #[must_use]
    pub fn new() -> Self {
        let mut finish = Self {
            offset: 0.0,
            declick: 0.0,
            gain: 1.0,
            fade: 0.0,
            choking: false,
            last: 0.0,
        };
        finish.prepare(48_000.0);
        finish
    }

    /// Set the fade times for `sample_rate` and fall silent.
    pub fn prepare(&mut self, sample_rate: f32) {
        self.declick = (-1.0 / (0.0015 * sample_rate)).exp();
        self.fade = (-1.0 / (0.002 * sample_rate)).exp();
        self.clear();
    }

    /// The voice is about to restart from rest: hold the last output so the
    /// restart does not jump. Also ends a choke in progress.
    pub const fn restrike(&mut self) {
        self.offset = self.last;
        self.choking = false;
        self.gain = 1.0;
    }

    /// The voice is struck again without restarting (a physical model
    /// adding to what rings). A choke in progress ends; returns whether one
    /// was, in which case the voice must clear what was being choked before
    /// adding the strike (the fade's last output is held, as for
    /// [`Self::restrike`], so there is no jump).
    #[must_use]
    pub const fn continue_strike(&mut self) -> bool {
        let choking = self.choking;
        if choking {
            self.restrike();
        }
        choking
    }

    /// Start a choke.
    pub const fn choke(&mut self) {
        self.choking = true;
    }

    /// Finish one sample of the voice's `dry` output. Returns the sample
    /// and whether the voice must clear its state now (a choke has ended or
    /// `dry` was not finite).
    pub fn next(&mut self, dry: f32) -> (f32, bool) {
        let mut clear = !dry.is_finite();
        let mut wet = if clear { 0.0 } else { dry };
        if self.choking {
            self.gain *= self.fade;
            if self.gain < 1.0e-3 {
                self.choking = false;
                self.gain = 1.0;
                clear = true;
                wet = 0.0;
            } else {
                wet *= self.gain;
            }
        }
        let out = safety(wet + self.offset);
        self.offset *= self.declick;
        if self.offset.abs() < FLOOR {
            self.offset = 0.0;
        }
        self.last = out;
        (out, clear)
    }

    /// Whether there is nothing left to fade.
    #[must_use]
    pub fn is_quiet(&self) -> bool {
        self.offset == 0.0 && !self.choking
    }

    /// Drop every fade at once.
    pub const fn clear(&mut self) {
        self.offset = 0.0;
        self.gain = 1.0;
        self.choking = false;
        self.last = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mode_rings_at_its_frequency_and_decays() {
        let rate = 48_000.0;
        let mut mode = Mode::default();
        mode.tune(1_000.0, 0.5, rate);
        mode.strike(1.0);
        let mut crossings = 0;
        let mut last = 0.0;
        let mut peak = 0.0f32;
        for _ in 0..4_800 {
            let now = mode.tick();
            if last <= 0.0 && now > 0.0 {
                crossings += 1;
            }
            last = now;
            peak = peak.max(now.abs());
        }
        assert!((99..=101).contains(&crossings), "{crossings}");
        assert!(peak <= 1.0 && peak > 0.9, "{peak}");
        for _ in 0..(rate as usize / 2) {
            mode.tick();
        }
        assert!(mode.energy().sqrt() < 2.0e-3);
    }

    #[test]
    fn a_mode_above_nyquist_is_silent() {
        let mut mode = Mode::default();
        mode.tune(30_000.0, 1.0, 48_000.0);
        mode.strike(1.0);
        mode.tick();
        assert!(mode.tick().abs() < f32::EPSILON);
    }

    #[test]
    fn the_safety_ceiling_is_smooth_and_bounded() {
        assert!((safety(0.5) - 0.5).abs() < f32::EPSILON);
        assert!((safety(1.0) - 1.0).abs() < f32::EPSILON);
        assert!(safety(1.0e9) <= 1.5 && safety(-1.0e9) >= -1.5);
        assert!((safety(1.001) - 1.001).abs() < 1.0e-4);
    }

    #[test]
    fn saturation_is_odd_and_bounded() {
        assert!((saturate(0.0)).abs() < f32::EPSILON);
        assert!((saturate(3.0) - 1.0).abs() < 1.0e-6);
        assert!((saturate(-10.0) + 1.0).abs() < 1.0e-6);
        assert!((saturate(0.1) - 0.1).abs() < 1.0e-3);
    }

    #[test]
    fn the_svf_bandpass_has_unity_gain_at_centre() {
        let rate = 48_000.0;
        let mut filter = Svf::default();
        filter.set(1_000.0, 5.0, rate);
        let mut peak = 0.0f32;
        for n in 0..48_000 {
            let x = (TAU * 1_000.0 * n as f32 / rate).sin();
            let out = filter.process(x);
            if n > 24_000 {
                peak = peak.max(out.band.abs());
            }
        }
        assert!((peak - 1.0).abs() < 0.02, "{peak}");
    }

    #[test]
    fn finish_declicks_and_chokes() {
        let mut finish = Finish::new();
        finish.prepare(48_000.0);
        finish.next(0.8);
        finish.restrike();
        let (first, clear) = finish.next(0.0);
        assert!(!clear && (first - 0.8).abs() < 1.0e-6);
        for _ in 0..2_000 {
            finish.next(0.0);
        }
        assert!(finish.is_quiet());
        finish.choke();
        let mut cleared = false;
        for _ in 0..2_000 {
            let (_, clear) = finish.next(0.5);
            cleared |= clear;
        }
        assert!(cleared && finish.is_quiet());
        assert_eq!(finish.next(f32::NAN), (0.0, true));
    }

    #[test]
    fn params_clamp_ignore_nan_and_step() {
        static SPECS: [ParamSpec; 2] = [
            ParamSpec {
                name: "a",
                min: 0.0,
                max: 1.0,
                default: 0.5,
                unit: "",
                curve: Curve::Linear,
            },
            ParamSpec {
                name: "b",
                min: 0.0,
                max: 2.0,
                default: 0.0,
                unit: "",
                curve: Curve::Stepped {
                    labels: &["x", "y", "z"],
                },
            },
        ];
        let mut params = Params::new(&SPECS);
        params.prepare(48_000.0);
        params.set(0, 9.0);
        params.set(0, f32::NAN);
        params.set(1, 1.6);
        params.set(7, 1.0);
        assert_eq!(params.step_index(1), 2);
        for _ in 0..48_000 {
            params.step();
        }
        assert!((params.get(0) - 1.0).abs() < 1.0e-4);
        assert!(params.get(9).abs() < f32::EPSILON);
    }

    #[test]
    fn a_band_limited_square_holds_its_level() {
        let mut phase = 0.0;
        for _ in 0..10_000 {
            let value = square(&mut phase, 0.013);
            assert!(value.abs() <= 1.0 + 1.0e-6);
        }
    }
}
