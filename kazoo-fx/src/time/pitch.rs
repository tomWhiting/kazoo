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
//! (25 ms, and further for shifts of more than an octave up, so a splice
//! comes at most 40 times a second), slid by up to 8 ms to wherever its
//! audio best matches the heard tap's, as in Verhelst and Roelands' WSOLA
//! (the same alignment as `shimmer`'s: coarse, then to the sample), and
//! the two crossfade over 12 ms (over less for big shifts up, and sooner
//! whenever the shift speeds up mid-crossfade, so the fading tap never
//! runs out of audio while it is still heard), equal-amplitude for audio that
//! matches and equal-power for audio that does not. Between splices only
//! one tap plays, so a held note comes through as it went in, with no
//! comb from two taps a few milliseconds apart, and with no shift at all
//! the effect is a plain delay. Both channels share the taps, aligned on
//! their sum, so the stereo image holds.
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
//! harmonics move, the formants stay. Each tap's audio is reshaped through
//! a lattice of its own with exactly the envelope it was flattened with
//! (the two taps of a splice read moments 25 ms apart, and a voice's
//! envelope can change a lot in 25 ms), and the reshaped taps are then
//! crossfaded. What the envelope cannot hold (a plain sine has no
//! formants; the envelope fits the sine itself) would change the level, so
//! the kept sound is held to the loudness of the plainly shifted sound
//! (within 30 dB): the switch changes timbre, never level. The switch
//! crossfades over 30 ms, and both ways are always running, so it never
//! clicks.
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
//! two octaves up; with no shift, 21 ms. The dry path is not delayed: it is
//! the effect's sound, as a harmony a moment behind the voice, so the
//! effect reports no latency.
//!
//! Sources: W. Verhelst and M. Roelands, "An overlap-add technique based on
//! waveform similarity (WSOLA) for high quality time-scale modification of
//! speech" (ICASSP 1993); J. Makhoul, "Linear prediction: a tutorial
//! review" (Proc. IEEE, 1975), for the envelope.

use std::f32::consts::FRAC_PI_2;

use super::formant::{Formants, Lattice, MAX_ORDER, Set};
use super::parts::{
    Biquad, SINC_SHORTEST, Sinc, accept, clean, defaults, equal_power, frames, guard, silence_from,
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
/// The largest ratio there can be: two octaves and a semitone up.
const MOST_RATIO: f32 = 4.238;

/// The read delay's floor, seconds: half a millisecond, never closer than
/// the band-limited read allows.
const FLOOR: f32 = 0.000_5;
/// How far a new tap may slide to match the heard one, seconds each way,
/// and how much audio the match compares.
const SEARCH: f32 = 0.008;
const MATCH: f32 = 0.008;
/// How far a new tap starts from the heard one, seconds, for shifts of up
/// to an octave up (further beyond, so splices stay as far apart in time).
const JUMP: f32 = 0.025;
/// How long two taps crossfade, seconds.
const OVERLAP: f32 = 0.012;
/// The coarse alignment pass looks at the audio at about this rate.
const COARSE_RATE: f32 = 12_000.0;
/// The alignment hears the audio through a lowpass here: the fundamental
/// and the first few harmonics are what line up.
const GUIDE_HZ: f32 = 2_500.0;
/// The guard's corner as a share of the rate: over the ratio for a shift
/// up, and wide open for a shift down.
const RAISED_SHARE: f32 = 0.42;
const OPEN_SHARE: f32 = 0.47;
/// The damping (`1 / Q`) of the four sections of an eighth-order
/// Butterworth lowpass.
const BUTTERWORTH_8: [f32; 4] = [1.961_57, 1.662_939, 1.111_14, 0.390_181];
/// Samples between guard corners worked out; the corner glides between
/// them.
const CONTROL: usize = 32;
/// Glides, seconds: the shift, the formant switch and the mix.
const SHIFT_GLIDE: f32 = 0.03;
const FORMANT_GLIDE: f32 = 0.03;
const MIX_GLIDE: f32 = 0.02;
/// How quickly the kept sound's loudness follows the plain sound's,
/// seconds, and the most it is lifted or lowered (30 dB).
const LEVEL_TIME: f32 = 0.005;
const MOST_LIFT: f32 = 31.6;

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
        default: 1.0,
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
    description: "A clean pitch shifter in semitones, two octaves each way, that can keep \
                  a voice's formants in place so it never goes chipmunk.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Effect> {
    Box::new(Pitch::new())
}

/// The shifter's distances, in samples at the running rate.
#[derive(Debug, Clone, Copy, Default)]
struct Geometry {
    floor: f32,
    search: i32,
    span: usize,
    jump: f32,
    overlap: f32,
    stride: i32,
    /// The furthest back a tap ever reads.
    longest: f32,
}

