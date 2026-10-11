//! `pitch`: a clean pitch shifter for music, in semitones, that can keep a
//! voice's formants where they are.
//!
//! # The model
//!
//! **Why in the time domain.** A phase vocoder (an FFT, every bin's phase
//! advanced) shifts anything, but it needs a window of 40 ms or more to
//! resolve a low voice, and it smears transients and phases out the
//! harmonics of a single voice into a hollow, "phasey" sound. A delay line
//! read at a moving delay keeps the waveform itself, so a voice stays a
//! voice, and it can answer in a few tens of milliseconds. The cost of the
//! time domain is the splice: where one read hands over to the next. Here
//! the splices are few, short and matched.
//!
//! **The shifter.** A delay line read at a delay that changes by `1 - r`
//! samples every sample plays back at `r` times the pitch. One tap is heard
//! at a time. When it nears the end of its travel (the newest audio, going
//! up; the oldest kept for it, going down) a new tap starts a jump away
//! (25 ms, and further, in step, for shifts of more than an octave up),
//! slid by up to 8 ms either way to wherever its audio best matches the
//! heard tap's, as in Verhelst and Roelands' WSOLA (the same alignment as
//! `shimmer`'s: coarse, then to the sample), and the two crossfade over
//! 12 ms (over less for big shifts up, and sooner whenever the shift speeds
//! up mid-crossfade, so the fading tap never runs out of audio while it is
//! still heard), equal-amplitude for audio that matches and equal-power for
//! audio that does not. A new tap travels at least 17 ms of audio before
//! the next splice (a 25 ms jump less the 8 ms slide), so in steady going
//! the splices come at most about 59 times a second, an octave up; fewer
//! for smaller shifts and for shifts down. Between splices only one tap
//! plays, so a held note comes through as it went in, with no comb from
//! two taps a few milliseconds apart, and with no shift at all the effect
//! is a plain delay. Both channels share the taps, aligned on their sum,
//! so the stereo image holds.
//!
//! **Attacks.** A shift up jumps back over audio already played and a
//! shift down jumps forward over audio not yet played, so a careless
//! splice plays a drum hit twice or loses it. Attacks are flagged as they
//! come in (a fast envelope leaping over a slow one), and every attack is
//! played whole by exactly one tap (see `taps`): going up, a new tap starts
//! just after the latest attack in the stretch it would replay (so a splice
//! can then come sooner than 17 ms); going down, a splice waits for an
//! attack the heard tap will reach within 10 ms, and otherwise jumps only as
//! far as just before the attack it would skip; and a crossfade ends before
//! either tap reaches an attack still ahead of it, so the attack comes
//! neither at part strength from a tap fading in nor twice. A click every
//! 150 ms comes out once per click at every shift tested, two octaves
//! either way.
//!
//! **Aliasing.** Reading faster than real time raises every frequency, and
//! whatever would land past Nyquist folds back as grit, so a shift up first
//! lowpasses the input (an eighth-order Butterworth guard by the bilinear
//! transform, as `shift`'s) at 42% of the rate over the ratio: what is
//! raised lands below 42% of the rate, clear of the top. At no shift the
//! guard sits there too, above 18 kHz from 44.1 kHz up; a shift down needs
//! none, and the guard opens to 47%. The guard follows the shift as it
//! moves, so it is built from Zavalishin's topology-preserving
//! state-variable sections, whose corner can glide every sample without a
//! click (a direct-form biquad redesigned under moving audio leaves a
//! small step in it each time).
//!
//! **Formants.** Shifting a voice moves its formants (the resonances of
//! the throat and mouth that make an "ah" an "ah") along with its pitch:
//! up, a chipmunk; down, a giant. With `formant` on `keep`, the input's
//! envelope is found by linear prediction (see `formant`), flattened out
//! of the audio before it is shifted, and put back on afterwards: the
//! harmonics move, the formants stay (see `keep`). The switch crossfades
//! over 30 ms, so it never clicks. The kept sound is held to the loudness
//! of the plainly shifted sound (within 30 dB): on a voice the envelope
//! changes the timbre and barely the level, but on a near-pure tone the
//! envelope fits the tone itself, so the kept path alone is far quieter
//! and its level depends sharply on the shift, and the match has to make
//! up the difference as the shift moves. Swept by an LFO, a sine's level
//! (in 10 ms windows) stays within 1 dB kept against 0.5 dB moved, and a
//! saw's within 1.3 dB either way (the guard's top moving with the shift);
//! but a sudden jump through no shift bumps a kept tone's level for a few
//! milliseconds, by up to 7 dB (12 to -7 semitones at once; moved, under
//! half a decibel), while the match catches up. So `move`,
//! whose level never depends on the envelope, is the default, and `keep`
//! the choice for a voice. The envelope also follows a change of vowel about 14 ms late
//! (the analysis window is 20 ms long and taken every 4 ms, and nothing is
//! read ahead): through a quick change of vowel the formants glide rather
//! than jump.
//!
//! **Knobs.** `shift` is the interval in semitones, two octaves each way;
//! `fine` adds cents. On the wall a jack on `shift` moves it 24 semitones
//! per 1.0, so a cable at an amount of 0.5 tracks 1.0 per octave. Both
//! glide (30 ms), so a jump bends the pitch rather than clicking, and a
//! sweep is a smooth bend. `mix` is equal-power, from dry to fully
//! shifted.
//!
//! **Latency.** The shifted sound comes out on average 13 ms to 25 ms late
//! for shifts within an octave each way (the mean read delay), up to 50 ms
//! two octaves up; with no shift, 21 ms. The dry path is not delayed, and
//! the effect reports no latency ([`Effect::latency`] stays 0): the delay
//! moves with the `shift` knob and through every splice, while the
//! contract only allows a latency that changes with a stepped knob, and a
//! host holding its dry signal back by the average would still be out of
//! step all the time. What that means in a patch: at `mix` 0.5 with no
//! shift the dry and the 21 ms late copy comb, notches every 48 Hz; a few
//! cents of `fine` for a doubled voice is a doubling 21 ms behind, a flam
//! on anything percussive; and the wet outputs, taken in parallel with the
//! dry sound elsewhere, arrive late in the same way.
//!
//! Sources: W. Verhelst and M. Roelands, "An overlap-add technique based on
//! waveform similarity (WSOLA) for high quality time-scale modification of
//! speech" (ICASSP 1993); J. Makhoul, "Linear prediction: a tutorial
//! review" (Proc. IEEE, 1975), for the envelope; J. P. Bello et al., "A
//! tutorial on onset detection in music signals" (IEEE Trans. Speech and
//! Audio Processing, 2005), for the attacks.

