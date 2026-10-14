//! Polyphonic octaves by instantaneous phase scaling in 80 analytic bands.
//!
//! Etienne Thuillier, "Real-time polyphonic octave doubling for the guitar",
//! Aalto University, 2016, <https://aaltodoc.aalto.fi/handle/123456789/20158>,
//! motivates the ERB-spaced complex analysis bank and octave-up phase scaling.
//! The spacing uses Glasberg and Moore, "Derivation of auditory filter shapes
//! from notched-noise data", 1990, <https://doi.org/10.1016/0378-5955(90)90170-T>:
//! `E(f) = 21.4 log10(1 + 0.00437 f)`.
//!
//! Terrarium Poly Octave (MIT),
//! <https://github.com/schult/terrarium-poly-octave/blob/main/util/OctaveGenerator.h>
//! and <https://github.com/schult/terrarium-poly-octave/blob/main/util/BandShifter.h>,
//! supplies the harmonic-mean bandwidth rule and wrap-continuous half-angle
//! dividers for two octaves down. Both files were read in full. The equations
//! are implemented here with ordinary square roots. The empirical centre curve
//! is replaced by ERB spacing. The RBJ low-pass prototype,
//! <https://webaudio.github.io/Audio-EQ-Cookbook/audio-eq-cookbook.html>, is
//! rotated into a complex band-pass by Noga, "Complex band-pass filters for
//! analytic signal generation and their application", section 3.1,
//! <https://apps.dtic.mil/sti/tr/pdf/ADA395963.pdf>.
//!
//! Analogue context: <https://en.wikipedia.org/wiki/Octave_effect> and The Valve
//! Wizard's U-Boat, <https://www.valvewizard.co.uk/uboat.html>, describe octave
//! effects and the single-note OC-2 divider approach. The U-Boat description
//! explains alternate-cycle inversion and why competing chord crossings defeat
//! a single divider; here each band has its own continuous phase divider.
//!
//! Processing stays at the host rate: decimation and interpolation would spend
//! about a millisecond of the input path's 2.7 ms budget on resampling filters.
//! The dry mono signal is undelayed, so `latency()` is zero. Octave response is
//! the analysis bands' sound, approximately `2.4 / (pi * bandwidth)`: with
//! width one, about 24 ms at 1 kHz, 100 ms at 60 Hz, and 230 ms at the
//! 30 Hz floor with a bass range ending near 500 Hz. Response depends on
//! both range endpoints. Width ranges from 0.9 to 1.2, default one;
//! wider bands respond sooner but adjacent notes leak and sound rougher.
//! Up2's level varies by about 9 dB across the bank because neighbouring bands,
//! 180 degrees apart at their crossover, do not stay aligned between crossovers
//! when their phase is multiplied by four.
//! A redesign clears and warms the spare bank for `3 / (pi * bandwidth_min)`
//! before a 10 ms crossfade. Each bank carries its own unity calibration.
//!
//! Terrarium's permission notice is retained for the adapted band equations:
//!
//! MIT License
//!
//! Copyright (c) 2024 Steven Schulteis
//!
//! Permission is hereby granted, free of charge, to any person obtaining a copy
//! of this software and associated documentation files (the "Software"), to deal
//! in the Software without restriction, including without limitation the rights
//! to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//! copies of the Software, and to permit persons to whom the Software is
//! furnished to do so, subject to the following conditions:
//!
//! The above copyright notice and this permission notice shall be included in all
//! copies or substantial portions of the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//! SOFTWARE.

mod band;
mod level;
#[cfg(test)]
mod tests;

use band::Band;
use level::LevelMatch;

use crate::drive::kit::{DEFAULT_RATE, Knobs, for_each_frame, for_each_frame_wet};
use crate::dsp::{OnePole, db_to_gain, sane_rate};
use crate::{Context, Curve, Effect, EffectKind, ParamSpec, WetOut};

const BANDS: usize = 80;
const N: usize = 10;
const RATIOS: [f64; 4] = [2.0, 4.0, 0.5, 0.25];

const fn knob(
    name: &'static str,
    min: f32,
    max: f32,
    default: f32,
    unit: &'static str,
    curve: Curve,
) -> ParamSpec {
    ParamSpec {
        name,
        min,
        max,
        default,
        unit,
        curve,
    }
}

