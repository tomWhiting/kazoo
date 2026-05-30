//! Small pipe-friendly musical notation primitives.
//!
//! This is intentionally not a full music language. It is a compact textual
//! event format that humans and agents can type, pipe, or generate without a GUI:
//!
//! ```text
//! c4/4 d4/8 eb4/8 [g4 bb4 d5]/2 r/4
//! ```
//!
//! Tokens are separated by whitespace. A token is either a note, a rest (`r`), or
//! a chord in square brackets. Durations are written after `/` as note values:
//! `1` whole, `2` half, `4` quarter, `8` eighth, etc. Dotted durations append
//! `.`. Notes are MIDI-style pitch spellings with octave numbers; middle C is
//! `c4` / MIDI note 60.

use crate::protocol::{NoteEvent, NoteEventKind};

/// Default velocity used by [`parse_note_events`].
pub const DEFAULT_NOTATION_VELOCITY: f32 = 0.8;

/// Parsed musical event from the text notation.
#[derive(Debug, Clone, PartialEq)]
pub struct NotationEvent {
    /// Start time in quarter-note beats from the beginning of the phrase.
    pub start_beats: f64,
    /// Duration in quarter-note beats.
    pub duration_beats: f64,
    /// MIDI note numbers. Empty means a rest.
    pub notes: Vec<u8>,
}

impl NotationEvent {
    /// Whether this event is a rest.
    #[must_use]
    pub fn is_rest(&self) -> bool {
        self.notes.is_empty()
    }
}

/// Error returned by the text notation parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotationError {
    /// Token index where parsing failed.
    pub token_index: usize,
    /// Human-readable diagnostic.
    pub message: String,
}

/// Parse pipe-friendly text notation into beat-timed events.
pub fn parse_notation(input: &str) -> Result<Vec<NotationEvent>, NotationError> {
    let mut events = Vec::new();
    let mut cursor_beats = 0.0_f64;

    for (token_index, token) in tokenize(input).into_iter().enumerate() {
        let (body, duration_beats) = split_duration(&token).map_err(|message| NotationError {
            token_index,
            message,
        })?;
        let notes = parse_body(body).map_err(|message| NotationError {
            token_index,
            message,
        })?;

        events.push(NotationEvent {
            start_beats: cursor_beats,
            duration_beats,
            notes,
        });
        cursor_beats += duration_beats;
    }

    Ok(events)
}

/// Parse text notation directly into timestamped note on/off events.
///
/// `bpm` and `sample_rate` convert beat positions to absolute frame times.
pub fn parse_note_events(
    input: &str,
    bpm: f64,
    sample_rate: u32,
    channel: u8,
) -> Result<Vec<NoteEvent>, NotationError> {
    let events = parse_notation(input)?;
    let frames_per_beat = frames_per_beat(bpm, sample_rate);
    let mut out = Vec::new();

    for event in events {
        if event.is_rest() {
            continue;
        }
        let start_frame = beats_to_frame(event.start_beats, frames_per_beat);
        let end_frame = beats_to_frame(event.start_beats + event.duration_beats, frames_per_beat);
        for note in event.notes {
            out.push(NoteEvent::note_on(
                start_frame,
                channel,
                note,
                DEFAULT_NOTATION_VELOCITY,
            ));
            out.push(NoteEvent {
                frame: end_frame,
                source: None,
                destination: None,
                channel: channel.min(15),
                kind: NoteEventKind::NoteOff {
                    note,
                    velocity: 0.0,
                },
            });
        }
    }

    out.sort_by_key(|event| event.frame);
    Ok(out)
}

