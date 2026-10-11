//! A voice's spectral envelope, for the pitch shifter's formant keeping.
//!
//! The envelope is found by linear prediction (LPC), the way speech coders
//! have found it since the 1970s: every few milliseconds a short window of
//! the input is analysed, and the Levinson–Durbin recursion turns its
//! autocorrelation into the reflection coefficients of an all-pole filter
//! whose peaks sit on the formants. A [`Lattice`] run forwards with those
//! coefficients flattens the envelope out of a signal (leaving the
//! excitation: the harmonics, all at about one level); run backwards it
//! puts the envelope back on. Reflection coefficients are used rather than
//! the direct-form ones because a lattice stays stable for any
//! coefficients under one in size, however they move, and because a
//! straight line between two stable sets is itself stable, so the sets can
//! glide from one analysis to the next without a step.
//!
//! - The analysis hears the input through a gentle pre-emphasis (a first
//!   difference at 0.9), so the envelope fits the formants rather than the
//!   voice's falling slope, and through a Hann window 20 ms long, two
//!   periods of a low voice.
//! - The autocorrelation is tapered by a Gaussian lag window (150 Hz wide)
//!   and its zero lag raised by a hundredth (a noise floor 20 dB down):
//!   the classic guards that keep the envelope from locking onto single
//!   harmonics and keep every pole well inside the unit circle. The floor
//!   is high for speech coding on purpose: a pure tone's envelope is the
//!   tone itself, and the floor holds how far flattening can take a tone
//!   down to 20 dB.
//! - The order follows the analysis rate (one pole pair for every 2 kHz of
//!   band: 22 at 44.1 kHz, 24 at 48 kHz, at most 32). The pitch shifter
//!   analyses at 40 to 80 kHz whatever the host's rate (see `pitch::keep`),
//!   which is what gives the envelope the same detail at every rate.
//! - Every set is kept for as long as the shifter can hold audio back, so
//!   the envelope put back on the shifted sound is the one its audio was
//!   analysed with: [`Formants::at`] reads the sets at any age, gliding
//!   between them.
//! - A set stands for the window that ended a hop before it is used: the
//!   envelope given for any moment of the audio is centred about 14 ms
//!   (half the window and a hop) earlier, so it follows a change of vowel
//!   that late. Centring it would mean reading 14 ms ahead, every shifted
//!   sound 14 ms later; the lag only shows as formants gliding through a
//!   quick change of vowel.
//!
//! Sources: J. Makhoul, "Linear prediction: a tutorial review" (Proc. IEEE,
//! 1975); J. D. Markel and A. H. Gray, "Linear prediction of speech"
//! (Springer, 1976), for the lattice forms and the lag window.

use crate::dsp::flush;

/// The most reflection coefficients a set holds.
pub const MAX_ORDER: usize = 32;
/// The analysis window and the time between analyses, seconds.
const WINDOW: f32 = 0.02;
const HOP: f32 = 0.004;
/// The pre-emphasis the analysis hears through.
const EMPHASIS: f32 = 0.9;
/// The lag window's width, hertz, and the noise floor added to the zero
/// lag, as a share of it.
const LAG_HZ: f64 = 150.0;
const NOISE_FLOOR: f64 = 1e-2;
/// The largest reflection coefficient allowed: the sharpest formant is
/// still a resonance that dies away.
const MOST_REFLECTION: f64 = 0.995;
/// The hertz of band each pole pair covers.
const BAND_PER_PAIR: f32 = 2_000.0;
/// A window quieter than this, in summed squares, holds no envelope.
const QUIET: f64 = 1e-12;

/// One set of reflection coefficients; the first `order` are used.
pub type Set = [f32; MAX_ORDER];

/// The running envelope analysis.
#[derive(Debug, Clone)]
pub struct Formants {
    order: usize,
    hop: usize,
    /// Samples since the newest set was analysed.
    since: usize,
    /// The last input, for the pre-emphasis.
    last: f32,
    /// The most recent window's worth of pre-emphasised input, a ring.
    recent: Vec<f32>,
    write: usize,
    window: Vec<f32>,
    /// The windowed copy the autocorrelation reads.
    scratch: Vec<f32>,
    lags: [f64; MAX_ORDER + 1],
    /// Every set still needed, a ring; `newest` is the latest.
    sets: Vec<Set>,
    newest: usize,
}

