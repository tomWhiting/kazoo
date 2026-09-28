//! `eq`: a musical equaliser, the kind a mixing console's channel strip has.
//!
//! From the bottom up: a low cut, a low shelf, two fully parametric bells, a
//! high shelf, a tilt, a high cut and an output gain.
//!
//! Every band is a second-order filter designed by magnitude matching
//! (Vicanek, *Matched Second Order Digital Filters*, 2016; see `matched`),
//! not the usual bilinear-transform cookbook. The difference shows at the
//! top of the range: a bilinear bell set at 15 kHz comes out narrower and
//! lopsided because the whole analogue frequency axis has been squeezed in
//! below Nyquist ("cramping"). A matched bell keeps its analogue shape: its
//! gain at the centre is exactly the knob's, its width is the one the Q asks
//! for, and it does not pull the response to zero at Nyquist. That is how an
//! analogue desk's EQ sounds in the air band, and why a high shelf here
//! sounds open rather than brittle.
//!
//! The shapes are the classic analogue ones: bells whose boost and cut are
//! mirror images, shelves with no overshoot (a slope of Q 0.7071), and
//! Butterworth 12 dB-per-octave cuts. The tilt is a gentle first-order
//! seesaw around 1 kHz, half its gain each way: tip the whole mix darker or
//! brighter with one knob.
//!
//! The low cut is out at its lowest setting and the high cut at its
//! highest; moving either off its end fades the filter in over 20 ms, so it
//! never clicks. Every knob glides and the filters are redesigned as it
//! does, with their state kept, so sweeps are smooth. Coefficients and
//! state are `f64`, so even a 20 Hz shelf at 192 kHz is clean.
//!
//! A safety limit on the output leaves everything up to +3.5 dBFS
//! untouched and bends anything louder (an 18 dB boost on a hot track)
//! smoothly toward +6 dBFS, which it never passes.

use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

use super::kit::{
    CONTROL, Knobs, Ramp, clean, frames, silence, soft_limit, unchanged, usable_rate,
};
use super::matched::{self, Biquad, Coeffs, FirstOrder};

const LOW_CUT: usize = 0;
const LOW_FREQ: usize = 1;
const LOW_GAIN: usize = 2;
const FREQ1: usize = 3;
const GAIN1: usize = 4;
const Q1: usize = 5;
const FREQ2: usize = 6;
const GAIN2: usize = 7;
const Q2: usize = 8;
const HIGH_FREQ: usize = 9;
const HIGH_GAIN: usize = 10;
const TILT: usize = 11;
const HIGH_CUT: usize = 12;
const OUTPUT: usize = 13;

/// The low cut is out below this, the high cut above that.
const LOW_CUT_OFF: f32 = 20.0;
const HIGH_CUT_OFF: f32 = 20_000.0;
/// The tilt's pivot.
const PIVOT: f64 = 1_000.0;
/// The output's safety limit: untouched up to +3.5 dBFS (so the EQ is
/// transparent to anything but a big boost on a hot signal), then bending
/// toward +6 dBFS, which it never passes.
const SAFE_KNEE: f32 = 1.5;
const SAFE_TOP: f32 = 2.0;
/// Shelf slope: the steepest with no overshoot.
const SHELF_Q: f64 = std::f64::consts::FRAC_1_SQRT_2;

const fn gain(name: &'static str, default: f32) -> ParamSpec {
    ParamSpec {
        name,
        min: -18.0,
        max: 18.0,
        default,
        unit: "dB",
        curve: Curve::Linear,
    }
}

const fn freq(name: &'static str, min: f32, max: f32, default: f32) -> ParamSpec {
    ParamSpec {
        name,
        min,
        max,
        default,
        unit: "Hz",
        curve: Curve::Log,
    }
}

const fn q(name: &'static str) -> ParamSpec {
    ParamSpec {
        name,
        min: 0.2,
        max: 10.0,
        default: 0.8,
        unit: "",
        curve: Curve::Log,
    }
}

