//! Deterministic analytic voice takes, with a nonzero unvoiced floor.

use super::{SCORE_STEP, Take};
use std::f64::consts::{PI, TAU};

#[derive(Clone, Copy)]
enum Sound {
    Sine,
    Hum,
    Bait,
    Vowel,
    Bursts,
    Whistle,
    Syllables,
    HighWhistle,
    SungSecond,
    SungThird,
    Spliced,
}

impl Sound {
    const fn needs_peak_normalisation(self) -> bool {
        matches!(
            self,
            Self::Hum
                | Self::Bait
                | Self::Vowel
                | Self::Whistle
                | Self::HighWhistle
                | Self::SungSecond
                | Self::SungThird
                | Self::Spliced
        )
    }
}

struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (f64::from(self.0) / f64::from(u32::MAX)).mul_add(2.0, -1.0)
    }
}

struct Resonator {
    a1: f64,
    a2: f64,
    gain: f64,
    y1: f64,
    y2: f64,
}

impl Resonator {
    fn new(rate: u32, frequency: f64, bandwidth: f64) -> Self {
        let r = (-PI * bandwidth / f64::from(rate)).exp();
        Self {
            a1: 2.0 * r * (TAU * frequency / f64::from(rate)).cos(),
            a2: r * r,
            gain: 1.0 - r,
            y1: 0.0,
            y2: 0.0,
        }
    }
    fn push(&mut self, input: f64) -> f64 {
        let output = self
            .gain
            .mul_add(input, self.a1.mul_add(self.y1, -self.a2 * self.y2));
        self.y2 = self.y1;
        self.y1 = output;
        output
    }
}

fn syllable_gain(time: f64) -> f64 {
    let position = time.rem_euclid(0.2);
    let db = if time >= 0.2 && position < 0.01 {
        -24.0
    } else if time >= 0.2 && position < 0.02 {
        -24.0 * (0.02 - position) / 0.01
    } else if time < 0.8 && position >= 0.18 {
        -24.0 * ((position - 0.18) / 0.01).min(1.0)
    } else {
        0.0
    };
    10.0_f64.powf(db / 20.0)
}

fn high_tone(phase: f64, noise: f64, harmonics: [f64; 3], breath: f64) -> f64 {
    let [fundamental, second, third] = harmonics;
    breath.mul_add(
        noise,
        third.mul_add(
            (3.0 * phase).sin(),
            second.mul_add((2.0 * phase).sin(), fundamental * phase.sin()),
        ),
    )
}

fn splice_phase(phase: f64, hz: f64, rate: u32, index: u32) -> f64 {
    if index > 0 && index % (rate / 20).max(1) == 0 {
        (phase + TAU * hz * 128.0 / f64::from(rate)).rem_euclid(TAU)
    } else {
        phase
    }
}

fn make(
    name: &str,
    rate: u32,
    duration_ms: u32,
    seed: u32,
    sound: Sound,
    pitch: impl Fn(f64) -> Option<f64>,
) -> Take {
    let floor = rate / 5;
    let body = (u64::from(rate) * u64::from(duration_ms) / 1000) as u32;
    let length = body.saturating_add(floor.saturating_mul(2));
    let mut noise = Noise(seed);
    let mut phase = 0.0_f64;
    let mut onsets = Vec::new();
    let mut was_voiced = false;
    let mut samples = Vec::with_capacity(length as usize);
    let mut resonators = [
        Resonator::new(rate, 700.0, 80.0),
        Resonator::new(rate, 1220.0, 90.0),
        Resonator::new(rate, 2600.0, 120.0),
    ];
    for index in 0..length {
        if index < floor || index >= floor + body {
            samples.push((noise.next() * 0.000_316) as f32);
            was_voiced = false;
            continue;
        }
        let time = f64::from(index - floor) / f64::from(rate);
        let hz = pitch(time);
        if hz.is_some() && !was_voiced {
            onsets.push(index as usize);
        }
        was_voiced = hz.is_some();
        let value = if let Some(hz) = hz {
            let value = match sound {
                Sound::Hum => 0.25_f64.mul_add(
                    (3.0 * phase).sin(),
                    0.5_f64.mul_add((2.0 * phase).sin(), phase.sin()),
                ),
                Sound::Bait => 0.3_f64.mul_add(
                    (3.0 * phase).sin(),
                    1.5_f64.mul_add((2.0 * phase).sin(), phase.sin()),
                ),
                Sound::Vowel => {
                    let harmonics = (0.45 * f64::from(rate) / hz).floor() as u32;
                    let saw = (1..=harmonics)
                        .map(|k| (f64::from(k) * phase).sin() / f64::from(k))
                        .sum::<f64>();
                    resonators.iter_mut().map(|filter| filter.push(saw)).sum()
                }
                Sound::Whistle => 0.01_f64.mul_add(noise.next(), 0.5 * phase.sin()),
                Sound::HighWhistle => high_tone(phase, noise.next(), [0.5, 0.0158, 0.0], 0.05),
                Sound::SungSecond => high_tone(phase, noise.next(), [1.0, 1.2, 0.3], 0.02),
                Sound::SungThird => high_tone(phase, noise.next(), [1.0, 0.4, 1.1], 0.02),
                Sound::Spliced => {
                    phase = splice_phase(phase, hz, rate, index - floor);
                    high_tone(phase, noise.next(), [0.5, 0.0158, 0.0], 0.05)
                }
                Sound::Syllables => 0.5 * phase.sin() * syllable_gain(time),
                Sound::Sine | Sound::Bursts => 0.5 * phase.sin(),
            };
            phase = (phase + TAU * hz / f64::from(rate)).rem_euclid(TAU);
            value
        } else {
            let amplitude = if matches!(sound, Sound::Bursts) {
                0.25
            } else {
                0.000_316
            };
            noise.next() * amplitude
        };
        samples.push(value as f32);
    }
    if sound.needs_peak_normalisation() {
        let active = &mut samples[floor as usize..(floor + body) as usize];
        let peak = active
            .iter()
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        if peak > 0.0 {
            for sample in active {
                *sample *= 0.5 / peak;
            }
        }
    }
    let truth: Vec<_> = (0..samples.len().div_ceil(SCORE_STEP))
        .map(|step| {
            let centre = (step * SCORE_STEP + SCORE_STEP / 2).min(samples.len().saturating_sub(1));
            let centre = u32::try_from(centre).unwrap_or(u32::MAX);
            if centre < floor || centre >= floor + body {
                None
            } else {
                pitch(f64::from(centre - floor) / f64::from(rate)).map(|hz| hz as f32)
            }
        })
        .collect();
    if matches!(sound, Sound::Syllables) {
        onsets = (0..5).map(|i| (floor + i * rate / 5) as usize).collect();
    }
    Take {
        name: name.into(),
        rate,
        samples,
        truth,
        onsets,
        exact: true,
    }
}

