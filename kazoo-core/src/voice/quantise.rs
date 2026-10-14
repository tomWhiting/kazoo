//! Scale correction of voice pitch, with a direct path and retained vibrato.

use super::{VOICE_HOP, VoiceFrame, hz_from_volts};
use std::f32::consts::TAU;

/// Quantiser scales: the semitones above the root each holds.
pub const SCALES: [&[u8]; 10] = [
    &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
    &[0, 2, 4, 5, 7, 9, 11],
    &[0, 2, 3, 5, 7, 8, 10],
    &[0, 2, 3, 5, 7, 9, 10],
    &[0, 1, 3, 5, 7, 8, 10],
    &[0, 2, 4, 6, 7, 9, 11],
    &[0, 2, 4, 5, 7, 9, 10],
    &[0, 2, 4, 7, 9],
    &[0, 3, 5, 7, 10],
    &[0, 2, 4, 6, 8, 10],
];

/// The scales' names.
pub const SCALE_NAMES: [&str; 10] = [
    "chromatic",
    "major",
    "minor",
    "dorian",
    "phrygian",
    "lydian",
    "mixolydian",
    "pentatonic major",
    "pentatonic minor",
    "whole tone",
];

/// Pitch-class names, 0 = C.
pub const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// A 400 ms time constant lets slides through while pulling held notes in.
pub const SNAP_MS_MAX: f32 = 400.0;
/// Thirty milliseconds corrects a syllable's arrival while retaining slides.
const SNAP_MS_DEFAULT: f32 = 30.0;
/// Fifty cents bounds the hold margin below a complete semitone.
pub const HYSTERESIS_MAX: f32 = 50.0;
/// Thirty cents exceeds typical held-note wavering without trapping a slide.
const HYSTERESIS_DEFAULT: f32 = 30.0;
/// Two 2.5 Hz poles leave about seven cents of a 6 Hz, fifty-cent vibrato.
pub const CENTRE_HZ: f32 = 2.5;
/// A 150-cent departure exceeds vibrato peaks and identifies a new note.
pub const NEW_NOTE_CENTS: f32 = 150.0;
/// Two consecutive departures reject an isolated peak without lagging a jump.
pub const NEW_NOTE_FRAMES: u8 = 2;

/// Snap volts to the nearest scale degree on a root in semitones above C.
///
/// Input is bounded to eight octaves either side of C4. The next octave's
/// root is a candidate; ordered degrees resolve ties to the lower note.
/// Non-finite pitch or root is treated as zero.
#[must_use]
pub fn nearest(volts: f32, scale: &[u8], root: f32) -> f32 {
    let root = crate::sanitize_sample(root);
    let semitones = crate::sanitize_sample(volts)
        .clamp(-8.0, 8.0)
        .mul_add(12.0, -root);
    let octave = (semitones / 12.0).floor();
    let within = octave.mul_add(-12.0, semitones);
    let mut best = 0.0_f32;
    let mut distance = f32::MAX;
    // Including the next root makes the octave boundary obey the same rule.
    for degree in scale
        .iter()
        .map(|&degree| f32::from(degree))
        .chain(std::iter::once(12.0))
    {
        let gap = (within - degree).abs();
        if gap < distance {
            distance = gap;
            best = degree;
        }
    }
    crate::sanitize_sample((octave.mul_add(12.0, best) + root) / 12.0)
}

/// Copyable controls for the callback's pitch correction stage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuantiseSettings {
    /// Scale index, held to 0..=9.
    pub scale: usize,
    /// Root in semitones above C, held to 0..=11.
    pub root: u8,
    /// Correction time constant in milliseconds; zero is instant.
    pub snap_ms: f32,
    /// Correct the filtered centre while leaving vibrato on top.
    pub keep_vibrato: bool,
    /// Distance advantage required before changing a held target.
    pub hysteresis_cents: f32,
    /// Correction amount, from zero (direct) to one (fully corrected).
    pub blend: f32,
}

impl Default for QuantiseSettings {
    fn default() -> Self {
        Self {
            scale: 0,
            root: 0,
            snap_ms: SNAP_MS_DEFAULT,
            keep_vibrato: true,
            hysteresis_cents: HYSTERESIS_DEFAULT,
            blend: 0.0,
        }
    }
}

impl QuantiseSettings {
    /// Clamp each control; a NaN uses that control's default.
    #[must_use]
    pub fn held(self) -> Self {
        Self {
            scale: self.scale.min(SCALES.len() - 1),
            root: self.root.min(11),
            snap_ms: held_float(self.snap_ms, SNAP_MS_MAX, SNAP_MS_DEFAULT),
            keep_vibrato: self.keep_vibrato,
            hysteresis_cents: held_float(self.hysteresis_cents, HYSTERESIS_MAX, HYSTERESIS_DEFAULT),
            blend: held_float(self.blend, 1.0, 0.0),
        }
    }
}

const fn held_float(value: f32, max: f32, default: f32) -> f32 {
    if value.is_nan() {
        default
    } else {
        value.clamp(0.0, max)
    }
}

/// Allocation-free pitch correction at the voice frame clock.
#[derive(Debug)]
pub struct Quantiser {
    frame_seconds: f32,
    settings: QuantiseSettings,
    centre_coefficient: f32,
    snap_coefficient: f32,
    first_pole: f32,
    centre: f32,
    target: Option<f32>,
    correction: f32,
    new_note_frames: u8,
    last_hz: f32,
    last_volts: f32,
}