static PARAMS: [ParamSpec; 14] = [
    freq("lowcut", LOW_CUT_OFF, 1_000.0, LOW_CUT_OFF),
    freq("lowfreq", 20.0, 1_000.0, 100.0),
    gain("lowgain", 0.0),
    freq("freq1", 30.0, 16_000.0, 400.0),
    gain("gain1", 0.0),
    q("q1"),
    freq("freq2", 200.0, 20_000.0, 3_000.0),
    gain("gain2", 0.0),
    q("q2"),
    freq("highfreq", 1_000.0, 20_000.0, 8_000.0),
    gain("highgain", 0.0),
    ParamSpec {
        name: "tilt",
        min: -6.0,
        max: 6.0,
        default: 0.0,
        unit: "dB",
        curve: Curve::Linear,
    },
    freq("highcut", 1_000.0, HIGH_CUT_OFF, HIGH_CUT_OFF),
    gain("output", 0.0),
];

const GLIDES: [f32; 14] = [0.03; 14];

/// The EQ's entry in the catalogue.
pub static KIND: EffectKind = EffectKind {
    id: "eq",
    name: "EQ",
    description: "A console-style EQ: cuts, shelves, two parametric bells and a tilt, with analogue-matched curves right up to the top octave.",
    params: &PARAMS,
    build: || Box::new(Eq::new()),
};

/// One channel's chain of filters.
#[derive(Debug, Clone, Copy, Default)]
struct Channel {
    low_cut: Biquad,
    low: Biquad,
    bell1: Biquad,
    bell2: Biquad,
    high: Biquad,
    tilt: FirstOrder,
    high_cut: Biquad,
}

impl Channel {
    fn reset(&mut self) {
        for filter in [
            &mut self.low_cut,
            &mut self.low,
            &mut self.bell1,
            &mut self.bell2,
            &mut self.high,
            &mut self.high_cut,
        ] {
            filter.reset();
        }
        self.tilt.reset();
    }

    fn process(&mut self, input: f64, low_cut: f64, high_cut: f64, gain: f64) -> f64 {
        let cut = self.low_cut.process(input);
        let mut x = (cut - input).mul_add(low_cut, input);
        x = self.low.process(x);
        x = self.bell1.process(x);
        x = self.bell2.process(x);
        x = self.high.process(x);
        x = self.tilt.process(x);
        let cut = self.high_cut.process(x);
        x = (cut - x).mul_add(high_cut, x);
        x * gain
    }
}

/// The EQ.
#[derive(Debug, Clone)]
pub struct Eq {
    rate: f32,
    prepared: bool,
    knobs: Knobs<14>,
    channels: [Channel; 2],
    low_cut_in: Ramp,
    high_cut_in: Ramp,
    designed: [f32; 13],
    gain: f64,
    gain_step: f64,
    countdown: usize,
}

impl Default for Eq {
    fn default() -> Self {
        Self::new()
    }
}