/// The ten analytic takes, with 200 ms of floor on both ends.
#[must_use]
pub fn suite(rate: u32) -> Vec<Take> {
    vec![
        make("sine_ladder", rate, 2900, 1, Sound::Sine, |t| {
            let note = (t / 0.5).floor() as usize;
            if t.rem_euclid(0.5) < 0.4 {
                [110.0, 220.0, 440.0, 880.0, 1760.0, 3000.0]
                    .get(note)
                    .copied()
            } else {
                None
            }
        }),
        make("hum_low", rate, 600, 2, Sound::Hum, |_| Some(65.406)),
        make("steps", rate, 1750, 3, Sound::Sine, |t| {
            [48.0, 52.0, 55.0, 60.0, 55.0, 52.0, 48.0]
                .get((t / 0.25).floor() as usize)
                .map(|note| 440.0 * ((note - 69.0) / 12.0_f64).exp2())
        }),
        make("vibrato", rate, 1000, 4, Sound::Sine, |t| {
            Some(220.0 * (50.0 * (TAU * 6.0 * t).sin() / 1200.0).exp2())
        }),
        make("glide", rate, 900, 5, Sound::Sine, |t| {
            Some(110.0 * 4.0_f64.powf(((t - 0.2) / 0.5).clamp(0.0, 1.0)))
        }),
        make("octave_bait", rate, 600, 6, Sound::Bait, |_| Some(150.0)),
        make("vowel_ah", rate, 800, 7, Sound::Vowel, |_| Some(130.81)),
        make("noise_bursts", rate, 1300, 8, Sound::Bursts, |t| {
            if t < 0.3 {
                Some(220.0)
            } else if t < 0.5 {
                None
            } else if t < 0.8 {
                Some(330.0)
            } else if t < 1.0 {
                None
            } else {
                Some(440.0)
            }
        }),
        make("whistle", rate, 800, 9, Sound::Whistle, |t| {
            Some(1500.0 * (20.0 * (TAU * 5.0 * t).sin() / 1200.0).exp2())
        }),
        make("rearticulate", rate, 1000, 10, Sound::Syllables, |_| {
            Some(220.0)
        }),
    ]
}

/// Sixteen three-second high notes with a ±15-cent, 5 Hz wobble and 200 ms
/// of floor at each end: whistles, strong second or third harmonics, and
/// whistles with a 128-sample splice every 50 ms.
#[must_use]
pub fn high_suite(rate: u32) -> Vec<Take> {
    let mut takes = Vec::with_capacity(16);
    for frequency in [800_u32, 1100, 1500, 2200] {
        for (prefix, sound, seed) in [
            ("whistle", Sound::HighWhistle, 11),
            ("sung2", Sound::SungSecond, 12),
            ("sung3", Sound::SungThird, 13),
            ("splice", Sound::Spliced, 14),
        ] {
            takes.push(make(
                &format!("{prefix}_{frequency}"),
                rate,
                3000,
                seed + frequency,
                sound,
                |time| {
                    Some(f64::from(frequency) * (15.0 * (TAU * 5.0 * time).sin() / 1200.0).exp2())
                },
            ));
        }
    }
    takes
}
