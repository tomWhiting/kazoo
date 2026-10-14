//! Deterministic plucked strings with sample-clock pitch and articulation truth.

use super::{SCORE_STEP, Take};
use std::f64::consts::TAU;

struct Noise(u32);
impl Noise {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (f64::from(self.0) / f64::from(u32::MAX)).mul_add(2.0, -1.0)
    }
}

/// Fractional delay lets a plucked string bend without rounding to sample lengths.
#[derive(Debug)]
pub(crate) struct StringTone {
    delay: Vec<f64>,
    position: usize,
    rate: f64,
}
impl StringTone {
    pub(crate) fn new(rate: u32, hz: f64, lowest: f64, pluck: f64, seed: u32) -> Self {
        let period = f64::from(rate) / hz;
        let length = (f64::from(rate) / lowest).ceil() as usize + 4;
        // Start at the rising zero, including the fractional read position,
        // so the first negative lobe spans the string's displacement.
        let phase_offset = pluck.mul_add(0.5, -(length as f64 - period + 0.5) / period);
        let mut noise = Noise(seed);
        let delay = (0..length)
            .map(|index| {
                let phase = (index as f64 / period + phase_offset).rem_euclid(1.0);
                let displacement = if phase < pluck {
                    phase / pluck
                } else {
                    (1.0 - phase) / (1.0 - pluck)
                };
                (displacement - 0.5).mul_add(0.9, noise.next() * 0.005)
            })
            .collect();
        Self {
            delay,
            position: 0,
            rate: f64::from(rate),
        }
    }

    fn noise(rate: u32, lowest: f64, seed: u32) -> Self {
        let length = (f64::from(rate) / lowest).ceil() as usize + 4;
        let mut noise = Noise(seed);
        Self {
            delay: (0..length).map(|_| noise.next()).collect(),
            position: 0,
            rate: f64::from(rate),
        }
    }

    fn read(&self, delay: f64) -> f64 {
        let position = (self.position as f64 - delay).rem_euclid(self.delay.len() as f64);
        let before = position.floor() as usize;
        let fraction = position - before as f64;
        let after = (before + 1) % self.delay.len();
        self.delay[before].mul_add(1.0 - fraction, self.delay[after] * fraction)
    }

    pub(crate) fn push(&mut self, hz: f64) -> f64 {
        // The two-point average contributes half a sample of loop delay;
        // subtract it so the loop's instantaneous period is rate / hz.
        let delay = self.rate / hz - 0.5;
        let output = self.read(delay);
        self.delay[self.position] = 0.996 * 0.5 * (output + self.read(delay + 1.0));
        self.position = (self.position + 1) % self.delay.len();
        output
    }
}

#[derive(Clone, Copy)]
enum Sound {
    String,
    NoiseString,
    Sweep,
    Sine,
    NoiseHum,
}

fn count(rate: u32, ms: u32) -> usize {
    (u64::from(rate) * u64::from(ms) / 1000) as usize
}

fn make(
    name: &str,
    rate: u32,
    duration_ms: u32,
    sound: Sound,
    pluck_ms: &[u32],
    pitch: impl Fn(usize) -> f64,
) -> Take {
    let floor = count(rate, 200);
    let body = count(rate, duration_ms);
    let length = body + 2 * floor;
    let plucks: Vec<_> = pluck_ms.iter().map(|&ms| count(rate, ms)).collect();
    let mut strings: Vec<(StringTone, f64)> = Vec::new();
    let mut samples = vec![0.0; length];
    let mut noise = Noise(0x9e37_79b9);
    let mut phase = 0.0_f64;
    for (index, sample) in samples[floor..floor + body].iter_mut().enumerate() {
        let time = index as f64 / f64::from(rate);
        let hz = pitch(index);
        if let Some(pluck) = plucks.iter().position(|&at| at == index) {
            if matches!(sound, Sound::String | Sound::NoiseString) {
                strings.clear();
            }
            if matches!(sound, Sound::String | Sound::NoiseString | Sound::Sweep) {
                let string = if matches!(sound, Sound::NoiseString) {
                    StringTone::noise(rate, 29.0, 1 + pluck as u32)
                } else {
                    StringTone::new(rate, hz, 29.0, 0.25, 1 + pluck as u32)
                };
                strings.push((string, hz));
            }
        }
        let value = match sound {
            Sound::String | Sound::NoiseString => {
                strings.iter_mut().map(|(string, _)| string.push(hz)).sum()
            }
            Sound::Sweep => strings
                .iter_mut()
                .map(|(string, hz)| string.push(*hz))
                .sum(),
            Sound::Sine => {
                let value = 0.5 * phase.sin();
                phase = (phase + TAU * hz / f64::from(rate)).rem_euclid(TAU);
                value
            }
            Sound::NoiseHum => {
                let hum = 10.0_f64.powf(-55.0 / 20.0) * (TAU * 50.0 * time).sin();
                // Each short pick is noise, with no periodic pitch truth.
                let pick = if time.rem_euclid(0.2) < 0.002 {
                    0.1
                } else {
                    0.0
                };
                noise.next().mul_add(pick, hum)
            }
        };
        *sample = value as f32;
    }
    if matches!(sound, Sound::String | Sound::NoiseString | Sound::Sweep) {
        let peak = samples
            .iter()
            .copied()
            .map(f32::abs)
            .fold(0.0_f32, f32::max);
        if peak > 0.0 {
            for sample in &mut samples {
                *sample *= 0.5 / peak;
            }
        }
    }
    let truth = (0..length.div_ceil(SCORE_STEP))
        .map(|step| {
            let centre = (step * SCORE_STEP + SCORE_STEP / 2).min(length - 1);
            if centre < floor || centre >= floor + body || matches!(sound, Sound::NoiseHum) {
                None
            } else {
                Some(pitch(centre - floor) as f32)
            }
        })
        .collect();
    Take {
        name: name.into(),
        rate,
        samples,
        truth,
        onsets: if matches!(sound, Sound::NoiseHum) {
            Vec::new()
        } else {
            plucks.into_iter().map(|at| floor + at).collect()
        },
        exact: true,
    }
}

