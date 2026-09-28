//! Checks every drive effect must pass, and the test signals and
//! measurements the effects' own tests share.

use std::f64::consts::PI;
use std::sync::{Arc, Mutex, PoisonError};

use crate::{Context, Effect, EffectKind};

/// The rate every test runs at.
pub const RATE: f32 = 48_000.0;

/// The tempo every test runs at.
pub const CONTEXT: Context = Context { bpm: 120.0 };

/// Samples in one aliasing measurement.
const FFT_SIZE: usize = 8_192;

/// Run `left` and `right` through `effect` in blocks of awkward, changing
/// sizes, as a host might.
pub fn render(effect: &mut dyn Effect, left: &[f32], right: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let len = left.len().min(right.len());
    let mut out_left = vec![0.0; len];
    let mut out_right = vec![0.0; len];
    let sizes = [1, 7, 64, 333, 512, 0, 129];
    let mut start = 0;
    let mut turn = 0;
    while start < len {
        let end = (start + sizes[turn % sizes.len()]).min(len);
        turn += 1;
        effect.process(
            &CONTEXT,
            [&left[start..end], &right[start..end]],
            [&mut out_left[start..end], &mut out_right[start..end]],
        );
        start = end;
    }
    (out_left, out_right)
}

/// Run a mono signal through `effect`, returning the left output.
pub fn render_mono(effect: &mut dyn Effect, input: &[f32]) -> Vec<f32> {
    render(effect, input, input).0
}

/// A prepared effect of `kind`.
pub fn built(kind: &EffectKind) -> Box<dyn Effect> {
    let mut effect = (kind.build)();
    effect.prepare(RATE);
    effect
}

/// A sine at `hz` and peak `amplitude`, `seconds` long.
pub fn sine(hz: f64, amplitude: f32, seconds: f64) -> Vec<f32> {
    let n = (seconds * f64::from(RATE)) as usize;
    (0..n)
        .map(|i| amplitude * (2.0 * PI * hz * i as f64 / f64::from(RATE)).sin() as f32)
        .collect()
}

/// Something like a guitar: plucked notes and a chord, each a stack of
/// harmonics that die away faster the higher they are, peaking at `peak`.
pub fn guitar(seconds: f64, peak: f32) -> Vec<f32> {
    guitar_at(RATE, seconds, peak)
}

/// [`guitar`] at sample rate `rate`.
pub fn guitar_at(rate: f32, seconds: f64, peak: f32) -> Vec<f32> {
    let (out, top) = &*made("guitar", rate, seconds, || {
        let n = (seconds * f64::from(rate)) as usize;
        let strum = (0.005 * f64::from(rate)) as usize;
        let notes = [82.41, 110.0, 146.83, 196.0, 246.94, 329.63];
        let mut out = vec![0.0f64; n];
        let spacing = (n / 8).max(1);
        for (slot, start) in (0..n).step_by(spacing).enumerate() {
            let chord = slot % 3 == 2;
            let played: &[f64] = if chord {
                &notes
            } else {
                &notes[slot % notes.len()..=slot % notes.len()]
            };
            for (string, hz) in played.iter().enumerate() {
                let offset = start + string * strum;
                for (i, sample) in out.iter_mut().enumerate().skip(offset) {
                    let t = (i - offset) as f64 / f64::from(rate);
                    for harmonic in 1..=12 {
                        let h = f64::from(harmonic);
                        let decay = (-t * 0.8f64.mul_add(h, 1.5)).exp();
                        *sample += decay * (2.0 * PI * hz * h).mul_add(t, h).sin() / h;
                    }
                }
            }
        }
        out
    });
    out.iter()
        .map(|s| (s / top * f64::from(peak)) as f32)
        .collect()
}

