//! Keyboard-to-note mapping, aftertouch simulation, and mouse zones.
//!
//! Maps QWERTY keyboard to chromatic notes for playing the synth.

/// MIDI note for a given keyboard character.
///
/// Layout (bottom row = lower octave, top row = upper octave):
/// ```text
///  2 3   5 6 7   9 0
/// Q W E R T Y U I O P
///  S D   G H J   L ;
/// Z X C V B N M , . /
/// ```
#[must_use]
pub fn key_to_note(ch: char, octave: i8) -> Option<u8> {
    let semitone = match ch {
        // Lower octave (bottom two rows)
        'z' | 'Z' => Some(0),  // C
        's' | 'S' => Some(1),  // C#
        'x' | 'X' => Some(2),  // D
        'd' | 'D' => Some(3),  // D#
        'c' | 'C' => Some(4),  // E
        'v' | 'V' => Some(5),  // F
        'g' | 'G' => Some(6),  // F#
        'b' | 'B' => Some(7),  // G
        'h' | 'H' => Some(8),  // G#
        'n' | 'N' => Some(9),  // A
        'j' | 'J' => Some(10), // A#
        'm' | 'M' => Some(11), // B

        // Upper octave (top two rows)
        'q' | 'Q' => Some(12), // C
        '2' => Some(13),       // C#
        'w' | 'W' => Some(14), // D
        '3' => Some(15),       // D#
        'e' | 'E' => Some(16), // E
        'r' | 'R' => Some(17), // F
        '5' => Some(18),       // F#
        't' | 'T' => Some(19), // G
        '6' => Some(20),       // G#
        'y' | 'Y' => Some(21), // A
        '7' => Some(22),       // A#
        'u' | 'U' => Some(23), // B
        'i' | 'I' => Some(24), // C (next octave)
        '9' => Some(25),       // C#
        'o' | 'O' => Some(26), // D
        '0' => Some(27),       // D#
        'p' | 'P' => Some(28), // E

        _ => None,
    };

    semitone.and_then(|s| {
        let note = i16::from(octave) * 12 + 48 + s; // octave 0 = middle C area
        if (0..=127).contains(&note) {
            Some(note as u8)
        } else {
            None
        }
    })
}

/// Default velocity for keyboard-triggered notes.
pub const DEFAULT_VELOCITY: f32 = 0.8;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_mapping_middle_c() {
        // 'z' at octave 0 should map to note 48 (C3)
        let note = key_to_note('z', 0);
        assert_eq!(note, Some(48));
    }

    #[test]
    fn key_mapping_upper_octave() {
        // 'q' at octave 0 should be C4 (60)
        let note = key_to_note('q', 0);
        assert_eq!(note, Some(60));
    }

    #[test]
    fn key_mapping_octave_shift() {
        // 'z' at octave 1 should be C4 (60)
        let note = key_to_note('z', 1);
        assert_eq!(note, Some(60));
    }

    #[test]
    fn key_mapping_unknown_char() {
        assert_eq!(key_to_note('!', 0), None);
        assert_eq!(key_to_note(' ', 0), None);
    }

    #[test]
    fn key_mapping_out_of_range() {
        // Very high octave should return None
        assert_eq!(key_to_note('p', 10), None);
    }
}
