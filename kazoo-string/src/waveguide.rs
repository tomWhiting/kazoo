//! One vibrating string: a Karplus-Strong digital waveguide.
//!
//! A delay line the length of one period feeds itself through a loop filter.
//! The filter chain is a two-point averager (the string loses treble as it
//! rings), an optional pair of allpass "dispersion" stages (stiffness: upper
//! partials run sharp, like a piano wire or a bar), and a fractional-delay
//! allpass that tunes the loop to the exact pitch. Every delay in the loop is
//! measured at the fundamental, so the note lands on pitch whatever the
//! damping or stiffness.
//!
//! Nothing here allocates after [`StringLine::new`].

use std::f32::consts::TAU;

/// Lowest pitch the delay line can hold, in hertz.
pub const MIN_HZ: f32 = 8.0;
/// A period shorter than this many samples leaves no room for the loop
/// filters, so the top pitch is `sample_rate / MIN_PERIOD`.
pub const MIN_PERIOD: f32 = 5.0;
/// Dispersion stages in the loop.
const DISPERSION_STAGES: usize = 2;
/// Loop gain never reaches unity, so the loop cannot grow at any frequency.
const MAX_LOOP_GAIN: f32 = 0.999_99;
/// Below this smoothed energy a released string is silent.
const SILENCE_ENERGY: f32 = 1.0e-11;
/// Smoothing for the energy follower, per sample.
const ENERGY_SMOOTHING: f32 = 0.002;
/// DC blocker pole.
const DC_POLE: f32 = 0.995;
/// The comb the pick position cuts halves the injected power.
const COMB_COMPENSATION: f32 = 0.75;

/// Phase delay, in samples, of the allpass `(a + z^-1) / (1 + a z^-1)` at the
/// angular frequency `w` (radians per sample).
#[must_use]
pub fn allpass_delay(a: f32, w: f32) -> f32 {
    if w < 1.0e-5 {
        return (1.0 - a) / (1.0 + a);
    }
    let (sin, cos) = w.sin_cos();
    let num = (-sin).atan2(a + cos);
    let den = (-a * sin).atan2(a.mul_add(cos, 1.0));
    (-(num - den) / w).max(0.0)
}

/// The allpass coefficient whose delay at DC is `delay` samples.
#[must_use]
pub fn allpass_coefficient(delay: f32) -> f32 {
    (1.0 - delay) / (1.0 + delay)
}

/// Phase delay, in samples, of the averager `(1 - s) + s z^-1` at `w`.
#[must_use]
pub fn averager_delay(s: f32, w: f32) -> f32 {
    if w < 1.0e-5 {
        return s;
    }
    let (sin, cos) = w.sin_cos();
    (-(-s * sin).atan2(s.mul_add(cos, 1.0 - s)) / w).max(0.0)
}

/// Gain of the averager `(1 - s) + s z^-1` at `w`.
#[must_use]
pub fn averager_gain(s: f32, w: f32) -> f32 {
    let (sin, cos) = w.sin_cos();
    let re = s.mul_add(cos, 1.0 - s);
    let im = -s * sin;
    re.hypot(im)
}

/// Per-pass loop gain that makes a partial at `hz` fall 60 dB in `t60` seconds.
#[must_use]
pub fn gain_for_t60(hz: f32, t60: f32) -> f32 {
    let passes = (hz * t60).max(1.0);
    (-6.907_755 / passes).exp()
}

/// Loop gains `(held, released)` that give `t60` and `release_t60` seconds of
/// ring at the fundamental `hz`, for an averager blend of `blend`. Never
/// unity, so the loop cannot grow, and release never rings longer than hold.
#[must_use]
pub fn loop_gains(sample_rate: f32, hz: f32, blend: f32, t60: f32, release_t60: f32) -> (f32, f32) {
    let w = TAU * hz / sample_rate;
    let averager = averager_gain(blend, w).max(1.0e-3);
    let gain = (gain_for_t60(hz, t60.max(0.01)) / averager).min(MAX_LOOP_GAIN);
    let release = (gain_for_t60(hz, release_t60.max(0.005)) / averager).min(gain);
    (gain, release)
}