mod keep;
mod taps;

use keep::Keeper;
use taps::{Geometry, Onsets, Taps};

use super::parts::{
    Biquad, Sinc, accept, clean, defaults, equal_power, frames, guard, silence_from,
};
use crate::dsp::{DelayLine, Smoothed, flush, sane_rate};
use crate::{Context, Curve, Effect, EffectKind, ParamSpec, WetOut};

const SHIFT: usize = 0;
const FINE: usize = 1;
const FORMANT: usize = 2;
const MIX: usize = 3;

/// The widest shift, semitones each way, and the fine knob's reach, cents.
const MAX_SHIFT: f32 = 24.0;
const MAX_FINE: f32 = 100.0;

/// The alignment hears the audio through a lowpass here: the fundamental
/// and the first few harmonics are what line up.
const GUIDE_HZ: f32 = 2_500.0;
/// The guard's corner as a share of the rate: over the ratio for a shift
/// up, and wide open for a shift down.
const RAISED_SHARE: f32 = 0.42;
pub(super) const OPEN_SHARE: f32 = 0.47;
/// The damping (`1 / Q`) of the four sections of an eighth-order
/// Butterworth lowpass.
pub(super) const BUTTERWORTH_8: [f32; 4] = [1.961_57, 1.662_939, 1.111_14, 0.390_181];
/// Samples between guard corners worked out; the corner glides between
/// them.
const CONTROL: usize = 32;
/// Glides, seconds: the shift, the formant switch and the mix.
const SHIFT_GLIDE: f32 = 0.03;
const FORMANT_GLIDE: f32 = 0.03;
const MIX_GLIDE: f32 = 0.02;

static PARAMS: [ParamSpec; 4] = [
    ParamSpec {
        name: "shift",
        min: -MAX_SHIFT,
        max: MAX_SHIFT,
        default: 12.0,
        unit: "st",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "fine",
        min: -MAX_FINE,
        max: MAX_FINE,
        default: 0.0,
        unit: "ct",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "formant",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["move", "keep"],
        },
    },
    ParamSpec {
        name: "mix",
        min: 0.0,
        max: 1.0,
        default: 0.5,
        unit: "",
        curve: Curve::Linear,
    },
];