impl Eq {
    /// A flat EQ, unprepared.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rate: 0.0,
            prepared: false,
            knobs: Knobs::new(&PARAMS),
            channels: [Channel::default(); 2],
            low_cut_in: Ramp::new(0.0),
            high_cut_in: Ramp::new(0.0),
            designed: [f32::NAN; 13],
            gain: 1.0,
            gain_step: 0.0,
            countdown: 0,
        }
    }

    /// The output knob as a gain.
    fn output_gain(&self) -> f64 {
        10f64.powf(f64::from(self.knobs.get(OUTPUT)) / 20.0)
    }

    /// Whether each cut should be in, from its knob's target.
    fn cuts_wanted(&self) -> (f32, f32) {
        let low = if self.knobs.target(LOW_CUT) > LOW_CUT_OFF * 1.001 {
            1.0
        } else {
            0.0
        };
        let high = if self.knobs.target(HIGH_CUT) < HIGH_CUT_OFF * 0.999 {
            1.0
        } else {
            0.0
        };
        (low, high)
    }

    fn update(&mut self) {
        let (low, high) = self.cuts_wanted();
        let fade = 0.02 * self.rate;
        if (self.low_cut_in.target() - low).abs() > f32::EPSILON {
            self.low_cut_in.set(low, fade);
        }
        if (self.high_cut_in.target() - high).abs() > f32::EPSILON {
            self.high_cut_in.set(high, fade);
        }
        // The output gain heads for the knob over the next period.
        self.gain_step = (self.output_gain() - self.gain) / CONTROL as f64;
        let wanted: [f32; 13] = std::array::from_fn(|i| self.knobs.get(i));
        if unchanged(&wanted, &self.designed) {
            return;
        }
        self.designed = wanted;
        let k = |i: usize| f64::from(wanted[i]);
        let rate = f64::from(self.rate);
        let butterworth = std::f64::consts::FRAC_1_SQRT_2;
        let designs: [Coeffs; 6] = [
            matched::highpass(k(LOW_CUT), butterworth, rate),
            matched::low_shelf(k(LOW_FREQ), k(LOW_GAIN), SHELF_Q, rate),
            matched::peaking(k(FREQ1), k(GAIN1), k(Q1), rate),
            matched::peaking(k(FREQ2), k(GAIN2), k(Q2), rate),
            matched::high_shelf(k(HIGH_FREQ), k(HIGH_GAIN), SHELF_Q, rate),
            matched::lowpass(k(HIGH_CUT), butterworth, rate),
        ];
        let tilt = matched::tilt(PIVOT, k(TILT), rate);
        for channel in &mut self.channels {
            channel.low_cut.set(designs[0]);
            channel.low.set(designs[1]);
            channel.bell1.set(designs[2]);
            channel.bell2.set(designs[3]);
            channel.high.set(designs[4]);
            channel.high_cut.set(designs[5]);
            channel.tilt.set(tilt);
        }
    }

    fn frame(&mut self, left: f32, right: f32) -> (f32, f32) {
        self.knobs.step();
        if self.countdown == 0 {
            self.update();
            self.countdown = CONTROL;
        }
        self.countdown -= 1;
        let low_cut = f64::from(self.low_cut_in.next());
        let high_cut = f64::from(self.high_cut_in.next());
        self.gain += self.gain_step;
        let gain = self.gain;
        let [l, r] = [clean(left), clean(right)];
        let out_l = self.channels[0].process(f64::from(l), low_cut, high_cut, gain);
        let out_r = self.channels[1].process(f64::from(r), low_cut, high_cut, gain);
        (
            soft_limit(out_l as f32, SAFE_KNEE, SAFE_TOP),
            soft_limit(out_r as f32, SAFE_KNEE, SAFE_TOP),
        )
    }
}

impl Effect for Eq {
    fn prepare(&mut self, sample_rate: f32) {
        let Some(rate) = usable_rate(sample_rate) else {
            self.prepared = false;
            return;
        };
        self.rate = rate;
        self.knobs.prepare(rate, &GLIDES);
        let (low, high) = self.cuts_wanted();
        self.low_cut_in.snap(low);
        self.high_cut_in.snap(high);
        self.gain = self.output_gain();
        self.gain_step = 0.0;
        self.prepared = true;
        self.reset();
    }

    fn reset(&mut self) {
        for channel in &mut self.channels {
            channel.reset();
        }
        self.designed = [f32::NAN; 13];
        self.countdown = 0;
    }

    fn set_param(&mut self, index: usize, value: f32) {
        self.knobs.set(index, value);
    }