static PARAMS: [ParamSpec; N] = [
    knob("low", 30.0, 500.0, 60.0, "Hz", Curve::Log),
    knob("high", 500.0, 5000.0, 2000.0, "Hz", Curve::Log),
    knob("width", 0.9, 1.2, 1.0, "", Curve::Linear),
    knob("dry", 0.0, 200.0, 100.0, "%", Curve::Linear),
    knob("up1", 0.0, 200.0, 0.0, "%", Curve::Linear),
    knob("up2", 0.0, 200.0, 0.0, "%", Curve::Linear),
    knob("down1", 0.0, 200.0, 100.0, "%", Curve::Linear),
    knob("down2", 0.0, 200.0, 0.0, "%", Curve::Linear),
    knob("tone", 500.0, 16_000.0, 8000.0, "Hz", Curve::Log),
    knob("level", -24.0, 12.0, 0.0, "dB", Curve::Linear),
];

pub(super) const KIND: EffectKind = EffectKind {
    id: "octaves",
    name: "Octaves",
    description: "Add two octaves up and down to chords through an analytic band bank.",
    params: &PARAMS,
    build: || Box::new(Octaves::new()),
};

fn erb(frequency: f64) -> f64 {
    21.4 * 0.00437_f64.mul_add(frequency, 1.0).log10()
}

fn centre(low: f64, high: f64, position: f64) -> f64 {
    let scale = (erb(high) - erb(low)).mul_add(position / (BANDS - 1) as f64, erb(low));
    (10.0_f64.powf(scale / 21.4) - 1.0) / 0.00437
}

#[derive(Debug, Default)]
struct Bank {
    bands: Vec<Band>,
    gains: [f32; 4],
    input_scale: f64,
    levels: [LevelMatch; 2],
    settling: usize,
    down1_sign: f32,
    down2_turn: usize,
}

impl Bank {
    fn prepare(&mut self, settings: [f32; 3], rate: f32) {
        self.bands.resize(BANDS, Band::new());
        self.design(settings, rate);
        self.reset();
    }

    fn design(&mut self, settings: [f32; 3], rate: f32) {
        let high = f64::from(settings[1]).min(f64::from(rate) * 0.45);
        // The endpoint knobs can meet at 500 Hz; retain a finite one-Hz span.
        let low = f64::from(settings[0]).min(high - 1.0);
        let ratio = 10.0_f64.powf((erb(high) - erb(low)) / (21.4 * (BANDS - 1) as f64));
        let offset = 1.0 / 0.00437;
        let mut frequency = low;
        let mut minimum = f64::INFINITY;
        for band in &mut self.bands {
            let below = (frequency + offset) * (1.0 - 1.0 / ratio);
            let above = (frequency + offset) * (ratio - 1.0);
            let bandwidth = 2.0 * below * above / (below + above) * f64::from(settings[2]);
            band.design(frequency, bandwidth, f64::from(rate));
            minimum = minimum.min(bandwidth);
            frequency = (frequency + offset).mul_add(ratio, -offset);
        }
        self.settling = (3.0 * f64::from(rate) / (std::f64::consts::PI * minimum)).ceil() as usize;
        self.gains = self.calibrate(settings, rate);
        let frequency = centre(low, high, 40.0);
        // Summing band energies avoids making the reference depend on their phases.
        let power = self
            .bands
            .iter()
            .map(|band| {
                let (real, imag) = band.response(frequency, f64::from(rate));
                0.25 * real.mul_add(real, imag * imag)
            })
            .sum::<f64>();
        self.input_scale = 1.0 / power;
        for level in &mut self.levels {
            level.prepare(rate);
        }
    }

    fn reset(&mut self) {
        self.down1_sign = 1.0;
        self.down2_turn = 0;
        for level in &mut self.levels {
            level.reset();
        }
        for band in &mut self.bands {
            band.reset();
        }
    }

    fn sample(&mut self, input: f32, enabled: [bool; 4], audible: bool) -> BankSample {
        let mut sum = [0.0; 4];
        let mut down = [(0.0, 0.0); 2];
        let mut input_power = 0.0;
        for band in &mut self.bands {
            let (paths, divided, power) = band.sample(input, enabled, audible);
            input_power += power;
            for (sum, path) in sum.iter_mut().zip(paths) {
                *sum += path;
            }
            for (sum, path) in down.iter_mut().zip(divided) {
                sum.0 += path.0;
                sum.1 += path.1;
            }
        }
        down[0] = (down[0].0 * self.down1_sign, down[0].1 * self.down1_sign);
        down[1] = quarter_turn(down[1], self.down2_turn);
        sum[2] = down[0].0;
        sum[3] = down[1].0;
        let mut paths = std::array::from_fn(|path| sum[path] * self.gains[path]);
        for path in 0..2 {
            if enabled[path + 2] {
                let (real, imag) = down[path];
                let output_power =
                    2.0 * f64::from(real).powi(2) * f64::from(self.gains[path + 2]).powi(2)
                        / self.input_scale;
                let gain = self.levels[path].sample(input_power, output_power) as f32;
                paths[path + 2] *= gain;
                down[path] = (real * gain, imag * gain);
            }
        }
        if !audible {
            paths = [0.0; 4];
        }
        BankSample { paths, down }
    }