/// Everything a note needs to tune and voice a string. Built once per note
/// by [`StringTuning::solve`], off the sample loop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StringTuning {
    /// Whole samples in the delay line.
    pub length: usize,
    /// Averager blend, 0 (bright) to 0.5 (dark).
    pub blend: f32,
    /// Dispersion coefficient, 0 when bypassed.
    pub dispersion: f32,
    /// Fractional-delay allpass coefficient.
    pub tune: f32,
    /// Loop gain while the note is held.
    pub gain: f32,
    /// Loop gain after the note is released.
    pub release_gain: f32,
    /// The pitch actually tuned, after clamping to what the line can hold.
    pub hz: f32,
}

impl StringTuning {
    /// Solve the loop for `hz` at `sample_rate`.
    ///
    /// `blend` is the averager blend (0 to 0.5), `stiffness` (0 to 1) sets the
    /// dispersion, `t60` the ring time at the fundamental and `release_t60`
    /// the ring time once released. Out-of-range or non-finite inputs are
    /// clamped, never rejected.
    #[must_use]
    pub fn solve(
        sample_rate: f32,
        hz: f32,
        blend: f32,
        stiffness: f32,
        t60: f32,
        release_t60: f32,
    ) -> Self {
        let max_hz = sample_rate / MIN_PERIOD;
        let hz = if hz.is_finite() { hz } else { 440.0 }.clamp(MIN_HZ, max_hz);
        let blend = if blend.is_finite() { blend } else { 0.25 }.clamp(0.0, 0.5);
        let stiffness = if stiffness.is_finite() {
            stiffness
        } else {
            0.0
        }
        .clamp(0.0, 1.0);
        let t60 = if t60.is_finite() { t60 } else { 1.0 }.max(0.01);
        let release_t60 = if release_t60.is_finite() {
            release_t60
        } else {
            0.1
        }
        .max(0.005);

        let period = sample_rate / hz;
        let w = TAU * hz / sample_rate;
        let damp_delay = averager_delay(blend, w);

        // Stiffness bends upper partials sharp with a negative coefficient.
        // Shrink it until the stages leave room for a delay line.
        let mut dispersion = -stiffness * 0.6;
        let mut stage_delay = 0.0;
        if dispersion.abs() > 1.0e-4 {
            let mut fits = false;
            for _ in 0..40 {
                stage_delay = allpass_delay(dispersion, w) * DISPERSION_STAGES as f32;
                if period - damp_delay - stage_delay >= 1.5 {
                    fits = true;
                    break;
                }
                dispersion *= 0.8;
            }
            if !fits {
                dispersion = 0.0;
            }
        }
        if dispersion.abs() <= 1.0e-4 {
            dispersion = 0.0;
            stage_delay = 0.0;
        }

        let rest = period - damp_delay - stage_delay;
        let length = ((rest - 0.5).floor().max(1.0)) as usize;
        let want = (rest - length as f32).clamp(0.1, 1.9);
        // The tuning allpass delays a little more at `w` than at DC; walk the
        // coefficient until the delay at the fundamental is what is wanted.
        let mut aim = want;
        for _ in 0..4 {
            let actual = allpass_delay(allpass_coefficient(aim), w);
            aim = (aim + (want - actual)).clamp(0.05, 1.95);
        }
        let tune = allpass_coefficient(aim);

        let (gain, release_gain) = loop_gains(sample_rate, hz, blend, t60, release_t60);
        Self {
            length,
            blend,
            dispersion,
            tune,
            gain,
            release_gain,
            hz,
        }
    }
}

/// A first-order allpass in transposed direct form.
#[derive(Debug, Clone, Copy, Default)]
struct Allpass {
    state: f32,
}

impl Allpass {
    fn process(&mut self, a: f32, x: f32) -> f32 {
        let y = a.mul_add(x, self.state);
        self.state = (-a).mul_add(y, x);
        y
    }
}

/// The plucking force: a burst of noise, softened by the pick's hardness,
/// injected into the loop for one period.
#[derive(Debug, Clone, Copy)]
struct Exciter {
    left: usize,
    amplitude: f32,
    /// One-pole lowpass coefficient: 1 is white, small is soft.
    brightness: f32,
    lowpass: f32,
    /// Where the pick sits: the delayed, inverted copy lands this many
    /// samples behind the direct one.
    comb: usize,
    seed: u32,
}

