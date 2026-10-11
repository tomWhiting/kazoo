//! Allocation-free modal synthesis.
//!
//! Each strike starts eight damped complex
//! oscillators; their ratios morph from harmonic tines to inharmonic bells.
//! Modes above 45% of the sample rate are omitted rather than aliased.

use crate::patch::{PATCHES, Patch};

pub const VOICES: usize = 8;
pub const MODES: usize = 8;
pub const SCOPE_LEN: usize = 512;
const GHOSTS: usize = 4;
const QUIET_SQUARED: f64 = 1.0e-14;

/// Independent note ownership: releasing a phrase or keyboard note cannot
/// release the same pitch being played from the hub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Player,
    Phrase,
    Hub,
}

/// Bell-inspired modal ratios, with a clearly tuned fundamental.
const BELL_RATIOS: [f32; MODES] = [1.0, 2.756, 5.404, 8.933, 13.34, 18.64, 24.83, 31.87];

/// The ratios shown on the front panel and used by the sound engine.
#[must_use]
pub fn mode_ratios(material: f32) -> [f32; MODES] {
    let material = if material.is_finite() {
        material.clamp(0.0, 1.0)
    } else {
        0.5
    };
    std::array::from_fn(|i| {
        let harmonic = (i + 1) as f32;
        material.mul_add(BELL_RATIOS[i] - harmonic, harmonic)
    })
}

#[derive(Debug, Clone, Copy, Default)]
struct Mode {
    real: f64,
    imag: f64,
    cos: f64,
    sin: f64,
    radius: f64,
    pan: [f32; 2],
}

impl Mode {
    fn process(&mut self) -> f32 {
        if self.real.mul_add(self.real, self.imag * self.imag) < QUIET_SQUARED {
            self.real = 0.0;
            self.imag = 0.0;
            return 0.0;
        }
        let real = self.real.mul_add(self.cos, -self.imag * self.sin) * self.radius;
        self.imag = self.real.mul_add(self.sin, self.imag * self.cos) * self.radius;
        self.real = real;
        self.imag as f32
    }

    fn sounding(self) -> bool {
        self.real != 0.0 || self.imag != 0.0
    }
}

#[derive(Debug, Clone, Copy)]
struct Voice {
    modes: [Mode; MODES],
    note: u8,
    source: Source,
    gate: bool,
    started: u64,
}

impl Voice {
    const EMPTY: Self = Self {
        modes: [Mode {
            real: 0.0,
            imag: 0.0,
            cos: 0.0,
            sin: 0.0,
            radius: 0.0,
            pan: [0.0; 2],
        }; MODES],
        note: 0,
        source: Source::Player,
        gate: false,
        started: 0,
    };

    fn sounding(&self) -> bool {
        self.modes.iter().any(|m| m.sounding())
    }

    fn damp(&mut self, seconds: f32, rate: f32) {
        self.gate = false;
        let radius = t60_radius(seconds, rate);
        for mode in &mut self.modes {
            mode.radius = mode.radius.min(radius);
        }
    }

    fn process(&mut self) -> [f32; 2] {
        let mut out = [0.0; 2];
        for mode in &mut self.modes {
            let sample = mode.process();
            for (channel, pan) in out.iter_mut().zip(mode.pan) {
                *channel = sample.mul_add(pan, *channel);
            }
        }
        out
    }
}

fn t60_radius(seconds: f32, rate: f32) -> f64 {
    (-std::f64::consts::LN_10 * 3.0 / (f64::from(seconds) * f64::from(rate))).exp()
}

/// Eight voices and four short stolen-note tails, all stored inline.
#[derive(Debug)]
pub struct GlassSynth {
    sample_rate: f32,
    patch: Patch,
    voices: [Voice; VOICES],
    ghosts: [Voice; GHOSTS],
    clock: u64,
    master: f32,
    smoothed_master: f32,
    scope: [f32; SCOPE_LEN],
    scope_pos: usize,
}