    fn align(&mut self, active: &BankSample, spare: &mut BankSample) {
        if dot(active.down[0], spare.down[0]) < 0.0 {
            self.down1_sign = -self.down1_sign;
            spare.down[0] = (-spare.down[0].0, -spare.down[0].1);
            spare.paths[2] = -spare.paths[2];
        }
        let mut best = 0;
        let mut score = dot(active.down[1], spare.down[1]);
        for turn in 1..4 {
            let candidate = dot(active.down[1], quarter_turn(spare.down[1], turn));
            if candidate > score {
                score = candidate;
                best = turn;
            }
        }
        self.down2_turn = (self.down2_turn + best) % 4;
        spare.down[1] = quarter_turn(spare.down[1], best);
        spare.paths[3] = spare.down[1].0 * self.gains[3];
    }

    fn calibrate(&self, settings: [f32; 3], rate: f32) -> [f32; 4] {
        let high = f64::from(settings[1]).min(f64::from(rate) * 0.45);
        let frequency = centre(f64::from(settings[0]).min(high - 1.0), high, 40.0);
        let mut sums = [(0.0_f64, 0.0_f64); 4];
        for band in &self.bands {
            let (real, imag) = band.response(frequency, f64::from(rate));
            // A real sine has half its amplitude in the positive-frequency band.
            let amplitude = 0.5 * real.hypot(imag);
            let phase = imag.atan2(real);
            for (sum, ratio) in sums.iter_mut().zip(RATIOS) {
                let (sin, cos) = (phase * ratio).sin_cos();
                sum.0 += amplitude * cos;
                sum.1 += amplitude * sin;
            }
        }
        sums.map(|(real, imag)| {
            let amplitude = real.hypot(imag);
            if amplitude.is_finite() && amplitude > 1e-9 {
                (1.0 / amplitude) as f32
            } else {
                // A singular bank has no calibratable output; silence its paths.
                0.0
            }
        })
    }
}

#[derive(Debug, Default)]
struct BankSample {
    paths: [f32; 4],
    down: [(f32, f32); 2],
}

fn dot(left: (f32, f32), right: (f32, f32)) -> f32 {
    left.0.mul_add(right.0, left.1 * right.1)
}

const fn quarter_turn((real, imag): (f32, f32), turn: usize) -> (f32, f32) {
    match turn {
        1 => (-imag, real),
        2 => (-real, -imag),
        3 => (imag, -real),
        _ => (real, imag),
    }
}

#[derive(Debug)]
struct Octaves {
    knobs: Knobs<N>,
    banks: [Bank; 2],
    rate: f32,
    active: usize,
    requested: [f32; 3],
    designed: [f32; 3],
    quiet: usize,
    hold: usize,
    fade: usize,
    fade_length: usize,
    warming: usize,
    tone: OnePole,
    tone_hz: f32,
    level_db: f32,
    level: f32,
}

impl Octaves {
    fn new() -> Self {
        Self {
            knobs: Knobs::new(&PARAMS),
            banks: [Bank::default(), Bank::default()],
            rate: DEFAULT_RATE,
            active: 0,
            requested: [60.0, 2000.0, 1.0],
            designed: [60.0, 2000.0, 1.0],
            quiet: 0,
            hold: 2400,
            fade: 0,
            fade_length: 480,
            warming: 0,
            tone: OnePole::default(),
            tone_hz: f32::NAN,
            level_db: f32::NAN,
            level: 1.0,
        }
    }

    fn retune(&mut self) {
        self.quiet = self.quiet.saturating_add(1).min(self.hold);
        let changed = self.requested.map(f32::to_bits) != self.designed.map(f32::to_bits);
        if changed && self.quiet >= self.hold && self.fade == 0 && self.warming == 0 {
            let spare = &mut self.banks[1 - self.active];
            spare.design(self.requested, self.rate);
            spare.reset();
            self.designed = self.requested;
            self.warming = spare.settling;
        }
    }