/// The pitch shifter.
pub static KIND: EffectKind = EffectKind {
    id: "pitch",
    name: "Pitch shifter",
    description: "A clean pitch shifter in semitones, two octaves each way; `formant` on \
                  keep holds a voice's formants so it never goes chipmunk. The shifted \
                  sound is about 21 ms behind (13 to 50 ms, with the shift), so mixed \
                  with the dry it combs and doubles.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Effect> {
    Box::new(Pitch::new())
}

/// A lowpass section of Zavalishin's topology-preserving state-variable
/// filter: the bilinear transform of the analogue state-variable filter,
/// built so that its corner can move every sample without a click.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Svf {
    first: f32,
    second: f32,
}

impl Svf {
    /// One sample through at prewarped corner `g` (`tan(π fc / rate)`) and
    /// damping `k` (`1 / Q`).
    pub(super) fn lowpass(&mut self, input: f32, g: f32, k: f32) -> f32 {
        let a1 = 1.0 / g.mul_add(g + k, 1.0);
        let a2 = g * a1;
        let a3 = g * a2;
        let v3 = input - self.second;
        let v1 = a1.mul_add(self.first, a2 * v3);
        let v2 = a2.mul_add(self.first, a3.mul_add(v3, self.second));
        self.first = 2.0f32.mul_add(v1, -self.first);
        self.second = 2.0f32.mul_add(v2, -self.second);
        flush(&mut self.first);
        flush(&mut self.second);
        v2
    }

    pub(super) const fn reset(&mut self) {
        self.first = 0.0;
        self.second = 0.0;
    }
}

/// The guard's prewarped corner: where it is, and how far it moves each
/// sample to reach the next one worked out.
#[derive(Debug, Clone, Copy, Default)]
struct Corner {
    g: f32,
    step: f32,
}

/// One channel's guard and line.
#[derive(Debug, Clone, Default)]
struct Side {
    guard: [Svf; 4],
    /// The guarded input.
    plain: DelayLine,
}

/// The pitch shifter.
#[derive(Debug, Clone)]
pub struct Pitch {
    prepared: bool,
    values: [f32; 4],
    semitones: Smoothed,
    keep: Smoothed,
    mix: Smoothed,
    shape: Geometry,
    sinc: Sinc,
    sides: [Side; 2],
    /// The two sides' sum, lowpassed, for the alignment.
    guide: DelayLine,
    guide_filter: Biquad,
    corner: Corner,
    taps: Taps,
    onsets: Onsets,
    keeper: Keeper,
    countdown: usize,
}

impl Default for Pitch {
    fn default() -> Self {
        Self::new()
    }
}

impl Pitch {
    /// A pitch shifter at its defaults, unprepared.
    #[must_use]
    pub fn new() -> Self {
        let values = defaults(&PARAMS);
        Self {
            prepared: false,
            values,
            semitones: Smoothed::new(values[SHIFT]),
            keep: Smoothed::new(values[FORMANT]),
            mix: Smoothed::new(values[MIX]),
            shape: Geometry::default(),
            sinc: Sinc::default(),
            sides: [Side::default(), Side::default()],
            guide: DelayLine::default(),
            guide_filter: Biquad::new(),
            corner: Corner::default(),
            taps: Taps::default(),
            onsets: Onsets::default(),
            keeper: Keeper::default(),
            countdown: 0,
        }
    }

    /// The interval the knobs ask for, semitones.
    fn wanted(&self) -> f32 {
        self.values[FINE].mul_add(0.01, self.values[SHIFT])
    }

    /// The guard's prewarped corner for the shift now.
    fn guard_corner(&self) -> f32 {
        let ratio = (self.semitones.value() / 12.0).exp2();
        let share = (RAISED_SHARE / ratio).min(OPEN_SHARE);
        (std::f32::consts::PI * share).tan()
    }

    /// Glide the guard's corner to where the shift now asks, over the next
    /// [`CONTROL`] samples.
    fn aim_guard(&mut self) {
        self.corner.step = (self.guard_corner() - self.corner.g) / CONTROL as f32;
    }