impl Exciter {
    const IDLE: Self = Self {
        left: 0,
        amplitude: 0.0,
        brightness: 1.0,
        lowpass: 0.0,
        comb: 1,
        seed: 0x9E37_79B9,
    };

    /// Next noise sample in -1..1 (xorshift32, so notes are reproducible).
    fn noise(&mut self) -> f32 {
        let mut x = self.seed;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.seed = x;
        (x as f32 / u32::MAX as f32).mul_add(2.0, -1.0)
    }

    /// The next injected sample, zero once the pluck is over.
    fn sample(&mut self) -> f32 {
        if self.left == 0 {
            return 0.0;
        }
        self.left -= 1;
        let noise = self.noise();
        self.lowpass = self.brightness.mul_add(noise - self.lowpass, self.lowpass);
        // Keep the energy the same however soft the pick: a one-pole lowpass
        // of white noise has power c / (2 - c).
        let norm = ((2.0 - self.brightness) / self.brightness).sqrt();
        self.lowpass * norm * self.amplitude
    }
}

/// A plucked-string voice: delay line, loop filters, exciter and pickup.
#[derive(Debug)]
pub struct StringLine {
    buffer: Vec<f32>,
    length: usize,
    pos: usize,
    tuning: StringTuning,
    gain: f32,
    previous: f32,
    dispersion: [Allpass; DISPERSION_STAGES],
    tuner: Allpass,
    exciter: Exciter,
    energy: f32,
    dc_in: f32,
    dc_out: f32,
}