    fn clear_audio(&mut self) {
        for bank in &mut self.banks {
            bank.reset();
        }
        self.tone.reset();
    }

    fn sample(&mut self, left: f32, right: f32) -> f32 {
        self.frame(left, right)[0]
    }

    fn frame(&mut self, left: f32, right: f32) -> [f32; 2] {
        let params = self.knobs.step();
        self.retune();
        if params[8].to_bits() != self.tone_hz.to_bits() {
            self.tone_hz = params[8];
            self.tone.set_cutoff(self.tone_hz, self.rate);
        }
        if params[9].to_bits() != self.level_db.to_bits() {
            self.level_db = params[9];
            self.level = db_to_gain(self.level_db);
        }
        if !left.is_finite() || !right.is_finite() {
            self.clear_audio();
            return [0.0; 2];
        }
        let mono = 0.5 * (left.clamp(-16.0, 16.0) + right.clamp(-16.0, 16.0));
        let enabled = std::array::from_fn(|path| params[4 + path] > 0.0);
        let active = self.banks[self.active].sample(mono, enabled, true);
        let mut spare = if self.warming > 0 || self.fade > 0 {
            self.banks[1 - self.active].sample(mono, enabled, self.fade > 0)
        } else {
            BankSample::default()
        };
        if self.fade == self.fade_length {
            self.banks[1 - self.active].align(&active, &mut spare);
        }
        let blend = if self.fade > 0 {
            1.0 - self.fade as f32 / self.fade_length as f32
        } else {
            0.0
        };
        let mut octaves = 0.0;
        for path in 0..4 {
            let sum = (spare.paths[path] - active.paths[path]).mul_add(blend, active.paths[path]);
            octaves = (params[4 + path] * 0.01).mul_add(sum, octaves);
        }
        if self.fade > 0 {
            self.fade -= 1;
            if self.fade == 0 {
                self.active = 1 - self.active;
            }
        }
        if self.warming > 0 {
            self.warming -= 1;
            if self.warming == 0 {
                self.fade = self.fade_length;
            }
        }
        let wet = self.level * self.tone.lowpass(octaves);
        let output = (self.level * params[3] * 0.01).mul_add(mono, wet);
        if output.is_finite() && wet.is_finite() {
            [output.clamp(-4.0, 4.0), wet.clamp(-4.0, 4.0)]
        } else {
            self.clear_audio();
            [0.0; 2]
        }
    }
}

impl Effect for Octaves {
    fn prepare(&mut self, sample_rate: f32) {
        self.rate = sane_rate(sample_rate);
        self.knobs.prepare(self.rate);
        let params = self.knobs.values();
        self.requested = [params[0], params[1], params[2]];
        for bank in &mut self.banks {
            bank.prepare(self.requested, self.rate);
        }
        self.hold = (self.rate * 0.05).ceil() as usize;
        self.fade_length = (self.rate * 0.01).ceil() as usize;
        self.reset();
    }

    fn reset(&mut self) {
        self.knobs.settle();
        let params = self.knobs.values();
        self.requested = [params[0], params[1], params[2]];
        self.designed = self.requested;
        for bank in &mut self.banks {
            bank.design(self.requested, self.rate);
        }
        self.active = 0;
        self.fade = 0;
        self.warming = 0;
        self.quiet = self.hold;
        self.tone_hz = params[8];
        self.tone.set_cutoff(self.tone_hz, self.rate);
        self.level_db = params[9];
        self.level = db_to_gain(self.level_db);
        self.clear_audio();
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if !value.is_finite() {
            return;
        }
        self.knobs.set(index, value);
        if let (Some(spec), Some(requested)) = (PARAMS.get(index), self.requested.get_mut(index)) {
            let value = spec.clamp(value);
            if value.to_bits() != requested.to_bits() {
                *requested = value;
                self.quiet = 0;
            }
        }
    }

    fn process(&mut self, _context: &Context, input: [&[f32]; 2], output: [&mut [f32]; 2]) {
        for_each_frame(input, output, |left, right| [self.sample(left, right); 2]);
    }

    fn process_wet(
        &mut self,
        _context: &Context,
        input: [&[f32]; 2],
        output: [&mut [f32]; 2],
        wet: [&mut [f32]; 2],
    ) {
        for_each_frame_wet(input, output, WetOut::to(wet), |left, right| {
            let [mixed, alone] = self.frame(left, right);
            [[mixed; 2], [alone; 2]]
        });
    }
}