/// Something like a voice, to weigh the mids: a 150 Hz harmonic stack
/// shaped by three formants (700, 1,200 and 2,600 Hz), peaking at `peak`,
/// at sample rate `rate`.
pub fn voice_at(rate: f32, seconds: f64, peak: f32) -> Vec<f32> {
    let (out, top) = &*made("voice", rate, seconds, || {
        let n = (seconds * f64::from(rate)) as usize;
        let formants = [(700.0, 130.0), (1_200.0, 150.0), (2_600.0, 250.0)];
        (0..n)
            .map(|i| {
                let t = i as f64 / f64::from(rate);
                (1..=40)
                    .map(|k| {
                        let hz = 150.0 * f64::from(k);
                        let shape: f64 = formants
                            .iter()
                            .map(|(centre, width)| (-((hz - centre) / width).powi(2)).exp())
                            .sum();
                        (0.05 + shape) / f64::from(k).sqrt()
                            * (2.0 * PI * hz).mul_add(t, f64::from(k)).sin()
                    })
                    .sum()
            })
            .collect()
    });
    let scale = f64::from(peak) / top;
    out.iter().map(|s| (s * scale) as f32).collect()
}

/// A test signal as made, before it is scaled to a peak, and its own peak.
type Made = Arc<(Vec<f64>, f64)>;

/// Which test signal: its name, rate and length (as bits).
type Recipe = (&'static str, u32, u64);

/// The test signals made so far: a guitar at 192 kHz takes as long to make
/// as most effects take to play it, and every check across rates asks for
/// the same few.
static MADE: Mutex<Vec<(Recipe, Made)>> = Mutex::new(Vec::new());

/// The signal `name` at `rate`, `seconds` long, from [`MADE`], or from
/// `make` (then kept) when it is not there yet. Tests that ask for the
/// same one at once may both make it; the samples are the same.
fn made(name: &'static str, rate: f32, seconds: f64, make: impl FnOnce() -> Vec<f64>) -> Made {
    let recipe = (name, rate.to_bits(), seconds.to_bits());
    let kept = MADE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .find(|(at, _)| *at == recipe)
        .map(|(_, signal)| Arc::clone(signal));
    kept.unwrap_or_else(|| {
        let out = make();
        let top = out.iter().fold(0.0f64, |m, s| m.max(s.abs())).max(1e-9);
        let signal = Arc::new((out, top));
        MADE.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((recipe, Arc::clone(&signal)));
        signal
    })
}

/// Root mean square.
pub fn rms(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|s| f64::from(*s).powi(2)).sum();
    (sum / samples.len() as f64).sqrt()
}

/// The largest magnitude.
pub fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |m, s| m.max(s.abs()))
}

/// Decibels between two levels.
pub fn db(ratio: f64) -> f64 {
    20.0 * ratio.max(1e-12).log10()
}

/// In-place radix-2 FFT of `re` and `im` (length a power of two).
pub fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let angle = -2.0 * PI / len as f64;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (sin, cos) = (angle * k as f64).sin_cos();
                let a = start + k;
                let b = a + len / 2;
                let tr = re[b].mul_add(cos, -im[b] * sin);
                let ti = re[b].mul_add(sin, im[b] * cos);
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        len <<= 1;
    }
}

/// The power spectrum of the last [`FFT_SIZE`] samples, Hann windowed.
pub fn spectrum(samples: &[f32]) -> Vec<f64> {
    spectrum_of(samples, FFT_SIZE)
}

/// The power spectrum of the last `size` samples (a power of two), Hann
/// windowed.
fn spectrum_of(samples: &[f32], size: usize) -> Vec<f64> {
    let tail = &samples[samples.len() - size..];
    let mut re: Vec<f64> = tail
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let window = 0.5f64.mul_add(-(2.0 * PI * i as f64 / size as f64).cos(), 0.5);
            f64::from(*s) * window
        })
        .collect();
    let mut im = vec![0.0; size];
    fft(&mut re, &mut im);
    re.iter()
        .zip(&im)
        .take(size / 2)
        .map(|(r, i)| r.mul_add(*r, i * i))
        .collect()
}

/// The power in the spectrum near `hz`.
pub fn power_near(spectrum: &[f64], hz: f64) -> f64 {
    let bin = (hz * FFT_SIZE as f64 / f64::from(RATE)).round() as usize;
    let from = bin.saturating_sub(3);
    let to = (bin + 4).min(spectrum.len());
    spectrum[from..to].iter().sum()
}