impl Geometry {
    fn new(rate: f32) -> Self {
        let floor = (FLOOR * rate).max(SINC_SHORTEST as f32 + 2.0).round();
        let search = (SEARCH * rate).round();
        let jump = JUMP * rate;
        let overlap = OVERLAP * rate;
        Self {
            floor,
            search: search as i32,
            span: ((MATCH * rate) as usize).max(1),
            jump,
            overlap,
            stride: (rate / COARSE_RATE).round().max(1.0) as i32,
            longest: jump.mul_add(
                MOST_RATIO - 1.0,
                2.0f32.mul_add(overlap + search, floor) + 8.0,
            ),
        }
    }

    /// Where a tap starts: with no shift it stays here, a plain delay.
    fn start(&self) -> f32 {
        (0.5f32.mul_add(self.jump, self.floor) + self.search as f32).round()
    }
}

/// The two read taps: the one heard, and the one fading out after a
/// splice.
#[derive(Debug, Clone, Copy, Default)]
struct Taps {
    heard: f32,
    fading: f32,
    /// How far through the crossfade, 0 to 1; 1 when only one tap plays.
    fade: f32,
    step: f32,
    /// How alike the two taps' audio was at the splice, 0 to 1.
    alike: f32,
}

impl Taps {
    const fn settle(&mut self, at: f32) {
        self.heard = at;
        self.fading = at;
        self.fade = 1.0;
        self.step = 0.0;
        self.alike = 0.0;
    }

    /// Move both taps on by a sample at `drift` (`1 - ratio`), splicing to
    /// a new tap when the heard one reaches the end of its travel. True when
    /// a splice starts.
    fn advance(&mut self, drift: f32, shape: &Geometry, guide: &DelayLine) -> bool {
        let hold = |delay: f32| delay.clamp(shape.floor, shape.longest);
        self.heard = hold(self.heard + drift);
        if self.fade < 1.0 {
            self.fading = hold(self.fading + drift);
            // A tap held at the end of its travel stops shifting, and heard
            // like that it kinks the wave: when the shift speeds up through
            // a crossfade, the crossfade speeds up to end first.
            let room = |delay: f32| {
                if drift < 0.0 {
                    delay - shape.floor
                } else {
                    shape.longest - delay
                }
            };
            let pace = drift.abs();
            if pace > 0.0 {
                let travel = room(self.fading).min(room(self.heard)) / pace;
                let left = (1.0 - self.fade) / self.step.max(1e-9);
                if travel < left {
                    self.step = (1.0 - self.fade) / travel.max(1.0);
                }
            }
            self.fade = (self.fade + self.step).min(1.0);
            return false;
        }
        let pace = drift.abs();
        let rising = drift < 0.0 && self.heard <= shape.overlap.mul_add(pace.min(1.0), shape.floor);
        let falling = drift > 0.0 && self.heard >= shape.floor + shape.search as f32 + shape.jump;
        if rising || falling {
            let jump = shape.jump * pace.max(1.0);
            let target = if rising {
                self.heard + jump
            } else {
                self.heard - jump
            };
            let (at, alike) = self.align(target, shape, guide);
            self.fading = self.heard;
            self.heard = at;
            self.fade = 0.0;
            self.step = pace.max(1.0) / shape.overlap.max(1.0);
            self.alike = alike;
        }
        rising || falling
    }