impl Default for Formants {
    /// Unprepared: no sets, every answer flat.
    fn default() -> Self {
        Self {
            order: 0,
            hop: 0,
            since: 0,
            last: 0.0,
            recent: Vec::new(),
            write: 0,
            window: Vec::new(),
            scratch: Vec::new(),
            lags: [0.0; MAX_ORDER + 1],
            sets: Vec::new(),
            newest: 0,
        }
    }
}

impl Formants {
    /// Size everything for `rate`, keeping sets for `oldest` samples of
    /// age. Allocates.
    pub fn prepare(&mut self, rate: f32, oldest: f32) {
        self.order = ((rate / (2.0 * BAND_PER_PAIR)).round() as usize * 2).clamp(12, MAX_ORDER);
        self.hop = ((HOP * rate).round() as usize).max(16);
        let length = ((WINDOW * rate).round() as usize).max(64);
        self.window = (0..length)
            .map(|n| {
                let phase = std::f32::consts::TAU * (n as f32 + 0.5) / length as f32;
                (-0.5f32).mul_add(phase.cos(), 0.5)
            })
            .collect();
        self.recent = vec![0.0; length];
        self.scratch = vec![0.0; length];
        let spread = std::f64::consts::TAU * LAG_HZ / f64::from(rate);
        for (k, lag) in self.lags.iter_mut().enumerate() {
            let x = spread * k as f64;
            *lag = (-0.5 * x * x).exp();
        }
        self.lags[0] = 1.0 + NOISE_FLOOR;
        let hops = (oldest.max(0.0) as usize).div_ceil(self.hop) + 3;
        self.sets = vec![[0.0; MAX_ORDER]; hops];
        self.clear();
    }

    /// Forget the input: every set flat.
    pub fn clear(&mut self) {
        self.recent.fill(0.0);
        for set in &mut self.sets {
            set.fill(0.0);
        }
        self.write = 0;
        self.newest = 0;
        self.since = 0;
        self.last = 0.0;
    }

    /// How many coefficients a set uses at this rate.
    pub const fn order(&self) -> usize {
        self.order
    }

    /// Hear one sample of the input; every hop, analyse the window.
    pub fn push(&mut self, sample: f32) {
        if self.recent.is_empty() {
            return;
        }
        let emphasised = EMPHASIS.mul_add(-self.last, sample);
        self.last = sample;
        self.recent[self.write] = emphasised;
        self.write = (self.write + 1) % self.recent.len();
        self.since += 1;
        if self.since >= self.hop {
            self.since = 0;
            self.analyse();
        }
    }

    /// The set for audio `age` samples old (0 is now), into `into` (past
    /// the order, zeros). Each
    /// set stands one hop after its analysis, and the sets glide into one
    /// another in a straight line, so the answer moves smoothly with time
    /// and with age; past the oldest set kept, the oldest is used.
    pub fn at(&self, age: f32, into: &mut Set) {
        let count = self.sets.len();
        if count < 2 || self.hop == 0 {
            into.fill(0.0);
            return;
        }
        let hop = self.hop as f32;
        let back = ((hop - self.since as f32 + age.max(0.0)) / hop).min((count - 2) as f32);
        let whole = back.floor();
        let blend = back - whole;
        let later = (self.newest + count - whole as usize) % count;
        let earlier = (later + count - 1) % count;
        let (later, earlier) = (&self.sets[later], &self.sets[earlier]);
        for ((slot, late), early) in into.iter_mut().zip(later).zip(earlier).take(self.order) {
            *slot = blend.mul_add(early - late, *late);
        }
        if let Some(unused) = into.get_mut(self.order..) {
            unused.fill(0.0);
        }
    }