/// How much of the output of a sine near `hz` through `effect` is
/// audible aliasing: the power below 20 kHz away from every true harmonic,
/// as decibels below the output's power below 20 kHz. (What lands between
/// 20 kHz and Nyquist is left out: a halfband decimator is -6 dB at
/// Nyquist by design, and nobody hears it.)
pub fn aliasing_db(effect: &mut dyn Effect, hz: f64, amplitude: f32) -> f64 {
    // An odd bin: its harmonics can then never fold back onto one another.
    let bin = (hz * FFT_SIZE as f64 / f64::from(RATE)).round() as usize | 1;
    let exact = bin as f64 * f64::from(RATE) / FFT_SIZE as f64;
    let input = sine(exact, amplitude, 0.5);
    let out = render_mono(effect, &input);
    let mut power = spectrum(&out);
    let audible = (20_000.0 * FFT_SIZE as f64 / f64::from(RATE)) as usize;
    power.truncate(audible);
    let mut harmonic = vec![false; power.len()];
    for h in (bin..power.len()).step_by(bin) {
        let from = h.saturating_sub(3);
        let to = (h + 4).min(power.len());
        harmonic[from..to].fill(true);
    }
    let total: f64 = power.iter().skip(4).sum();
    let alias: f64 = power
        .iter()
        .zip(&harmonic)
        .skip(4)
        .filter(|(_, is)| !**is)
        .map(|(p, _)| p)
        .sum();
    10.0 * (alias.max(1e-30) / total.max(1e-30)).log10()
}

fn assert_sane(samples: &[f32], what: &str) {
    assert!(
        samples.iter().all(|s| s.is_finite()),
        "{what}: non-finite output"
    );
    let top = peak(samples);
    assert!(top <= 2.0 + 1e-6, "{what}: peak {top} beyond +6 dBFS");
}

/// What an effect must meet at every host rate.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// The most any one spur that is not a harmonic of a -6 dBFS sine near
    /// 5 kHz may reach at default settings, in dB below the fundamental,
    /// anywhere below 20 kHz (sub-harmonics included); `None` for an effect
    /// whose output is meant to be inharmonic (a crusher, a ring
    /// modulator).
    pub worst_spur_db: Option<f64>,
    /// The knobs pushed hard for the second spur check, as
    /// `(index, value)`, once for every mode that must be held to it.
    pub hot: &'static [&'static [(usize, f32)]],
    /// The spur limit with each of [`Self::hot`] applied. The limits are the
    /// family's targets, not records of what was measured: at default
    /// settings -80 dBc for the overdrives, amp and compressor and -70 dBc
    /// for the fuzz and folder; pushed hard, -48 dBc for all.
    pub hot_spur_db: Option<f64>,
    /// Whether a guitar and a voice, each peaking at -12 dBFS, must come
    /// out within 2 dB of the level they went in at, at default settings,
    /// at every rate: the guitar weighs the bass, the voice the mids.
    pub unity: bool,
    /// Knobs whose way from one end to the other passes through settings
    /// rougher than either end (a crusher's bit depth), so the click check
    /// cannot judge their glide by its ends.
    pub rough_glides: &'static [usize],
    /// Knobs set for the latency check, as `(index, value)`:
    /// for an effect whose own oscillator runs inside the oversampled
    /// domain (a ring modulator's carrier), whose phase against the plain
    /// build's says nothing about the signal's delay, the settings that
    /// take the oscillator out and leave the path; for a crusher, enough
    /// bits that the check's quiet tone is still a tone.
    pub latency_knobs: &'static [(usize, f32)],
}

/// The host rates the contract is checked at.
const RATES: [f32; 4] = [44_100.0, 48_000.0, 96_000.0, 192_000.0];