    fn process(&mut self, _context: &Context, input: [&[f32]; 2], mut output: [&mut [f32]; 2]) {
        if !self.prepared {
            silence(&mut output);
            return;
        }
        let count = frames(input, &mut output);
        let [out_left, out_right] = output;
        for n in 0..count {
            let (l, r) = self.frame(input[0][n], input[1][n]);
            out_left[n] = l;
            out_right[n] = r;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{self, RATE, build, rms, run_mono, sine};

    /// The EQ's gain at `hz` in dB, measured with a steady sine.
    fn measure(knobs: &[(&str, f32)], hz: f32) -> f32 {
        let mut eq = build("eq", knobs);
        let tone = sine(0.4, hz, 0.1);
        let (left, right) = run_mono(eq.as_mut(), &tone);
        let settled = (0.2 * RATE) as usize;
        let out = rms(&left[settled..]).max(rms(&right[settled..]));
        20.0 * (out / rms(&tone[settled..])).log10()
    }

    #[test]
    fn the_eq_keeps_the_contract() {
        testing::contract("eq");
    }

    #[test]
    fn a_flat_eq_passes_a_hot_signal_untouched() {
        let mut eq = build("eq", &[]);
        let tone = sine(0.5, 1_000.0, 1.4);
        let (left, _) = run_mono(eq.as_mut(), &tone);
        let worst = left
            .iter()
            .zip(&tone)
            .fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
        assert!(worst < 1e-5, "{worst}");
    }

    #[test]
    fn the_eq_is_the_same_at_every_rate_with_the_air_boosted() {
        testing::rates_agree_with(
            "eq",
            &[
                ("freq2", 15_000.0),
                ("gain2", 12.0),
                ("highgain", 6.0),
                ("tilt", 3.0),
            ],
        );
    }

    #[test]
    fn all_gains_at_zero_is_flat() {
        for hz in [20.0, 60.0, 200.0, 1_000.0, 5_000.0, 12_000.0, 19_000.0] {
            let db = measure(&[], hz);
            assert!(db.abs() < 0.01, "{hz} Hz: {db} dB");
        }
    }

    /// Knob settings, the frequency to measure and the gain expected there.
    type Case = (&'static [(&'static str, f32)], f32, f32);

    #[test]
    fn each_band_hits_its_knob_at_its_centre() {
        let cases: [Case; 7] = [
            (
                &[("freq1", 1_000.0), ("gain1", 9.0), ("q1", 1.0)],
                1_000.0,
                9.0,
            ),
            (
                &[("freq1", 150.0), ("gain1", -12.0), ("q1", 4.0)],
                150.0,
                -12.0,
            ),
            (
                &[("freq2", 15_000.0), ("gain2", -12.0), ("q2", 2.0)],
                15_000.0,
                -12.0,
            ),
            (
                &[("freq2", 18_000.0), ("gain2", 18.0), ("q2", 0.7)],
                18_000.0,
                18.0,
            ),
            (&[("lowfreq", 300.0), ("lowgain", 12.0)], 25.0, 12.0),
            (&[("highfreq", 2_000.0), ("highgain", -9.0)], 19_000.0, -9.0),
            (&[("output", 6.0)], 1_000.0, 6.0),
        ];
        for (knobs, hz, want) in cases {
            let db = measure(knobs, hz);
            assert!(
                (db - want).abs() < 0.5,
                "{knobs:?} at {hz} Hz: {db} dB, wanted {want}"
            );
        }
    }

    #[test]
    fn a_shelf_is_half_its_gain_at_its_corner() {
        let db = measure(&[("highfreq", 10_000.0), ("highgain", 12.0)], 10_000.0);
        assert!((db - 6.0).abs() < 0.5, "{db}");
    }

    #[test]
    fn the_tilt_seesaws_about_the_pivot() {
        let low = measure(&[("tilt", 6.0)], 20.0);
        let pivot = measure(&[("tilt", 6.0)], 1_000.0);
        let high = measure(&[("tilt", 6.0)], 19_000.0);
        assert!((low + 3.0).abs() < 0.3, "{low}");
        assert!(pivot.abs() < 0.3, "{pivot}");
        assert!((high - 3.0).abs() < 0.4, "{high}");
    }

    #[test]
    fn the_cuts_are_three_decibels_down_at_their_corner_and_out_at_their_ends() {
        assert!((measure(&[("lowcut", 100.0)], 100.0) + 3.0).abs() < 0.3);
        assert!((measure(&[("highcut", 5_000.0)], 5_000.0) + 3.0).abs() < 0.3);
        assert!(measure(&[("lowcut", 100.0)], 25.0) < -20.0);
    }
}
