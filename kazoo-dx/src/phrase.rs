//! Looping `kazoo-play` notation phrase, driven sample-accurately from the
//! audio callback.
//!
//! Parsing and allocation happen once in [`Phrase::parse`] on the UI thread.
//! [`Phrase::advance`] is called once per sample and never allocates.

use kazoo_core::notation::{NotationError, parse_notation, parse_note_events};
use kazoo_core::protocol::NoteEventKind;

use crate::synth::{FmSynth, Source};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Step {
    frame: u64,
    note: u8,
    /// MIDI velocity for note-on; `None` for note-off.
    velocity: Option<u8>,
}

/// A parsed phrase that loops forever.
#[derive(Debug)]
pub struct Phrase {
    steps: Vec<Step>,
    length: u64,
    /// Playhead in frames at the phrase's own tempo. Fractional so the hub
    /// tempo can speed it up or slow it down without re-parsing.
    position: f64,
    cursor: usize,
    /// Tempo the frame positions were computed at.
    base_bpm: f64,
    /// Frames of phrase time advanced per output frame.
    rate: f64,
}

impl Phrase {
    /// Parse notation at a tempo and sample rate.
    pub fn parse(text: &str, bpm: f64, sample_rate: u32) -> Result<Self, NotationError> {
        let events = parse_note_events(text, bpm, sample_rate, 0)?;
        let steps: Vec<Step> = events
            .iter()
            .filter_map(|event| match event.kind {
                NoteEventKind::NoteOn { note, velocity } => Some(Step {
                    frame: event.frame,
                    note,
                    velocity: Some(velocity_to_midi(velocity)),
                }),
                NoteEventKind::NoteOff { note, .. } => Some(Step {
                    frame: event.frame,
                    note,
                    velocity: None,
                }),
                #[allow(unreachable_patterns)]
                _ => None,
            })
            .collect();

        // Loop length comes from the notation itself, so trailing rests count.
        let total_beats = parse_notation(text)?
            .iter()
            .map(|e| e.start_beats + e.duration_beats)
            .fold(0.0_f64, f64::max);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let from_beats = (total_beats * 60.0 / bpm * f64::from(sample_rate)).ceil() as u64;
        let last_event = steps.last().map_or(0, |s| s.frame);
        // Never zero, so an all-rest phrase cannot spin.
        let length = from_beats.max(last_event + 1).max(1);

        Ok(Self {
            steps,
            length,
            position: 0.0,
            cursor: 0,
            base_bpm: bpm,
            rate: 1.0,
        })
    }

    /// Follow a new tempo (for example from the hub transport). Non-finite or
    /// out-of-range tempos are ignored.
    pub fn set_bpm(&mut self, bpm: f64) {
        if bpm.is_finite() && (20.0..=400.0).contains(&bpm) && self.base_bpm > 0.0 {
            self.rate = bpm / self.base_bpm;
        }
    }

    /// Restart from the top.
    pub const fn rewind(&mut self) {
        self.position = 0.0;
        self.cursor = 0;
    }

    /// Fire every event due at the current frame, then move one frame on.
    pub fn advance(&mut self, synth: &mut FmSynth) {
        while let Some(step) = self.steps.get(self.cursor) {
            #[allow(clippy::cast_precision_loss)]
            if step.frame as f64 > self.position {
                break;
            }
            match step.velocity {
                Some(velocity) => synth.note_on_from(Source::Phrase, step.note, velocity),
                None => synth.note_off_from(Source::Phrase, step.note),
            }
            self.cursor += 1;
        }
        self.position += self.rate;
        #[allow(clippy::cast_precision_loss)]
        let length = self.length as f64;
        if self.position >= length {
            // Keep the overshoot so the loop stays locked to the hub tempo, and
            // release only the phrase's own voices so loops never pile up.
            self.position = (self.position - length).clamp(0.0, length);
            self.cursor = 0;
            synth.release_source(Source::Phrase);
        }
    }
}

fn velocity_to_midi(velocity: f32) -> u8 {
    if velocity.is_finite() {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let v = (velocity.clamp(0.0, 1.0) * 127.0).round() as u8;
        v.max(1)
    } else {
        100
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loops_and_makes_sound() {
        let mut phrase = Phrase::parse("c4/8 e4/8 g4/4", 120.0, 48_000).expect("valid notation");
        assert!(phrase.length > 0);
        let mut synth = FmSynth::new(48_000.0);
        let mut energy = 0.0;
        for _ in 0..phrase.length * 2 + 10 {
            phrase.advance(&mut synth);
            energy += synth.process().abs();
        }
        assert!(energy > 1.0);
        #[allow(clippy::cast_precision_loss)]
        let length = phrase.length as f64;
        assert!(phrase.position < length);
    }

    #[test]
    fn tempo_changes_loop_speed() {
        let run = |bpm: f64| {
            let mut phrase = Phrase::parse("c4/4 e4/4", 120.0, 48_000).expect("valid");
            phrase.set_bpm(bpm);
            let mut synth = FmSynth::new(48_000.0);
            let mut loops = 0;
            for _ in 0..200_000 {
                phrase.advance(&mut synth);
                if phrase.position < phrase.rate {
                    loops += 1;
                }
            }
            loops
        };
        assert!(run(240.0) > run(120.0));
    }

    #[test]
    fn wrap_keeps_fractional_overshoot() {
        let mut phrase = Phrase::parse("c4/4", 120.0, 48_000).expect("valid");
        phrase.set_bpm(173.0);
        let mut synth = FmSynth::new(48_000.0);
        let mut previous = 0.0;
        for _ in 0..100_000 {
            phrase.advance(&mut synth);
            if phrase.position < previous {
                // Just wrapped: position is the overshoot, not zero.
                assert!(phrase.position > 0.0 && phrase.position < phrase.rate);
                return;
            }
            previous = phrase.position;
        }
        panic!("phrase never wrapped");
    }

    #[test]
    fn hostile_tempo_is_ignored() {
        let mut phrase = Phrase::parse("c4/4", 120.0, 48_000).expect("valid");
        phrase.set_bpm(f64::NAN);
        phrase.set_bpm(0.0);
        phrase.set_bpm(10_000.0);
        assert!((phrase.rate - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn rests_extend_the_loop() {
        let short = Phrase::parse("c4/4", 120.0, 48_000).expect("valid");
        let long = Phrase::parse("c4/4 r/4 r/4", 120.0, 48_000).expect("valid");
        assert!(long.length > short.length);
    }

    #[test]
    fn bad_notation_is_an_error() {
        assert!(Phrase::parse("not-a-note!!", 120.0, 48_000).is_err());
    }

    #[test]
    fn velocity_mapping_is_safe() {
        assert_eq!(velocity_to_midi(f32::NAN), 100);
        assert_eq!(velocity_to_midi(0.0), 1);
        assert_eq!(velocity_to_midi(1.0), 127);
    }
}