fn tokenize(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_chord = false;

    for ch in input.chars() {
        match ch {
            '[' => {
                in_chord = true;
                current.push(ch);
            }
            ']' => {
                in_chord = false;
                current.push(ch);
            }
            ch if ch.is_whitespace() && !in_chord => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

fn split_duration(token: &str) -> Result<(&str, f64), String> {
    let Some((body, duration)) = token.rsplit_once('/') else {
        return Err("missing duration like /4 or /8".to_string());
    };
    if body.is_empty() {
        return Err("missing note, chord, or rest before duration".to_string());
    }

    let dotted = duration.ends_with('.');
    let denominator = duration
        .trim_end_matches('.')
        .parse::<u32>()
        .map_err(|_| "duration denominator must be a positive integer".to_string())?;
    if denominator == 0 {
        return Err("duration denominator must be non-zero".to_string());
    }

    let mut beats = 4.0 / f64::from(denominator);
    if dotted {
        beats *= 1.5;
    }
    Ok((body, beats))
}

fn parse_body(body: &str) -> Result<Vec<u8>, String> {
    if body.eq_ignore_ascii_case("r") || body.eq_ignore_ascii_case("rest") {
        return Ok(Vec::new());
    }

    if let Some(chord) = body
        .strip_prefix('[')
        .and_then(|body| body.strip_suffix(']'))
    {
        let mut notes = Vec::new();
        for part in chord.split_whitespace() {
            notes.push(parse_note(part)?);
        }
        if notes.is_empty() {
            return Err("chord must contain at least one note".to_string());
        }
        return Ok(notes);
    }

    Ok(vec![parse_note(body)?])
}

fn parse_note(note: &str) -> Result<u8, String> {
    let mut chars = note.chars().peekable();
    let Some(letter) = chars.next() else {
        return Err("empty note".to_string());
    };

    let pitch_class = match letter.to_ascii_lowercase() {
        'c' => 0,
        'd' => 2,
        'e' => 4,
        'f' => 5,
        'g' => 7,
        'a' => 9,
        'b' => 11,
        _ => return Err(format!("unknown note letter: {letter}")),
    };

    let accidental = match chars.peek().copied() {
        Some('#') => {
            chars.next();
            1
        }
        Some('b') | Some('♭') => {
            chars.next();
            -1
        }
        _ => 0,
    };

    let octave_text: String = chars.collect();
    let octave = octave_text
        .parse::<i16>()
        .map_err(|_| "note must include an octave, e.g. c4".to_string())?;
    let midi = (octave + 1) * 12 + pitch_class + accidental;
    if !(0..=127).contains(&midi) {
        return Err("note is outside MIDI range 0..127".to_string());
    }
    Ok(midi as u8)
}

fn frames_per_beat(bpm: f64, sample_rate: u32) -> f64 {
    let bpm = if bpm.is_finite() && bpm > 0.0 {
        bpm
    } else {
        120.0
    };
    60.0 / bpm * f64::from(sample_rate.max(1))
}

fn beats_to_frame(beats: f64, frames_per_beat: f64) -> u64 {
    (beats * frames_per_beat).round().max(0.0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_notes_rests_and_chords() {
        let events = parse_notation("c4/4 d4/8 r/8 [e4 g4 b4]/2").unwrap();

        assert_eq!(events.len(), 4);
        assert_eq!(events[0].notes, vec![60]);
        assert_eq!(events[1].start_beats, 1.0);
        assert!(events[2].is_rest());
        assert_eq!(events[3].notes, vec![64, 67, 71]);
        assert_eq!(events[3].duration_beats, 2.0);
    }

    #[test]
    fn supports_accidentals_and_dotted_durations() {
        let events = parse_notation("c#4/8. eb4/8").unwrap();

        assert_eq!(events[0].notes, vec![61]);
        assert_eq!(events[1].notes, vec![63]);
        assert_eq!(events[0].duration_beats, 0.75);
        assert_eq!(events[1].start_beats, 0.75);
    }

    #[test]
    fn converts_to_timestamped_note_events() {
        let events = parse_note_events("c4/4 [e4 g4]/4", 120.0, 48_000, 20).unwrap();

        assert_eq!(events.len(), 6);
        assert_eq!(events[0].frame, 0);
        assert_eq!(events[1].frame, 24_000);
        assert_eq!(events[0].channel, 15);
        assert_eq!(events[2].frame, 24_000);
    }

    #[test]
    fn reports_token_index_for_bad_note() {
        let err = parse_notation("c4/4 nope/4").unwrap_err();

        assert_eq!(err.token_index, 1);
        assert!(err.message.contains("unknown note"));
    }
}