/// Everything the trait promises, checked on one kind, plus how it holds
/// up across host rates, knob jumps and a change of rate mid-stream: one
/// test for each check (the spurs one for each rate, the clicks one for
/// each setting of the other knobs), so they run in parallel, in a module
/// `conformance` where the macro is called. `$kind` is the effect's kind
/// and `$limits` its [`Limits`], both named as the calling module names
/// them.
macro_rules! conformance {
    ($kind:expr, $limits:expr $(,)?) => {
        mod conformance {
            use super::*;
            use $crate::drive::testkit as kit;

            const LIMITS: kit::Limits = $limits;

            #[test]
            fn levels_agree_across_rates() {
                kit::levels_agree_across_rates(&$kind, LIMITS);
            }

            #[test]
            fn spurs_are_held_down_at_44k1() {
                kit::spurs_are_held_down(&$kind, LIMITS, 44_100.0);
            }

            #[test]
            fn spurs_are_held_down_at_48k() {
                kit::spurs_are_held_down(&$kind, LIMITS, 48_000.0);
            }

            #[test]
            fn spurs_are_held_down_at_96k() {
                kit::spurs_are_held_down(&$kind, LIMITS, 96_000.0);
            }

            #[test]
            fn spurs_are_held_down_at_192k() {
                kit::spurs_are_held_down(&$kind, LIMITS, 192_000.0);
            }

            #[test]
            fn poison_is_refused() {
                kit::poison_is_refused(&$kind);
            }

            #[test]
            fn silence_stays_silent() {
                kit::silence_stays_silent(&$kind);
            }

            #[test]
            fn every_param_extreme_is_safe() {
                kit::every_param_extreme_is_safe(&$kind);
            }

            #[test]
            fn nonsense_params_are_ignored() {
                kit::nonsense_params_are_ignored(&$kind);
            }

            #[test]
            fn odd_blocks_are_handled() {
                kit::odd_blocks_are_handled(&$kind);
            }

            #[test]
            fn a_guitar_comes_through() {
                kit::a_guitar_comes_through(&$kind);
            }

            #[test]
            fn a_new_rate_mid_stream_is_taken() {
                kit::a_new_rate_mid_stream_is_taken(&$kind);
            }

            #[test]
            fn latency_is_declared() {
                kit::latency_is_declared(&$kind, LIMITS.latency_knobs);
            }

            #[test]
            fn knob_jumps_do_not_click_with_the_others_at_default() {
                kit::knob_jumps_do_not_click(&$kind, LIMITS.rough_glides, None);
            }

            #[test]
            fn knob_jumps_do_not_click_with_the_others_at_their_lowest() {
                kit::knob_jumps_do_not_click(&$kind, LIMITS.rough_glides, Some(false));
            }

            #[test]
            fn knob_jumps_do_not_click_with_the_others_at_their_highest() {
                kit::knob_jumps_do_not_click(&$kind, LIMITS.rough_glides, Some(true));
            }
        }
    };
}

pub(crate) use conformance;

pub fn poison_is_refused(kind: &EffectKind) {
    let mut effect = built(kind);
    let mut poison = guitar(0.25, 0.5);
    for (i, sample) in poison.iter_mut().enumerate() {
        match i % 5 {
            0 => *sample = f32::NAN,
            1 => *sample = f32::INFINITY,
            2 => *sample = f32::NEG_INFINITY,
            _ => {}
        }
    }
    let (left, right) = render(&mut *effect, &poison, &poison);
    assert_sane(&left, kind.id);
    assert_sane(&right, kind.id);
    let clean = guitar(0.5, 0.3);
    let after = render_mono(&mut *effect, &clean);
    assert_sane(&after, kind.id);
    assert!(
        rms(&after[4_800..]) > 1e-4,
        "{}: silent after poison",
        kind.id
    );
    effect.reset();
    let quiet = render_mono(&mut *effect, &vec![0.0; 24_000]);
    assert!(
        peak(&quiet[12_000..]) < 1e-5,
        "{}: poisoned tail {}",
        kind.id,
        peak(&quiet)
    );
}

pub fn silence_stays_silent(kind: &EffectKind) {
    let mut effect = built(kind);
    let out = render_mono(&mut *effect, &vec![0.0; 24_000]);
    assert!(
        peak(&out) < 1e-6,
        "{}: silence became {}",
        kind.id,
        peak(&out)
    );
    let mut effect = built(kind);
    render_mono(&mut *effect, &guitar(0.3, 0.5));
    let tail = render_mono(&mut *effect, &vec![0.0; 72_000]);
    let end = &tail[60_000..];
    assert!(
        peak(end) < 1e-5,
        "{}: tail rang on at {}",
        kind.id,
        peak(end)
    );
    let denormal = end.iter().any(|s| *s != 0.0 && s.abs() < f32::MIN_POSITIVE);
    assert!(!denormal, "{}: denormals in the tail", kind.id);
}

