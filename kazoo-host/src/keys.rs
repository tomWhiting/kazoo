//! Shared typing-keyboard piano and OS repeat-delay probe.

use std::time::Duration;

/// Default delay when the OS repeat setting cannot be read.
pub const DEFAULT_REPEAT_DELAY: Duration = Duration::from_millis(250);
/// Move the piano one octave down.
pub const OCTAVE_DOWN: char = 'z';
/// Move the piano one octave up.
pub const OCTAVE_UP: char = 'x';

/// The MIDI octave of the `a` key, held to 0 through 8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Octave(i8);

impl Octave {
    /// Lowest octave: C0 is MIDI 12.
    pub const MIN: Self = Self(0);
    /// Highest octave: C8 is MIDI 108.
    pub const MAX: Self = Self(8);
    /// Default octave: C3 is MIDI 48.
    pub const DEFAULT: Self = Self(3);

    /// Construct an octave held to 0 through 8.
    #[must_use]
    pub const fn new(number: i8) -> Self {
        if number < 0 {
            Self::MIN
        } else if number > 8 {
            Self::MAX
        } else {
            Self(number)
        }
    }

    /// The MIDI octave number.
    #[must_use]
    pub const fn number(self) -> i8 {
        self.0
    }

    /// Move down one octave, stopping at the minimum.
    #[must_use]
    pub const fn down(self) -> Self {
        Self::new(self.0 - 1)
    }

    /// Move up one octave, stopping at the maximum.
    #[must_use]
    pub const fn up(self) -> Self {
        Self::new(self.0 + 1)
    }
}

impl Default for Octave {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// The MIDI note played by a piano key, ignoring ASCII case.
#[must_use]
pub const fn note(key: char, octave: Octave) -> Option<u8> {
    match semitone(key) {
        Some(offset) => Some((octave.number() as u8 + 1) * 12 + offset),
        None => None,
    }
}

/// What a character does for the typing-keyboard piano.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PianoKey {
    /// Play a MIDI note.
    Note(u8),
    /// Release held notes and move down one octave.
    OctaveDown,
    /// Release held notes and move up one octave.
    OctaveUp,
}

/// Classify a piano character before handling instrument commands.
#[must_use]
pub const fn piano_key(key: char, octave: Octave) -> Option<PianoKey> {
    match key.to_ascii_lowercase() {
        OCTAVE_DOWN => Some(PianoKey::OctaveDown),
        OCTAVE_UP => Some(PianoKey::OctaveUp),
        ch => match note(ch, octave) {
            Some(note) => Some(PianoKey::Note(note)),
            None => None,
        },
    }
}

/// The OS key-repeat delay: the macOS `InitialKeyRepeat` setting. The error
/// says why it could not be read, for the status line.
pub fn system_repeat_delay() -> Result<Duration, String> {
    if !cfg!(target_os = "macos") {
        return Err("not readable on this OS".to_owned());
    }
    let out = std::process::Command::new("defaults")
        .args(["read", "-g", "InitialKeyRepeat"])
        .output()
        .map_err(|e| format!("could not run `defaults`: {e}"))?;
    if !out.status.success() {
        // An unset key is the normal case on a fresh system.
        return Err("InitialKeyRepeat is not set".to_owned());
    }
    let text = String::from_utf8(out.stdout)
        .map_err(|_| "InitialKeyRepeat is not valid text".to_owned())?;
    parse_initial_key_repeat(&text)
}

/// Parse `InitialKeyRepeat` (units of 15 ms), clamped to a sane range.
pub fn parse_initial_key_repeat(text: &str) -> Result<Duration, String> {
    let ticks = text
        .trim()
        .parse::<u64>()
        .map_err(|e| format!("InitialKeyRepeat {:?} is not a number: {e}", text.trim()))?;
    Ok(Duration::from_millis(ticks.saturating_mul(15))
        .clamp(Duration::from_millis(100), Duration::from_secs(2)))
}

/// Tracker-style piano: the home row is white keys, the row above is black keys.
#[must_use]
pub const fn semitone(key: char) -> Option<u8> {
    Some(match key.to_ascii_lowercase() {
        'a' => 0,
        'w' => 1,
        's' => 2,
        'e' => 3,
        'd' => 4,
        'f' => 5,
        't' => 6,
        'g' => 7,
        'y' => 8,
        'h' => 9,
        'u' => 10,
        'j' => 11,
        'k' => 12,
        'o' => 13,
        'l' => 14,
        'p' => 15,
        ';' => 16,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const PIANO: &str = "awsedftgyhujkolp;";

    #[test]
    fn the_home_row_is_a_chromatic_octave_and_a_third() {
        for (offset, key) in PIANO.chars().enumerate() {
            assert_eq!(semitone(key), Some(offset as u8));
        }
        assert_eq!(semitone('A'), Some(0));
        for key in ['q', 'z', ' ', '1'] {
            assert_eq!(semitone(key), None);
        }
    }

    #[test]
    fn the_default_octave_puts_a_on_c3() {
        assert_eq!(note('a', Octave::DEFAULT), Some(48));
        assert_eq!(note(';', Octave::DEFAULT), Some(64));
    }

    #[test]
    fn octaves_stop_at_the_ends_and_every_key_stays_a_midi_note() {
        assert_eq!(Octave::MIN.down(), Octave::MIN);
        assert_eq!(Octave::MAX.up(), Octave::MAX);
        assert_eq!(Octave::new(9), Octave::MAX);
        assert_eq!(Octave::new(-1), Octave::MIN);
        for number in 0..=8 {
            for key in PIANO.chars() {
                assert!(matches!(note(key, Octave::new(number)), Some(0..=127)));
            }
        }
        assert_eq!(note('a', Octave::MAX), Some(108));
    }

    #[test]
    fn z_and_x_move_the_octave_and_are_never_notes() {
        assert_eq!(piano_key('z', Octave::DEFAULT), Some(PianoKey::OctaveDown));
        assert_eq!(piano_key('x', Octave::DEFAULT), Some(PianoKey::OctaveUp));
        assert_eq!(piano_key('a', Octave::DEFAULT), Some(PianoKey::Note(48)));
        assert_eq!(piano_key('q', Octave::DEFAULT), None);
    }

    #[test]
    fn key_repeat_setting_parses() {
        assert_eq!(
            parse_initial_key_repeat("25"),
            Ok(Duration::from_millis(375))
        );
        assert_eq!(
            parse_initial_key_repeat("1"),
            Ok(Duration::from_millis(100))
        );
        assert_eq!(parse_initial_key_repeat("1000"), Ok(Duration::from_secs(2)));
        assert!(matches!(parse_initial_key_repeat("abc"), Err(why) if why.contains("abc")));
    }
}