    /// Where near `target` a new tap's audio best matches the heard tap's,
    /// and how alike the two are there.
    fn align(&self, target: f32, shape: &Geometry, guide: &DelayLine) -> (f32, f32) {
        let here = self.heard.round() as usize;
        let centre = target.round();
        let lowest = (shape.floor - centre) as i32;
        let highest = (shape.longest - shape.span as f32 - centre) as i32;
        let (low, high) = (lowest.max(-shape.search), highest.min(shape.search));
        if low > high {
            return (hold_target(target, shape), 0.0);
        }
        let score = |lag: i32, step: usize| {
            let at = (centre as i64 + i64::from(lag)).max(1) as usize;
            let (mut both, mut mine, mut theirs) = (0.0f32, 0.0f32, 0.0f32);
            let mut i = 0;
            while i < shape.span {
                let (x, y) = (guide.tap(at + i), guide.tap(here + i));
                both = x.mul_add(y, both);
                mine = x.mul_add(x, mine);
                theirs = y.mul_add(y, theirs);
                i += step;
            }
            (both, mine, theirs)
        };
        let fit = |(both, mine, _): (f32, f32, f32)| both / (mine + 1e-20).sqrt();
        let stride = shape.stride.max(1);
        let coarse_step = stride.unsigned_abs() as usize;
        let mut best = (f32::MIN, 0i32);
        let mut lag = low;
        while lag <= high {
            let value = fit(score(lag, coarse_step));
            if value > best.0 {
                best = (value, lag);
            }
            lag += stride;
        }
        let fine_step = (coarse_step / 4).max(1);
        let coarse = best.1;
        best = (f32::MIN, coarse);
        for lag in (coarse - stride).max(low)..=(coarse + stride).min(high) {
            let value = fit(score(lag, fine_step));
            if value > best.0 {
                best = (value, lag);
            }
        }
        let (both, mine, theirs) = score(best.1, fine_step);
        let energy = (mine * theirs).sqrt();
        let alike = if energy > 1e-12 && both.is_finite() {
            (both / energy).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (centre + best.1 as f32, alike)
    }

    /// The heard and the fading tap's gains: one and nothing between
    /// splices; through a splice, a quarter-sine crossfade scaled so its
    /// power holds whether the taps' audio is alike or not.
    fn gains(&self) -> (f32, f32) {
        if self.fade >= 1.0 {
            return (1.0, 0.0);
        }
        let (rise, fall) = (FRAC_PI_2 * self.fade).sin_cos();
        let scale = 1.0 / (2.0 * self.alike * rise).mul_add(fall, 1.0).sqrt();
        (rise * scale, fall * scale)
    }

    /// `line` through both taps.
    fn read(&self, sinc: &Sinc, line: &DelayLine, (heard, fading): (f32, f32)) -> f32 {
        let now = heard * sinc.read(line, f64::from(self.heard));
        if fading > 0.0 {
            fading.mul_add(sinc.read(line, f64::from(self.fading)), now)
        } else {
            now
        }
    }
}

/// `target` held where a tap can read.
const fn hold_target(target: f32, shape: &Geometry) -> f32 {
    target.clamp(shape.floor, shape.longest)
}

/// A lowpass section of Zavalishin's topology-preserving state-variable
/// filter: the bilinear transform of the analogue state-variable filter,
/// built so that its corner can move every sample without a click.
#[derive(Debug, Clone, Copy, Default)]
struct Svf {
    first: f32,
    second: f32,
}

impl Svf {
    /// One sample through at prewarped corner `g` (`tan(π fc / rate)`) and
    /// damping `k` (`1 / Q`).
    fn lowpass(&mut self, input: f32, g: f32, k: f32) -> f32 {
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

    const fn reset(&mut self) {
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

/// One channel's lines and filters.
#[derive(Debug, Clone, Default)]
struct Side {
    guard: [Svf; 4],
    /// The guarded input, and its envelope flattened out.
    plain: DelayLine,
    flat: DelayLine,
    flatten: Lattice,
    /// The envelope put back on, one lattice for each tap: each tap's
    /// audio is reshaped with the envelope it was flattened with.
    shapes: [Lattice; 2],
}

/// How loud the plainly shifted and the kept sound are, and the gain that
/// matches the second to the first. Each loudness is a power through two
/// one-poles in a row: as quick to follow a change as one twice as slow,
/// with a tenth of its ripple on a low note, so the gain does not wobble
/// with the wave and add harmonics of its own.
#[derive(Debug, Clone, Copy)]
struct Level {
    plain: [f32; 2],
    kept: [f32; 2],
    coeff: f32,
    lift: Smoothed,
}

impl Default for Level {
    fn default() -> Self {
        Self {
            plain: [0.0; 2],
            kept: [0.0; 2],
            coeff: 1.0,
            lift: Smoothed::new(1.0),
        }
    }
}

/// `power` through the two one-poles `stages`; the second's output.
fn smoothed_power(stages: &mut [f32; 2], power: f32, coeff: f32) -> f32 {
    stages[0] = (power - stages[0]).mul_add(coeff, stages[0]);
    stages[1] = (stages[0] - stages[1]).mul_add(coeff, stages[1]);
    for stage in stages.iter_mut() {
        flush(stage);
    }
    stages[1]
}

impl Level {
    fn prepare(&mut self, rate: f32) {
        self.coeff = -(-1.0 / (LEVEL_TIME * rate)).exp_m1();
        self.lift.set_time(LEVEL_TIME, rate);
        self.clear();
    }

    const fn clear(&mut self) {
        self.plain = [0.0; 2];
        self.kept = [0.0; 2];
        self.lift.snap(1.0);
    }

    /// The gain for the kept sound, given a frame of each.
    fn follow(&mut self, plain: [f32; 2], kept: [f32; 2]) -> f32 {
        let power = |frame: [f32; 2]| frame[0].mul_add(frame[0], frame[1] * frame[1]);
        let plain = smoothed_power(&mut self.plain, power(plain), self.coeff);
        let kept = smoothed_power(&mut self.kept, power(kept), self.coeff);
        let wanted = ((plain + 1e-12) / (kept + 1e-12)).sqrt();
        self.lift.set(wanted.clamp(1.0 / MOST_LIFT, MOST_LIFT));
        self.lift.step()
    }
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
    formants: Formants,
    level: Level,
    /// The envelope now, and the ones each tap's audio was flattened with.
    now: Set,
    heard_set: Set,
    fading_set: Set,
    /// Which of each side's `shapes` the heard tap uses.
    lead: usize,
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
            formants: Formants::default(),
            level: Level::default(),
            now: [0.0; MAX_ORDER],
            heard_set: [0.0; MAX_ORDER],
            fading_set: [0.0; MAX_ORDER],
            lead: 0,
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
        let order = self.formants.order();
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
            let (dry, wet) = equal_power(self.mix.step());
            let x = [clean(input[0][i]), clean(input[1][i])];
            self.formants.push(0.5 * (x[0] + x[1]));
            self.formants.at(0.0, &mut self.now);
            let mut mid = 0.0;
            for (side, sample) in self.sides.iter_mut().zip(x) {
                let guarded = side
                    .guard
                    .iter_mut()
                    .zip(BUTTERWORTH_8)
                    .fold(sample, |signal, (section, k)| section.lowpass(signal, g, k));
                side.plain.push(guarded);
                side.flat
                    .push(side.flatten.flatten(guarded, &self.now, order));
                mid = 0.5f32.mul_add(guarded, mid);
            }
            self.guide.push(self.guide_filter.process(mid));
            if self.taps.advance(drift, &self.shape, &self.guide) {
                // The new tap takes the other lattice, from where the heard
                // one's is: the fading tap keeps its own.
                self.lead = 1 - self.lead;
                for side in &mut self.sides {
                    side.shapes[self.lead] = side.shapes[1 - self.lead];
                }
            }
            let gains = self.taps.gains();
            let (heard, fading) = (f64::from(self.taps.heard), f64::from(self.taps.fading));
            self.formants.at(self.taps.heard, &mut self.heard_set);
            if gains.1 > 0.0 {
                self.formants.at(self.taps.fading, &mut self.fading_set);
            }
            let lead = self.lead;
            let mut plain = [0.0f32; 2];
            let mut kept = [0.0f32; 2];
            for ((side, plain), kept) in self.sides.iter_mut().zip(&mut plain).zip(&mut kept) {
                *plain = self.taps.read(&self.sinc, &side.plain, gains);
                let excitation = self.sinc.read(&side.flat, heard);
                *kept = gains.0 * side.shapes[lead].shape(excitation, &self.heard_set, order);
                if gains.1 > 0.0 {
                    let excitation = self.sinc.read(&side.flat, fading);
                    let shaped = side.shapes[1 - lead].shape(excitation, &self.fading_set, order);
                    *kept = gains.1.mul_add(shaped, *kept);
                }
            }
            let lift = self.level.follow(plain, kept);
            for c in 0..2 {
                let shifted = kept[c].mul_add(lift, -plain[c]).mul_add(keep, plain[c]);
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
        let room = self.shape.longest as usize + self.shape.span + 32;
        for side in &mut self.sides {
            side.plain.resize(room);
            side.flat.resize(room);
        }
        self.guide.resize(room);
        self.guide_filter
            .set_lowpass(GUIDE_HZ, std::f32::consts::FRAC_1_SQRT_2, rate);
        self.formants.prepare(rate, self.shape.longest);
        self.semitones.set_time(SHIFT_GLIDE, rate);
        self.keep.set_time(FORMANT_GLIDE, rate);
        self.mix.set_time(MIX_GLIDE, rate);
        self.level.prepare(rate);
        self.prepared = true;
        self.reset();
    }

    fn reset(&mut self) {
        for side in &mut self.sides {
            side.plain.clear();
            side.flat.clear();
            side.flatten.clear();
            for lattice in &mut side.shapes {
                lattice.clear();
            }
            for section in &mut side.guard {
                section.reset();
            }
        }
        self.guide.clear();
        self.guide_filter.reset();
        self.formants.clear();
        self.level.clear();
        for smoother in [&mut self.semitones, &mut self.keep, &mut self.mix] {
            smoother.snap(smoother.target());
        }
        self.corner = Corner {
            g: self.guard_corner(),
            step: 0.0,
        };
        self.taps.settle(self.shape.start());
        self.now = [0.0; MAX_ORDER];
        self.heard_set = [0.0; MAX_ORDER];
        self.fading_set = [0.0; MAX_ORDER];
        self.lead = 0;
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
mod tests;