pub fn every_param_extreme_is_safe(kind: &EffectKind) {
    let input = guitar(0.25, 0.9);
    let loud = sine(3_000.0, 4.0, 0.1);
    let moderate = guitar(0.25, 0.25);
    for (index, spec) in kind.params.iter().enumerate() {
        for value in [spec.min, spec.max] {
            let mut effect = built(kind);
            effect.set_param(index, value);
            let (left, right) = render(&mut *effect, &input, &input);
            let what = format!("{} {}={value}", kind.id, spec.name);
            assert_sane(&left, &what);
            assert_sane(&right, &what);
            let hot = render_mono(&mut *effect, &loud);
            assert_sane(&hot, &what);
            let mut effect = built(kind);
            effect.set_param(index, value);
            let played = render_mono(&mut *effect, &moderate);
            let pinned = played.iter().filter(|s| s.abs() > 1.5).count();
            assert!(
                pinned * 100 < played.len(),
                "{what}: a -12 dBFS guitar sits on the ceiling {pinned} times"
            );
        }
    }
    // Every knob at its top, then every knob at its bottom, together.
    for top in [true, false] {
        let mut effect = built(kind);
        for (index, spec) in kind.params.iter().enumerate() {
            effect.set_param(index, if top { spec.max } else { spec.min });
        }
        let out = render_mono(&mut *effect, &input);
        assert_sane(&out, kind.id);
    }
}

pub fn nonsense_params_are_ignored(kind: &EffectKind) {
    let input = guitar(0.3, 0.5);
    let mut plain = built(kind);
    let mut pestered = built(kind);
    for index in 0..kind.params.len() {
        pestered.set_param(index, f32::NAN);
    }
    pestered.set_param(kind.params.len(), 1.0);
    pestered.set_param(usize::MAX, 0.5);
    let expected = render_mono(&mut *plain, &input);
    let got = render_mono(&mut *pestered, &input);
    assert_eq!(
        expected, got,
        "{}: nonsense params changed the sound",
        kind.id
    );
    for (index, spec) in kind.params.iter().enumerate() {
        pestered.set_param(index, f32::INFINITY);
        pestered.set_param(index, f32::NEG_INFINITY);
        pestered.set_param(index, spec.default);
    }
    let after = render_mono(&mut *pestered, &input);
    assert_sane(&after, kind.id);
}

pub fn odd_blocks_are_handled(kind: &EffectKind) {
    let mut effect = built(kind);
    let mut none_left: [f32; 0] = [];
    let mut none_right: [f32; 0] = [];
    effect.process(&CONTEXT, [&[], &[]], [&mut none_left, &mut none_right]);
    let input = guitar(0.1, 0.5);
    let mut left = vec![7.0f32; 4_800];
    let mut right = vec![7.0f32; 4_000];
    effect.process(
        &CONTEXT,
        [&input[..4_000], &input[..4_800]],
        [&mut left, &mut right],
    );
    assert!(left[4_000..].iter().all(|s| *s == 0.0), "{}", kind.id);
    assert_sane(&left, kind.id);
    assert_sane(&right, kind.id);
}

pub fn a_guitar_comes_through(kind: &EffectKind) {
    let mut effect = built(kind);
    let input = guitar(2.0, 0.3);
    let (left, right) = render(&mut *effect, &input, &input);
    assert_sane(&left, kind.id);
    assert_sane(&right, kind.id);
    assert!(rms(&left) > 1e-3, "{}: silent at {}", kind.id, rms(&left));
    assert!(rms(&right) > 1e-3, "{}: silent right", kind.id);
}

/// A sine at `hz` and peak `amplitude`, `seconds` long, at `rate`.
fn tone_at(rate: f32, hz: f64, amplitude: f32, seconds: f64) -> Vec<f32> {
    let n = (seconds * f64::from(rate)) as usize;
    (0..n)
        .map(|i| amplitude * (2.0 * PI * hz * i as f64 / f64::from(rate)).sin() as f32)
        .collect()
}