impl GlassSynth {
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        // Defend pathological rates while accepting every normal audio rate.
        let sample_rate = if sample_rate.is_finite() && (8_000.0..=384_000.0).contains(&sample_rate)
        {
            sample_rate
        } else {
            48_000.0
        };
        Self {
            sample_rate,
            patch: PATCHES[0],
            voices: [Voice::EMPTY; VOICES],
            ghosts: [Voice::EMPTY; GHOSTS],
            clock: 0,
            master: 0.8,
            smoothed_master: 0.8,
            scope: [0.0; SCOPE_LEN],
            scope_pos: 0,
        }
    }

    /// Edits voice the next strike; already ringing notes retain their material.
    pub fn set_patch(&mut self, patch: Patch) {
        self.patch = patch.sanitized();
    }

    pub const fn set_master(&mut self, master: f32) {
        if master.is_finite() {
            self.master = master.clamp(0.0, 1.0);
        }
    }

    pub fn note_on(&mut self, note: u8, velocity: u8) {
        self.note_on_from(Source::Player, note, velocity);
    }

    pub fn note_on_from(&mut self, source: Source, note: u8, velocity: u8) {
        if note > 127 {
            return;
        }
        if velocity == 0 {
            self.note_off_from(source, note);
            return;
        }
        let slot = self.pick_voice(source, note);
        if self.voices[slot].sounding() {
            // Retire the previous strike before retriggering; no abrupt reset.
            let ghost = self
                .ghosts
                .iter()
                .position(|g| !g.sounding())
                .unwrap_or_else(|| {
                    self.ghosts
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, g)| g.started)
                        .map_or(0, |(i, _)| i)
                });
            self.ghosts[ghost] = self.voices[slot];
            self.ghosts[ghost].damp(0.025, self.sample_rate);
        }
        self.clock = self.clock.wrapping_add(1);
        let velocity = f32::from(velocity.min(127)) / 127.0;
        let ratios = mode_ratios(self.patch.material);
        let hz = midi_to_hz(note);
        let hardness = self.patch.brightness * 0.5_f32.mul_add(velocity, 0.5);
        let position = self.patch.position.mul_add(0.45, 0.05);
        let weights: [f32; MODES] = std::array::from_fn(|i| {
            if hz * ratios[i] >= self.sample_rate * 0.45 {
                return 0.0;
            }
            let strike = (std::f32::consts::PI * (i + 1) as f32 * position)
                .sin()
                .abs();
            // Always retain the fundamental; other modes have strike nodes.
            let strike = if i == 0 { 1.0 } else { strike };
            strike * (-(i as f32) * (1.0 - hardness).mul_add(1.8, 0.12)).exp()
                / ((i + 1) as f32).sqrt()
        });
        let norm = weights.iter().sum::<f32>().max(1.0e-6);
        let level = 0.45 * velocity.powf(1.4) / norm;
        let modes = std::array::from_fn(|i| {
            if weights[i] == 0.0 {
                return Mode::default();
            }
            let angle =
                std::f64::consts::TAU * f64::from(hz * ratios[i]) / f64::from(self.sample_rate);
            let decay =
                self.patch.decay_seconds() / (self.patch.damping * i as f32).mul_add(1.5, 1.0);
            let pan = if i % 2 == 0 { -1.0 } else { 1.0 } * self.patch.width * 0.7;
            let pan_angle = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
            Mode {
                real: f64::from(weights[i] * level),
                imag: 0.0,
                cos: angle.cos(),
                sin: angle.sin(),
                radius: t60_radius(decay, self.sample_rate),
                pan: [
                    pan_angle.cos() * std::f32::consts::SQRT_2,
                    pan_angle.sin() * std::f32::consts::SQRT_2,
                ],
            }
        });
        self.voices[slot] = Voice {
            modes,
            note,
            source,
            gate: true,
            started: self.clock,
        };
    }

    fn pick_voice(&self, source: Source, note: u8) -> usize {
        self.voices
            .iter()
            .position(|v| v.source == source && v.note == note && v.sounding())
            .or_else(|| self.voices.iter().position(|v| !v.sounding()))
            .or_else(|| {
                self.voices
                    .iter()
                    .enumerate()
                    .filter(|(_, v)| !v.gate)
                    .min_by_key(|(_, v)| v.started)
                    .map(|(i, _)| i)
            })
            .unwrap_or_else(|| {
                self.voices
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, v)| v.started)
                    .map_or(0, |(i, _)| i)
            })
    }

    pub fn note_off(&mut self, note: u8) {
        self.note_off_from(Source::Player, note);
    }

    pub fn note_off_from(&mut self, source: Source, note: u8) {
        for voice in self
            .voices
            .iter_mut()
            .filter(|v| v.gate && v.note == note && v.source == source)
        {
            voice.damp(self.patch.release_seconds(), self.sample_rate);
        }
    }

    pub fn release_source(&mut self, source: Source) {
        for voice in self
            .voices
            .iter_mut()
            .filter(|v| v.gate && v.source == source)
        {
            voice.damp(self.patch.release_seconds(), self.sample_rate);
        }
    }

    /// Panic control: quickly damps held notes, natural tails and stolen voices.
    pub fn all_notes_off(&mut self) {
        for voice in self.voices.iter_mut().chain(&mut self.ghosts) {
            voice.damp(0.04, self.sample_rate);
        }
    }

    #[must_use]
    pub fn voice_states(&self) -> [Option<(u8, bool)>; VOICES] {
        self.voices
            .map(|v| v.sounding().then_some((v.note, v.gate)))
    }

    #[must_use]
    pub const fn scope(&self) -> (&[f32; SCOPE_LEN], usize) {
        (&self.scope, self.scope_pos)
    }

    /// One bounded, finite stereo frame. No allocation, locks or I/O.
    pub fn process(&mut self) -> [f32; 2] {
        let mut out = [0.0_f32; 2];
        for voice in self.voices.iter_mut().chain(&mut self.ghosts) {
            if voice.sounding() {
                let frame = voice.process();
                for (out, sample) in out.iter_mut().zip(frame) {
                    *out += sample;
                }
            }
        }
        // A 5 ms level smoother avoids zipper noise when the fader moves.
        let step = 1.0 / (0.005 * self.sample_rate);
        self.smoothed_master += step * (self.master - self.smoothed_master);
        let out = out.map(|s| {
            let sample = s * self.smoothed_master;
            if sample.is_finite() {
                sample.tanh()
            } else {
                0.0
            }
        });
        self.scope[self.scope_pos] = 0.5 * (out[0] + out[1]);
        self.scope_pos = (self.scope_pos + 1) % SCOPE_LEN;
        out
    }
}

#[must_use]
pub fn midi_to_hz(note: u8) -> f32 {
    440.0 * ((f32::from(note) - 69.0) / 12.0).exp2()
}

#[must_use]
pub fn note_name(note: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!(
        "{}{}",
        NAMES[usize::from(note % 12)],
        i16::from(note) / 12 - 1
    )
}

#[cfg(test)]
#[path = "synth_tests.rs"]
mod tests;