impl Quantiser {
    /// Construct at the device rate; a zero rate is held to one sample/second.
    #[must_use]
    pub fn new(rate: u32) -> Self {
        let frame_seconds = VOICE_HOP as f32 / rate.max(1) as f32;
        let settings = QuantiseSettings::default();
        Self {
            frame_seconds,
            settings,
            centre_coefficient: (-TAU * CENTRE_HZ * frame_seconds).exp(),
            snap_coefficient: coefficient(frame_seconds, settings.snap_ms),
            first_pole: 0.0,
            centre: 0.0,
            target: None,
            correction: 0.0,
            new_note_frames: 0,
            last_hz: 0.0,
            last_volts: 0.0,
        }
    }

    /// Hold controls to their ranges without allocating or resetting the note.
    ///
    /// A new scale or root moves a held target to the new scale's nearest
    /// note at once, so hysteresis never keeps a note the scale lacks; the
    /// correction still glides there at the snap speed.
    pub fn set(&mut self, settings: QuantiseSettings) {
        let held = settings.held();
        let retune = held.scale != self.settings.scale || held.root != self.settings.root;
        self.settings = held;
        self.snap_coefficient = coefficient(self.frame_seconds, held.snap_ms);
        if retune && self.target.is_some() {
            self.target = Some(self.nearest_note());
        }
    }

    /// The held controls currently used by the callback.
    #[must_use]
    pub const fn settings(&self) -> QuantiseSettings {
        self.settings
    }

    /// Write snapped pitch, retaining the last output while unvoiced.
    ///
    /// An onset chooses its own nearest note after resetting the centre, and
    /// hysteresis only holds a note already sounding.
    /// Non-finite pitch resets state and writes finite direct values.
    /// With blend at zero every frame, voiced or not, carries the direct
    /// pitch bit for bit.
    pub fn process(&mut self, frame: &mut VoiceFrame) {
        if !frame.voiced {
            if self.settings.blend <= 0.0 {
                self.direct(frame);
            } else {
                frame.snapped_hz = self.last_hz;
                frame.snapped_volts = self.last_volts;
            }
            return;
        }
        let semitones = frame.volts * 12.0;
        if !semitones.is_finite() || !frame.hz.is_finite() {
            self.reset();
            self.direct(frame);
            return;
        }
        self.update_centre(semitones, frame.onset);
        let note = self.nearest_note();
        let target = match self.target {
            Some(target)
                if !frame.onset
                    && (self.centre - target).abs() - (self.centre - note).abs()
                        <= self.settings.hysteresis_cents / 100.0 =>
            {
                target
            }
            _ => note,
        };
        self.target = Some(target);
        let goal = target - self.centre;
        self.correction = if frame.onset {
            goal
        } else {
            self.snap_coefficient.mul_add(self.correction - goal, goal)
        };
        let snapped = self.settings.blend.mul_add(self.correction, semitones) / 12.0;
        if ![
            self.first_pole,
            self.centre,
            target,
            self.correction,
            snapped,
        ]
        .iter()
        .all(|value| value.is_finite())
        {
            self.reset();
            self.direct(frame);
            return;
        }
        if self.settings.blend <= 0.0 {
            // Copying avoids round-trip rounding and preserves signed zero.
            self.direct(frame);
        } else {
            self.last_volts = snapped;
            self.last_hz = hz_from_volts(snapped);
            frame.snapped_volts = self.last_volts;
            frame.snapped_hz = self.last_hz;
        }
    }

    /// The current scale's note nearest the centre, in semitones from C4.
    fn nearest_note(&self) -> f32 {
        12.0 * nearest(
            self.centre / 12.0,
            SCALES[self.settings.scale],
            f32::from(self.settings.root),
        )
    }

    fn update_centre(&mut self, semitones: f32, onset: bool) {
        if onset || self.target.is_none() || !self.settings.keep_vibrato {
            self.first_pole = semitones;
            self.centre = semitones;
            self.new_note_frames = 0;
            return;
        }
        if (semitones - self.centre).abs() > NEW_NOTE_CENTS / 100.0 {
            self.new_note_frames += 1;
        } else {
            self.new_note_frames = 0;
        }
        if self.new_note_frames >= NEW_NOTE_FRAMES {
            self.first_pole = semitones;
            self.centre = semitones;
            self.new_note_frames = 0;
        } else {
            self.first_pole = self
                .centre_coefficient
                .mul_add(self.first_pole - semitones, semitones);
            self.centre = self
                .centre_coefficient
                .mul_add(self.centre - self.first_pole, self.first_pole);
        }
    }

    fn direct(&mut self, frame: &mut VoiceFrame) {
        self.last_volts = crate::sanitize_sample(frame.volts);
        self.last_hz = if frame.hz.is_finite() {
            frame.hz
        } else {
            hz_from_volts(self.last_volts)
        };
        frame.snapped_volts = self.last_volts;
        frame.snapped_hz = self.last_hz;
    }

    /// Clear note history and held output, retaining the rate and controls.
    pub const fn reset(&mut self) {
        self.first_pole = 0.0;
        self.centre = 0.0;
        self.target = None;
        self.correction = 0.0;
        self.new_note_frames = 0;
        self.last_hz = 0.0;
        self.last_volts = 0.0;
    }
}

fn coefficient(frame_seconds: f32, snap_ms: f32) -> f32 {
    if snap_ms <= 0.0 {
        0.0
    } else {
        (-frame_seconds / (snap_ms / 1000.0)).exp()
    }
}

#[cfg(test)]
mod tests;