/// The worst inharmonic spur below 20 kHz, in dB below the loudest
/// harmonic (the fundamental, unless an octave effect cancels it),
/// and where it is, for a -6 dBFS tone near `hz` through `kind` at `rate`
/// with `settings` applied.
pub fn worst_spur(kind: &EffectKind, rate: f32, settings: &[(usize, f32)], hz: f64) -> (f64, f64) {
    let size = if rate < 60_000.0 {
        FFT_SIZE
    } else if rate < 120_000.0 {
        2 * FFT_SIZE
    } else {
        4 * FFT_SIZE
    };
    // An odd bin, so no harmonic can fold back onto another.
    let bin = (hz * size as f64 / f64::from(rate)).round() as usize | 1;
    let bin_hz = f64::from(rate) / size as f64;
    let mut effect = (kind.build)();
    effect.prepare(rate);
    for (index, value) in settings {
        effect.set_param(*index, *value);
    }
    effect.reset();
    let loud = tone_at(rate, bin as f64 * bin_hz, 0.5, 0.5);
    let power = spectrum_of(&render_mono(&mut *effect, &loud), size);
    let audible = ((20_000.0 / bin_hz) as usize).min(power.len());
    // Against the loudest harmonic, which is the fundamental but for an
    // octave fuzz, whose fundamental all but cancels.
    let fundamental: f64 = (bin..audible.saturating_sub(3))
        .step_by(bin)
        .map(|centre| power[centre - 3..=centre + 3].iter().sum::<f64>())
        .fold(0.0, f64::max);
    let mut worst = f64::MIN;
    let mut at = 0;
    for centre in 8..audible.saturating_sub(3) {
        let offset = centre % bin;
        let harmonic = offset <= 5 || bin - offset <= 5;
        if !harmonic {
            let spur: f64 = power[centre - 3..=centre + 3].iter().sum();
            let spur_db = 10.0 * (spur.max(1e-30) / fundamental.max(1e-30)).log10();
            if spur_db > worst {
                worst = spur_db;
                at = centre;
            }
        }
    }
    (worst, at as f64 * bin_hz)
}

/// The worst of the power at half, a quarter and one and a half times the
/// pitch, in dB below the fundamental, for a -6 dBFS tone through `kind`
/// at `rate` pitched at `internal / (n + 1/2)` near 5 kHz, where `internal`
/// is the rate the effect's nonlinearity runs at. At that pitch every
/// harmonic that folds at the internal rate lands on an odd multiple of
/// half the pitch, where it reads as period doubling; a solver that failed
/// to settle would put energy there too.
pub fn subharmonic_db(
    kind: &EffectKind,
    rate: f32,
    internal: f64,
    settings: &[(usize, f32)],
) -> f64 {
    let n = (internal / 5_000.0).floor();
    let hz = internal / (n + 0.5);
    let size = if rate < 60_000.0 {
        FFT_SIZE
    } else if rate < 120_000.0 {
        2 * FFT_SIZE
    } else {
        4 * FFT_SIZE
    };
    let mut effect = (kind.build)();
    effect.prepare(rate);
    for (index, value) in settings {
        effect.set_param(*index, *value);
    }
    effect.reset();
    let power = spectrum_of(
        &render_mono(&mut *effect, &tone_at(rate, hz, 0.5, 0.5)),
        size,
    );
    let near = |at: f64| {
        let bin = (at * size as f64 / f64::from(rate)).round() as usize;
        power[bin.saturating_sub(3)..(bin + 4).min(power.len())]
            .iter()
            .sum::<f64>()
    };
    let fundamental = near(hz);
    [hz / 2.0, hz / 4.0, 1.5 * hz]
        .iter()
        .map(|at| 10.0 * (near(*at).max(1e-30) / fundamental).log10())
        .fold(f64::MIN, f64::max)
}

/// The same effect at 44.1, 48, 96 and 192 kHz: the level of a 1 kHz tone
/// at -12 dBFS must agree across rates to within half a decibel, and sit
/// at unity where the effect promises it.
pub fn levels_agree_across_rates(kind: &EffectKind, limits: Limits) {
    let mut levels = Vec::new();
    for rate in RATES {
        let mut effect = (kind.build)();
        effect.prepare(rate);
        let quiet = tone_at(rate, 1_000.0, 0.25, 1.0);
        let out = render_mono(&mut *effect, &quiet);
        let half = out.len() / 2;
        levels.push(db(rms(&out[half..]) / rms(&quiet[half..])));
        if limits.unity {
            let mut effect = (kind.build)();
            effect.prepare(rate);
            let played = guitar_at(rate, 1.0, 0.25);
            let level = db(rms(&render_mono(&mut *effect, &played)) / rms(&played));
            let mut effect = (kind.build)();
            effect.prepare(rate);
            let sung = voice_at(rate, 1.0, 0.25);
            let voice = db(rms(&render_mono(&mut *effect, &sung)) / rms(&sung));
            assert!(
                level.abs() < 2.0,
                "{} at {rate} Hz: guitar level {level:.2} dB",
                kind.id
            );
            assert!(
                voice.abs() < 2.0,
                "{} at {rate} Hz: voice level {voice:.2} dB",
                kind.id
            );
        }
    }
    let spread = levels.iter().fold(f64::MIN, |m, l| m.max(*l))
        - levels.iter().fold(f64::MAX, |m, l| m.min(*l));
    assert!(spread < 0.5, "{}: level across rates {levels:?}", kind.id);
}