impl StringLine {
    /// Allocate a string able to hold any pitch down to [`MIN_HZ`].
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let sample_rate = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };
        let capacity = (sample_rate / MIN_HZ).ceil() as usize + 8;
        Self {
            buffer: vec![0.0; capacity],
            length: 1,
            pos: 0,
            tuning: StringTuning::solve(sample_rate, 440.0, 0.25, 0.0, 1.0, 0.1),
            gain: 0.0,
            previous: 0.0,
            dispersion: [Allpass::default(); DISPERSION_STAGES],
            tuner: Allpass::default(),
            exciter: Exciter::IDLE,
            energy: 0.0,
            dc_in: 0.0,
            dc_out: 0.0,
        }
    }

    /// Silence the string and empty the loop.
    pub fn clear(&mut self) {
        self.buffer.fill(0.0);
        self.previous = 0.0;
        self.dispersion = [Allpass::default(); DISPERSION_STAGES];
        self.tuner = Allpass::default();
        self.exciter = Exciter::IDLE;
        self.energy = 0.0;
        self.dc_in = 0.0;
        self.dc_out = 0.0;
        self.gain = 0.0;
    }

    /// Whether the string is still audible: being plucked or ringing.
    #[must_use]
    pub fn is_sounding(&self) -> bool {
        self.exciter.left > 0 || self.energy > SILENCE_ENERGY
    }

    /// Smoothed energy of the pickup: how loud the string is right now.
    #[must_use]
    pub const fn level(&self) -> f32 {
        self.energy
    }

    /// The tuning in use.
    #[must_use]
    pub const fn tuning(&self) -> &StringTuning {
        &self.tuning
    }

    /// Pluck the string. What already rings stays in the loop, so a repeated
    /// or stolen note does not click; the loop is retuned only if `tuning`
    /// changes the length, in which case the old sound is dropped.
    ///
    /// `amplitude` is the pluck force, `brightness` (0 to 1) the pick's
    /// hardness and `position` (0 to 1) where along the string it sits.
    pub fn pluck(&mut self, tuning: StringTuning, amplitude: f32, brightness: f32, position: f32) {
        let length = tuning.length.clamp(1, self.buffer.len());
        if length != self.length {
            // A different loop length cannot carry the old contents.
            self.buffer.fill(0.0);
            self.pos = 0;
            self.previous = 0.0;
            self.dispersion = [Allpass::default(); DISPERSION_STAGES];
            self.tuner = Allpass::default();
        }
        self.length = length;
        self.tuning = tuning;
        self.gain = tuning.gain;
        let position = if position.is_finite() { position } else { 0.5 }.clamp(0.02, 0.5);
        let comb = ((position * length as f32).round() as usize).clamp(1, length.max(2) - 1);
        let brightness = if brightness.is_finite() {
            brightness
        } else {
            0.5
        }
        .clamp(0.02, 1.0);
        let amplitude = if amplitude.is_finite() {
            amplitude
        } else {
            0.0
        }
        .clamp(0.0, 2.0);
        self.exciter = Exciter {
            left: length,
            amplitude: amplitude * COMB_COMPENSATION,
            brightness,
            lowpass: 0.0,
            comb: if length > 1 { comb } else { 0 },
            seed: self.exciter.seed,
        };
        // Any energy counts as ringing until the follower says otherwise.
        self.energy = self.energy.max(amplitude * amplitude * 0.25);
    }

    /// Damp the string: the loop now loses energy at the release rate.
    pub const fn damp(&mut self) {
        self.gain = self.tuning.release_gain.min(self.gain);
    }

    /// Mute the string quickly: the loop now falls 60 dB in `t60` seconds,
    /// whatever its ring time was. Used for strings that were stolen.
    pub fn mute(&mut self, t60: f32) {
        let fast = gain_for_t60(self.tuning.hz, t60);
        self.gain = self.gain.min(fast);
        self.tuning.release_gain = self.tuning.release_gain.min(fast);
    }

    /// Change the ring time of a string that is already sounding. `held`
    /// picks which of the tuning's gains applies. This never moves the pitch.
    pub const fn set_gains(&mut self, gain: f32, release_gain: f32, held: bool) {
        self.tuning.gain = gain;
        self.tuning.release_gain = release_gain.min(gain);
        self.gain = if held {
            self.tuning.gain
        } else {
            self.tuning.release_gain
        };
    }

    /// Advance one sample and return the pickup.
    pub fn process(&mut self) -> f32 {
        let length = self.length;
        let out = self.buffer[self.pos];

        // Loop filter: averager, dispersion, then the tuning allpass.
        let blend = self.tuning.blend;
        let mut x = (1.0 - blend).mul_add(out, blend * self.previous);
        self.previous = out;
        if self.tuning.dispersion != 0.0 {
            for stage in &mut self.dispersion {
                x = stage.process(self.tuning.dispersion, x);
            }
        }
        x = self.tuner.process(self.tuning.tune, x);
        x *= self.gain;

        let pluck = self.exciter.sample();
        if pluck != 0.0 {
            x += pluck;
            if self.exciter.comb > 0 {
                // The inverted copy is read `comb` samples before the direct
                // one: a comb whose notches sit at the pick position.
                // `comb` is 1..length, so the slot is never the one written below.
                let slot = (self.pos + length - self.exciter.comb) % length;
                self.buffer[slot] -= pluck;
            }
        }
        if !x.is_finite() {
            self.clear();
            return 0.0;
        }
        self.buffer[self.pos] = x;
        self.pos += 1;
        if self.pos >= length {
            self.pos = 0;
        }

        // Pickup: remove DC and follow the energy.
        let blocked = DC_POLE.mul_add(self.dc_out, out - self.dc_in);
        self.dc_in = out;
        self.dc_out = blocked;
        self.energy = ENERGY_SMOOTHING.mul_add(blocked.mul_add(blocked, -self.energy), self.energy);
        blocked
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    const RATE: f32 = 48_000.0;

    fn render(hz: f32, blend: f32, stiffness: f32, t60: f32, samples: usize) -> Vec<f32> {
        let mut line = StringLine::new(RATE);
        let tuning = StringTuning::solve(RATE, hz, blend, stiffness, t60, 0.1);
        line.pluck(tuning, 0.8, 0.6, 0.2);
        (0..samples).map(|_| line.process()).collect()
    }

    /// Fundamental by autocorrelation with a parabolic peak, in hertz.
    fn measured_hz(signal: &[f32], expected: f32) -> f32 {
        let lag = |l: usize| -> f32 {
            signal[..signal.len() - l]
                .iter()
                .zip(&signal[l..])
                .map(|(a, b)| a * b)
                .sum()
        };
        // Measure across many periods: a bright string's autocorrelation
        // peak is a narrow spike, and the error shrinks with the lag.
        let period = RATE / expected;
        let periods = (2_000.0 / period).ceil().max(1.0);
        let target = period * periods;
        // Stay within under half a period so a neighbouring peak cannot win.
        let lo = period.mul_add(-0.4, target) as usize;
        let hi = period.mul_add(0.4, target) as usize + 1;
        let best = (lo..=hi)
            .max_by(|&a, &b| lag(a).total_cmp(&lag(b)))
            .unwrap();
        let (a, b, c) = (lag(best - 1), lag(best), lag(best + 1));
        let shift = 0.5 * (a - c) / (2.0_f32.mul_add(-b, a) + c);
        RATE * periods / (best as f32 + shift)
    }

    fn cents(measured: f32, expected: f32) -> f32 {
        1200.0 * (measured / expected).log2()
    }

    #[test]
    fn loop_gains_are_stable_and_ordered() {
        for hz in [8.0_f32, 110.0, 880.0, 9_000.0] {
            for blend in [0.0_f32, 0.25, 0.5] {
                let (held, released) = loop_gains(RATE, hz, blend, 25.0, 0.05);
                assert!(held < 1.0 && held > 0.0, "{hz} {blend}: {held}");
                assert!(released <= held);
            }
        }
    }

    #[test]
    fn mute_shortens_the_ring() {
        let ring = |mute: bool| {
            let mut line = StringLine::new(RATE);
            line.pluck(
                StringTuning::solve(RATE, 220.0, 0.3, 0.0, 20.0, 0.5),
                1.0,
                0.5,
                0.3,
            );
            if mute {
                line.mute(0.02);
            }
            (0..RATE as usize)
                .map(|_| line.process().abs())
                .sum::<f32>()
        };
        assert!(ring(true) < ring(false) * 0.1);
    }

    #[test]
    fn allpass_delay_matches_dc_formula() {
        for a in [-0.5_f32, -0.2, 0.0, 0.3] {
            let dc = (1.0 - a) / (1.0 + a);
            assert!((allpass_delay(a, 1.0e-3) - dc).abs() < 1.0e-2, "a={a}");
        }
        assert!((allpass_delay(0.0, 1.0) - 1.0).abs() < 1.0e-5);
    }

    #[test]
    fn averager_delay_is_half_a_sample_at_full_blend() {
        assert!((averager_delay(0.5, 0.05) - 0.5).abs() < 1.0e-3);
        assert!(averager_gain(0.5, PI) < 1.0e-6);
        assert!((averager_gain(0.5, 1.0e-6) - 1.0).abs() < 1.0e-6);
    }

    #[test]
    fn tuning_lands_on_pitch_across_the_keyboard() {
        for note in [28_u8, 40, 52, 60, 69, 81, 93, 105] {
            let hz = 440.0 * ((f32::from(note) - 69.0) / 12.0).exp2();
            for blend in [0.0_f32, 0.25, 0.5] {
                let signal = render(hz, blend, 0.0, 6.0, 24_000);
                let got = measured_hz(&signal[2_000..], hz);
                assert!(
                    cents(got, hz).abs() < 4.0,
                    "note {note} blend {blend}: {got} Hz vs {hz} Hz"
                );
            }
        }
    }

    #[test]
    fn stiffness_keeps_the_fundamental_in_tune() {
        for note in [40_u8, 60, 76] {
            let hz = 440.0 * ((f32::from(note) - 69.0) / 12.0).exp2();
            let signal = render(hz, 0.2, 0.7, 6.0, 24_000);
            let got = measured_hz(&signal[2_000..], hz);
            assert!(
                cents(got, hz).abs() < 15.0,
                "note {note}: {got} Hz vs {hz} Hz"
            );
        }
    }

    #[test]
    fn t60_is_honoured_at_the_fundamental() {
        // Fundamental-only check: a dark string rings at about its T60.
        let hz = 220.0;
        let signal = render(hz, 0.5, 0.0, 1.0, 96_000);
        let rms = |s: &[f32]| (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt();
        let early = rms(&signal[4_800..14_400]);
        let late = rms(&signal[52_800..62_400]);
        let db = 20.0 * (late / early).log10();
        // Roughly 60 dB per second across the 1 s gap between the windows;
        // the loop filter also removes treble, so allow a wide band.
        assert!((-90.0..-35.0).contains(&db), "fell {db} dB");
    }

    #[test]
    fn released_string_stops_and_reports_silence() {
        let mut line = StringLine::new(RATE);
        line.pluck(
            StringTuning::solve(RATE, 220.0, 0.3, 0.0, 8.0, 0.05),
            1.0,
            0.5,
            0.3,
        );
        for _ in 0..4_800 {
            line.process();
        }
        assert!(line.is_sounding());
        line.damp();
        for _ in 0..(RATE as usize * 2) {
            line.process();
        }
        assert!(!line.is_sounding());
    }

    #[test]
    fn pluck_position_changes_the_spectrum() {
        // Plucking at the middle cancels even harmonics.
        let hz = 220.0;
        let sample = |position: f32| {
            let mut line = StringLine::new(RATE);
            line.pluck(
                StringTuning::solve(RATE, hz, 0.1, 0.0, 6.0, 0.1),
                0.8,
                1.0,
                position,
            );
            (0..9_600).map(|_| line.process()).collect::<Vec<_>>()
        };
        let bin = |signal: &[f32], harmonic: f32| {
            let w = TAU * hz * harmonic / RATE;
            let (mut re, mut im) = (0.0_f32, 0.0_f32);
            for (n, s) in signal.iter().enumerate() {
                let (sin, cos) = (w * n as f32).sin_cos();
                re += s * cos;
                im += s * sin;
            }
            re.hypot(im)
        };
        let middle = sample(0.5);
        let edge = sample(0.1);
        let ratio_mid = bin(&middle, 2.0) / bin(&middle, 1.0);
        let ratio_edge = bin(&edge, 2.0) / bin(&edge, 1.0);
        assert!(
            ratio_mid < ratio_edge * 0.5,
            "middle {ratio_mid} vs edge {ratio_edge}"
        );
    }

    #[test]
    fn hostile_inputs_are_clamped() {
        for hz in [f32::NAN, f32::INFINITY, -5.0, 0.0, 1.0e9, 1.0e-9] {
            let tuning = StringTuning::solve(RATE, hz, f32::NAN, f32::INFINITY, -1.0, f32::NAN);
            assert!(tuning.length >= 1);
            assert!(tuning.gain > 0.0 && tuning.gain < 1.0);
            assert!(tuning.release_gain <= tuning.gain);
            let mut line = StringLine::new(RATE);
            line.pluck(tuning, f32::NAN, f32::NAN, f32::NAN);
            for _ in 0..2_000 {
                let s = line.process();
                assert!(s.is_finite() && s.abs() < 10.0, "hz {hz}: {s}");
            }
        }
    }

    #[test]
    fn extreme_settings_stay_stable() {
        for (blend, stiffness, t60) in [(0.0, 1.0, 25.0), (0.5, 1.0, 25.0), (0.0, 0.0, 1.0e6)] {
            let signal = render(110.0, blend, stiffness, t60, 96_000);
            assert!(signal.iter().all(|s| s.is_finite() && s.abs() < 4.0));
        }
    }

    #[test]
    fn low_notes_fit_the_delay_line() {
        let mut line = StringLine::new(96_000.0);
        let tuning = StringTuning::solve(96_000.0, 8.18, 0.3, 0.5, 5.0, 0.1);
        assert!(tuning.length < 96_000 / 8 + 8);
        line.pluck(tuning, 0.5, 0.5, 0.3);
        assert!((0..4_000).all(|_| line.process().is_finite()));
    }

    #[test]
    fn repluck_at_the_same_pitch_keeps_ringing() {
        let mut line = StringLine::new(RATE);
        let tuning = StringTuning::solve(RATE, 330.0, 0.3, 0.0, 4.0, 0.1);
        line.pluck(tuning, 0.6, 0.5, 0.3);
        for _ in 0..3_000 {
            line.process();
        }
        line.pluck(tuning, 0.6, 0.5, 0.3);
        let peak = (0..2_000)
            .map(|_| line.process().abs())
            .fold(0.0_f32, f32::max);
        assert!(peak > 0.05);
    }
}