fn hz(note: u32) -> f64 {
    440.0 * ((f64::from(note) - 69.0) / 12.0).exp2()
}

fn note_at(sample: usize, rate: u32, pluck_ms: &[u32], notes: &[u32]) -> f64 {
    // Pitch and excitation use the same integer clock, including a rate
    // where a millisecond is a fractional sample.
    let note = pluck_ms.partition_point(|&ms| count(rate, ms) <= sample) - 1;
    hz(notes[note])
}

fn open_string(
    name: &str,
    rate: u32,
    duration_ms: u32,
    sound: Sound,
    plucks: &[u32],
    notes: &[u32],
) -> Take {
    make(name, rate, duration_ms, sound, plucks, |sample| {
        note_at(sample, rate, plucks, notes)
    })
}

/// Ten exact takes: open strings, overlapping sweep, moving strings and hum.
///
/// Strings use triangular pick displacement with seeded scrape, a two-point
/// average and 0.996 loop loss. Two worst-case takes retain white-noise excitation. Fractional delays glide continuously; truth is their instantaneous
/// loop frequency. The sweep retains every earlier string under each pluck.
#[must_use]
pub fn instrument_suite(rate: u32) -> Vec<Take> {
    assert!(
        rate >= 8000,
        "instrument takes need at least an 8 kHz sample rate"
    );
    vec![
        open_string(
            "bass_open",
            rate,
            7500,
            Sound::String,
            &[0, 1500, 3000, 4500, 6000],
            &[23, 28, 33, 38, 43],
        ),
        open_string(
            "guitar_open",
            rate,
            6000,
            Sound::String,
            &[0, 1000, 2000, 3000, 4000, 5000],
            &[40, 45, 50, 55, 59, 64],
        ),
        make(
            "sweep",
            rate,
            800,
            Sound::Sweep,
            &[0, 70, 140, 210, 280, 350],
            |sample| {
                note_at(
                    sample,
                    rate,
                    &[0, 70, 140, 210, 280, 350],
                    &[45, 48, 52, 57, 60, 64],
                )
            },
        ),
        make("slide", rate, 1200, Sound::String, &[0], |sample| {
            let t = sample as f64 / f64::from(rate);
            let progress = if t < 0.2 {
                0.0
            } else if t < 0.6 {
                (t - 0.2) / 0.4
            } else if t < 1.0 {
                (1.0 - t) / 0.4
            } else {
                0.0
            };
            hz(45) * (700.0 * progress / 1200.0).exp2()
        }),
        make("bend", rate, 1000, Sound::String, &[0], |sample| {
            let t = sample as f64 / f64::from(rate);
            let progress = if t < 0.2 {
                0.0
            } else if t < 0.35 {
                (t - 0.2) / 0.15
            } else if t < 0.65 {
                1.0
            } else if t < 0.8 {
                (0.8 - t) / 0.15
            } else {
                0.0
            };
            hz(55) * (200.0 * progress / 1200.0).exp2()
        }),
        make("vibrato", rate, 1500, Sound::String, &[0], |sample| {
            let t = sample as f64 / f64::from(rate);
            hz(50) * (30.0 * (TAU * 6.0 * t).sin() / 1200.0).exp2()
        }),
        make("glide", rate, 2000, Sound::Sine, &[0], |sample| {
            let t = sample as f64 / f64::from(rate);
            hz(45) * (500.0 * t / 2.0 / 1200.0).exp2()
        }),
        make("noise_hum", rate, 1500, Sound::NoiseHum, &[], |_| 50.0),
        open_string(
            "bass_open_noise",
            rate,
            7500,
            Sound::NoiseString,
            &[0, 1500, 3000, 4500, 6000],
            &[23, 28, 33, 38, 43],
        ),
        open_string(
            "guitar_open_noise",
            rate,
            6000,
            Sound::NoiseString,
            &[0, 1000, 2000, 3000, 4000, 5000],
            &[40, 45, 50, 55, 59, 64],
        ),
    ]
}