    /// A new set from the window just heard.
    fn analyse(&mut self) {
        let length = self.recent.len();
        for (n, (slot, weight)) in self.scratch.iter_mut().zip(&self.window).enumerate() {
            *slot = self.recent[(self.write + n) % length] * weight;
        }
        let mut correlation = [0.0f64; MAX_ORDER + 1];
        for (lag, value) in correlation.iter_mut().enumerate().take(self.order + 1) {
            let mut sum = 0.0f64;
            for (a, b) in self.scratch.iter().zip(&self.scratch[lag..]) {
                sum = f64::from(*a).mul_add(f64::from(*b), sum);
            }
            *value = sum * self.lags[lag];
        }
        let count = self.sets.len();
        self.newest = (self.newest + 1) % count;
        reflections(&correlation, self.order, &mut self.sets[self.newest]);
    }
}

/// The reflection coefficients for the autocorrelation `r`, by the
/// Levinson–Durbin recursion, into `out`. Flat (all zero) for silence or
/// anything not finite.
fn reflections(r: &[f64; MAX_ORDER + 1], order: usize, out: &mut Set) {
    out.fill(0.0);
    let mut error = r[0];
    if !(error.is_finite() && error > QUIET) {
        return;
    }
    let mut a = [0.0f64; MAX_ORDER + 1];
    a[0] = 1.0;
    for m in 1..=order.min(MAX_ORDER) {
        let mut acc = r[m];
        for i in 1..m {
            acc = a[i].mul_add(r[m - i], acc);
        }
        let k = (-acc / error).clamp(-MOST_REFLECTION, MOST_REFLECTION);
        if !k.is_finite() {
            out.fill(0.0);
            return;
        }
        let previous = a;
        for i in 1..m {
            a[i] = k.mul_add(previous[m - i], previous[i]);
        }
        a[m] = k;
        error *= k.mul_add(-k, 1.0);
        out[m - 1] = k as f32;
        if error <= QUIET * r[0] {
            return;
        }
    }
}

/// A lattice filter's memory: one delayed backward value per stage.
#[derive(Debug, Clone, Copy)]
pub struct Lattice {
    state: [f32; MAX_ORDER],
}

impl Default for Lattice {
    fn default() -> Self {
        Self {
            state: [0.0; MAX_ORDER],
        }
    }
}

impl Lattice {
    /// Forget the past.
    pub const fn clear(&mut self) {
        self.state = [0.0; MAX_ORDER];
    }

    /// The envelope `set` flattened out of `x`: the prediction error, through
    /// the all-zero lattice.
    pub fn flatten(&mut self, x: f32, set: &Set, order: usize) -> f32 {
        let mut forward = x;
        let mut backward = x;
        for (k, held) in set.iter().zip(self.state.iter_mut()).take(order) {
            let delayed = *held;
            *held = backward;
            flush(held);
            backward = k.mul_add(forward, delayed);
            forward = k.mul_add(delayed, forward);
        }
        self.settle(forward)
    }

    /// The envelope `set` put onto `excitation`, through the all-pole
    /// lattice.
    pub fn shape(&mut self, excitation: f32, set: &Set, order: usize) -> f32 {
        let order = order.min(MAX_ORDER);
        let mut forward = excitation;
        for m in (0..order).rev() {
            forward = (-set[m]).mul_add(self.state[m], forward);
            if m + 1 < order {
                self.state[m + 1] = set[m].mul_add(forward, self.state[m]);
                flush(&mut self.state[m + 1]);
            }
        }
        if order > 0 {
            self.state[0] = forward;
            flush(&mut self.state[0]);
        }
        self.settle(forward)
    }