    fn render(
        &mut self,
        input: [&[f32]; 2],
        output: &mut [&mut [f32]; 2],
        wet_out: &mut WetOut<'_>,
        n: usize,
    ) {
        let later = self.keeper.later();
        for i in 0..n {
            if self.countdown == 0 {
                self.aim_guard();
                self.countdown = CONTROL;
            }
            self.countdown -= 1;
            self.corner.g += self.corner.step;
            let g = self.corner.g;
            let drift = 1.0 - (self.semitones.step() / 12.0).exp2();
            let keep = self.keep.step();
            // Kept while it is heard at all, or about to be.
            let keeping = keep > 0.0 || self.keep.target() > 0.0;
            let (dry, wet) = equal_power(self.mix.step());
            let x = [clean(input[0][i]), clean(input[1][i])];
            let mut guarded = [0.0f32; 2];
            for ((side, sample), out) in self.sides.iter_mut().zip(x).zip(&mut guarded) {
                *out = side
                    .guard
                    .iter_mut()
                    .zip(BUTTERWORTH_8)
                    .fold(sample, |signal, (section, k)| section.lowpass(signal, g, k));
                side.plain.push(*out);
            }
            let middle = 0.5 * (guarded[0] + guarded[1]);
            self.onsets.push(middle);
            self.guide.push(self.guide_filter.process(middle));
            if self
                .taps
                .advance(drift, &self.shape, &self.guide, &self.onsets)
            {
                self.keeper.splice();
            }
            let gains = self.taps.gains();
            let mut plain = [0.0f32; 2];
            for (side, plain) in self.sides.iter().zip(&mut plain) {
                *plain = self.taps.read(&self.sinc, &side.plain, gains, later);
            }
            let unguarded = 0.5 * (x[0] + x[1]);
            let kept = self.keeper.frame(
                guarded,
                unguarded,
                &self.taps,
                gains,
                &self.sinc,
                (1.0 - drift, keeping),
            );
            for c in 0..2 {
                let shifted = (kept[c] - plain[c]).mul_add(keep, plain[c]);
                output[c][i] = guard(dry.mul_add(x[c], wet * shifted));
                wet_out.put(c, i, guard(shifted));
            }
        }
    }

    /// One block, with the wet side to `wet_out`.
    fn run(&mut self, input: [&[f32]; 2], mut output: [&mut [f32]; 2], mut wet_out: WetOut<'_>) {
        let n = if self.prepared {
            wet_out.limit(frames(&input, &output))
        } else {
            0
        };
        self.render(input, &mut output, &mut wet_out, n);
        silence_from(&mut output, n);
        wet_out.silence_from(n);
    }
}

impl Effect for Pitch {
    fn prepare(&mut self, sample_rate: f32) {
        let rate = sane_rate(sample_rate);
        self.shape = Geometry::new(rate);
        self.sinc = Sinc::new();
        self.keeper.prepare(rate, self.shape.longest);
        let room = (self.shape.longest + self.keeper.later()) as usize + self.shape.span + 32;
        for side in &mut self.sides {
            side.plain.resize(room);
        }
        self.guide.resize(room);
        self.guide_filter
            .set_lowpass(GUIDE_HZ, std::f32::consts::FRAC_1_SQRT_2, rate);
        self.onsets.prepare(rate);
        self.semitones.set_time(SHIFT_GLIDE, rate);
        self.keep.set_time(FORMANT_GLIDE, rate);
        self.mix.set_time(MIX_GLIDE, rate);
        self.prepared = true;
        self.reset();
    }

    fn reset(&mut self) {
        for side in &mut self.sides {
            side.plain.clear();
            for section in &mut side.guard {
                section.reset();
            }
        }
        self.guide.clear();
        self.guide_filter.reset();
        self.onsets.clear();
        self.keeper.clear();
        for smoother in [&mut self.semitones, &mut self.keep, &mut self.mix] {
            smoother.snap(smoother.target());
        }
        self.corner = Corner {
            g: self.guard_corner(),
            step: 0.0,
        };
        // The plain sound is read further back to stay in step with the
        // kept sound's round trip; the taps start that much nearer, so with
        // no shift the effect is the same delay at every rate.
        let start = (self.shape.start() - self.keeper.later()).max(self.shape.floor);
        self.taps.settle(start);
        self.countdown = 0;
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let Some(value) = accept(&PARAMS, index, value) else {
            return;
        };
        self.values[index] = value;
        match index {
            SHIFT | FINE => self.semitones.set(self.wanted()),
            FORMANT => self.keep.set(value),
            MIX => self.mix.set(value),
            _ => {}
        }
    }

    fn process(&mut self, _context: &Context, input: [&[f32]; 2], output: [&mut [f32]; 2]) {
        self.run(input, output, WetOut::none());
    }

    fn process_wet(
        &mut self,
        _context: &Context,
        input: [&[f32]; 2],
        output: [&mut [f32]; 2],
        wet: [&mut [f32]; 2],
    ) {
        self.run(input, output, WetOut::to(wet));
    }
}

#[cfg(test)]
mod across;
#[cfg(test)]
mod tests;