/// The worst inharmonic spur from a -6 dBFS tone near 5 kHz through the
/// effect at `rate` (aliasing, and any sub-harmonic from a solver that
/// fails to settle) must stay under the effect's limits, at its defaults
/// and with its knobs pushed hard.
pub fn spurs_are_held_down(kind: &EffectKind, limits: Limits, rate: f32) {
    let checks = std::iter::once((limits.worst_spur_db, &[][..]))
        .chain(limits.hot.iter().map(|&hot| (limits.hot_spur_db, hot)));
    for (limit, settings) in checks {
        let Some(limit) = limit else {
            continue;
        };
        let (worst, at) = worst_spur(kind, rate, settings, 4_987.0);
        assert!(
            worst < limit,
            "{} at {rate} Hz with {settings:?}: spur at {at:.0} Hz is {worst:.1} dBc (limit {limit})",
            kind.id
        );
    }
}

/// Preparing again at a new rate in the middle of a stream is taken at
/// once: the output stays finite, bounded and alive at every rate.
pub fn a_new_rate_mid_stream_is_taken(kind: &EffectKind) {
    let mut effect = built(kind);
    for rate in [96_000.0f32, 44_100.0, 192_000.0, 8_000.0, 48_000.0] {
        let input = tone_at(rate, 220.0, 0.3, 0.2);
        let out = render_mono(&mut *effect, &input);
        assert_sane(&out, kind.id);
        assert!(
            rms(&out[out.len() / 2..]) > 1e-4,
            "{} silent at {rate} Hz",
            kind.id
        );
        effect.prepare(rate);
    }
}

/// The biggest second difference in `samples`: how sharply the wave
/// bends from one sample to the next. A click is a kink, and shows here
/// even on a distorted wave whose steps are large at rest.
fn biggest_bend(samples: &[f32]) -> f32 {
    samples.windows(3).fold(0.0f32, |m, three| {
        m.max((2.0f32.mul_add(-three[1], three[2]) + three[0]).abs())
    })
}

/// Every knob thrown from one end to the other while a low tone plays,
/// with every other knob at its default (`corner` `None`), at its lowest
/// (`Some(false)`) or at its highest (`Some(true)`): the glide must not
/// click. A click is a bend during the change sharper than two and a half
/// times the sharpest the effect makes at rest at any setting the glide
/// passes through: its ends, and (when the ends alone do not account for
/// it) settings along the way, whose own bends can be sharper than either
/// end's (a folder's symmetry passes through its fullest fold).
pub fn knob_jumps_do_not_click(kind: &EffectKind, rough: &[usize], corner: Option<bool>) {
    let input = sine(110.0, 0.3, 0.4);
    let jump = input.len() / 2;
    for (index, spec) in kind.params.iter().enumerate() {
        if rough.contains(&index) {
            continue;
        }
        for (from, to) in [(spec.min, spec.max), (spec.max, spec.min)] {
            let set_up = |value: f32| {
                let mut effect = built(kind);
                if let Some(top) = corner {
                    for (other, other_spec) in kind.params.iter().enumerate() {
                        if other != index && !rough.contains(&other) {
                            let end = if top { other_spec.max } else { other_spec.min };
                            effect.set_param(other, end);
                        }
                    }
                }
                effect.set_param(index, value);
                effect.reset();
                effect
            };
            let mut effect = set_up(from);
            let before = render_mono(&mut *effect, &input[..jump]);
            effect.set_param(index, to);
            let after = render_mono(&mut *effect, &input[jump..]);
            let rest_before = biggest_bend(&before[jump - 2_400..]);
            let rest_after = biggest_bend(&after[after.len() - 2_400..]);
            let during = biggest_bend(&after[..2_400]);
            let mut sharpest = rest_before.max(rest_after);
            if during > 2.5f32.mul_add(sharpest, 0.002) {
                for share in [0.25f32, 0.5, 0.75] {
                    let mut still = set_up((to - from).mul_add(share, from));
                    let rest = render_mono(&mut *still, &input[..jump]);
                    sharpest = sharpest.max(biggest_bend(&rest[jump - 2_400..]));
                }
            }
            let allowed = 2.5f32.mul_add(sharpest, 0.002);
            assert!(
                during <= allowed,
                "{} {} {from} -> {to} (others {corner:?}): bend {during} against {allowed}",
                kind.id,
                spec.name
            );
        }
    }
}