    /// `out`, or silence and a cleared memory if it is not a number.
    const fn settle(&mut self, out: f32) -> f32 {
        if out.is_finite() {
            out
        } else {
            self.clear();
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;

    /// A two-pole resonance at `hz` driven by a pulse train at `pitch`.
    fn vowel(len: usize, pitch: f32, hz: f32) -> Vec<f32> {
        let radius = 0.97f32;
        let angle = std::f32::consts::TAU * hz / RATE;
        let (a1, a2) = (-2.0 * radius * angle.cos(), radius * radius);
        let period = (RATE / pitch) as usize;
        let (mut y1, mut y2) = (0.0f32, 0.0f32);
        (0..len)
            .map(|n| {
                let pulse = if n % period == 0 { 1.0 } else { 0.0 };
                let y = (-a1).mul_add(y1, (-a2).mul_add(y2, pulse));
                y2 = y1;
                y1 = y;
                y * 0.05
            })
            .collect()
    }

    fn analysed(signal: &[f32]) -> Formants {
        let mut formants = Formants::default();
        formants.prepare(RATE, 4_800.0);
        for &sample in signal {
            formants.push(sample);
        }
        formants
    }

    #[test]
    fn flatten_then_shape_gives_the_signal_back() {
        let signal = vowel(9_600, 140.0, 700.0);
        let formants = analysed(&signal);
        let mut set = [0.0; MAX_ORDER];
        formants.at(0.0, &mut set);
        assert!(set.iter().any(|k| k.abs() > 0.1), "{set:?}");
        let (mut forward, mut back) = (Lattice::default(), Lattice::default());
        for &x in &signal {
            let flat = forward.flatten(x, &set, formants.order());
            let shaped = back.shape(flat, &set, formants.order());
            assert!((shaped - x).abs() < 1e-4, "{shaped} {x}");
        }
    }

    /// The level of `hz` in `signal`, by correlation.
    fn level(signal: &[f32], hz: f32) -> f32 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (n, sample) in signal.iter().enumerate() {
            let angle = std::f64::consts::TAU * f64::from(hz) * n as f64 / f64::from(RATE);
            re = f64::from(*sample).mul_add(angle.cos(), re);
            im = f64::from(*sample).mul_add(angle.sin(), im);
        }
        re.hypot(im) as f32
    }

    /// How far apart the loudest and quietest of the first sixteen
    /// harmonics of `f0` in `signal` are, as a ratio.
    fn spread(signal: &[f32], f0: f32) -> f32 {
        let levels: Vec<f32> = (1..=16).map(|k| level(signal, k as f32 * f0)).collect();
        let loudest = levels.iter().fold(0.0f32, |a, b| a.max(*b));
        let quietest = levels.iter().fold(f32::MAX, |a, b| a.min(*b));
        loudest / quietest.max(1e-12)
    }

    #[test]
    fn flattening_takes_the_formant_out() {
        // The resonance lifts the harmonics near 700 Hz far above the
        // rest; flattened, they all sit close to one level.
        let signal = vowel(19_200, 140.0, 700.0);
        let formants = analysed(&signal);
        let mut set = [0.0; MAX_ORDER];
        formants.at(0.0, &mut set);
        let mut lattice = Lattice::default();
        let flat: Vec<f32> = signal
            .iter()
            .map(|&x| lattice.flatten(x, &set, formants.order()))
            .collect();
        let f0 = RATE / (RATE / 140.0).floor();
        let (before, after) = (spread(&signal[9_600..], f0), spread(&flat[9_600..], f0));
        assert!(before > 8.0 && after < 4.0, "{before} to {after}");
    }

    #[test]
    fn silence_and_nonsense_leave_every_set_flat_and_the_lattices_clean() {
        let mut formants = Formants::default();
        formants.prepare(RATE, 4_800.0);
        for n in 0..4_800 {
            formants.push(if n % 7 == 0 { f32::NAN } else { 0.0 });
        }
        let mut set = [1.0; MAX_ORDER];
        formants.at(100.0, &mut set);
        assert!(set.iter().all(|k| *k == 0.0), "{set:?}");
        let wild = [0.999; MAX_ORDER];
        let mut lattice = Lattice::default();
        for x in [f32::NAN, f32::INFINITY, 1e30, -1e30, 0.5] {
            assert!(lattice.shape(x, &wild, MAX_ORDER).is_finite());
            assert!(lattice.flatten(x, &wild, MAX_ORDER).is_finite());
        }
    }

    #[test]
    fn every_reflection_stays_inside_the_unit_circle() {
        let mut noise = crate::dsp::Noise::new(9);
        let signal: Vec<f32> = (0..24_000)
            .map(|n| (n as f32 * 0.07).sin().mul_add(0.9, 0.1 * noise.sample()))
            .collect();
        let formants = analysed(&signal);
        for age in [0.0, 150.0, 1_000.0, 4_000.0, 1e9] {
            let mut set = [0.0; MAX_ORDER];
            formants.at(age, &mut set);
            assert!(set.iter().all(|k| k.abs() <= 0.995), "{age} {set:?}");
        }
    }
}