/// How many samples `signal` lags a sine at `hz` and `rate` that started
/// at phase zero, beyond `lag` whole samples, measured from sample `from`
/// on: the sine and cosine `lag` samples back are fitted to it by least
/// squares (over a span that is not a whole number of cycles the two are
/// not quite orthogonal), so its level does not count, only its phase.
fn delay_beyond(signal: &[f32], hz: f64, rate: f64, from: usize, lag: usize) -> f64 {
    let w = 2.0 * PI * hz / rate;
    let (mut ys, mut yc, mut ss, mut sc, mut cc) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (i, y) in signal.iter().enumerate().skip(from) {
        let (sin, cos) = (w * (i as f64 - lag as f64)).sin_cos();
        let y = f64::from(*y);
        ys = y.mul_add(sin, ys);
        yc = y.mul_add(cos, yc);
        ss = sin.mul_add(sin, ss);
        sc = sin.mul_add(cos, sc);
        cc = cos.mul_add(cos, cc);
    }
    let det = ss.mul_add(cc, -(sc * sc));
    let a = ys.mul_add(cc, -(yc * sc)) / det;
    let b = yc.mul_add(ss, -(ys * sc)) / det;
    // y = g sin(w (i - lag - e)) = g (cos(w e) sin - sin(w e) cos).
    (-b).atan2(a) / w
}

/// Where the latency check takes its reference: a rate at which no model
/// oversamples (every target is 705.6 kHz or less), so what is left of the
/// effect's delay beyond its declared latency is its circuit's own phase,
/// the sound, discretised finely enough to stand for the analogue one.
const REFERENCE_RATE: f32 = 705_600.0;

/// The declared latency is the real one, exactly: at every rate, a quiet
/// 250 Hz sine comes out as late, in seconds, as through the same effect
/// at [`REFERENCE_RATE`], once each rate's declared latency is taken off,
/// to within a twentieth of a sample, with `knobs` set. A wrong
/// antiderivative delay (half a sample at the circuit's rate is an eighth
/// of one at 44.1 kHz) shows. (A plain build at the host's rate is no
/// reference: its own coarse steps lag by fractions of a sample. And the
/// tone is low because a filter run at the host's rate, as an amp's
/// cabinet is, differs from its finely stepped self by a phase that grows
/// with the square of the frequency: part of its sound, not its latency.)
pub fn latency_is_declared(kind: &EffectKind, knobs: &[(usize, f32)]) {
    const HZ: f64 = 250.0;
    // The delay beyond the declared latency, in seconds, at `rate`.
    let beyond = |rate: f32| {
        let mut effect = (kind.build)();
        effect.prepare(rate);
        for &(index, value) in knobs {
            effect.set_param(index, value);
        }
        effect.reset();
        let input = tone_at(rate, HZ, 0.05, 0.5);
        let out = render_mono(&mut *effect, &input);
        let rate = f64::from(rate);
        delay_beyond(&out, HZ, rate, input.len() / 2, effect.latency()) / rate
    };
    let reference = beyond(REFERENCE_RATE);
    for rate in RATES {
        let off = (beyond(rate) - reference) * f64::from(rate);
        assert!(
            off.abs() < 0.05,
            "{} at {rate} Hz: off its declared latency by {off:.3} samples",
            kind.id
        );
    }
}
