//! Knob values, glides and times as people read them: Hz or kHz, ms or s,
//! semitones, beats, and the names of divisions, scales and notes.

use crate::catalogue::{KnobSpec, Unit};

/// A knob's value as people read it.
///
/// That is its label when the knob names its positions (`1/8`, `dorian`,
/// `saw`, or `2.5 (saw→square)` between two named points), else the number
/// with its unit (`420 Hz`, `1.2 kHz`, `350 ms`, `+3 st`, `-6 dB`).
#[must_use]
pub fn knob_value(spec: &KnobSpec, value: f32) -> String {
    let value = spec.clamp(value);
    if !spec.labels.is_empty() {
        return labelled(spec, value);
    }
    match spec.unit {
        Unit::None => number(value),
        Unit::Hz => hertz(value),
        Unit::Seconds => seconds(value),
        Unit::Semitones => format!("{} st", signed(value)),
        Unit::Octaves => format!("{} oct", signed(value)),
        Unit::Steps => format!("{} steps", value.round()),
        Unit::Other("%") => format!("{}%", number(value)),
        Unit::Other(unit) => format!("{} {unit}", number(value)),
    }
}

/// A value on a knob with named positions.
fn labelled(spec: &KnobSpec, value: f32) -> String {
    let offset = value - spec.min;
    let name = |position: f32| {
        // Rounded and not negative: the cast is exact.
        (position >= 0.0)
            .then(|| spec.labels.get(position.round() as usize))
            .flatten()
            .map(|name| (*name).to_string())
    };
    let nearest = offset.round();
    if spec.stepped || (offset - nearest).abs() < 0.05 {
        return name(nearest).unwrap_or_else(|| number(value));
    }
    let low = offset.floor();
    match (name(low), name(low + 1.0)) {
        (Some(from), Some(to)) => format!("{} ({from}→{to})", number(value)),
        _ => number(value),
    }
}

/// A frequency: `420 Hz`, `42.5 Hz`, `0.25 Hz`, `1.2 kHz`, `18 kHz`.
#[must_use]
pub fn hertz(hz: f32) -> String {
    if hz.abs() >= 1_000.0 {
        format!("{} kHz", trim(f64::from(hz) / 1_000.0, 2))
    } else if hz.abs() >= 100.0 {
        format!("{} Hz", trim(f64::from(hz), 0))
    } else if hz.abs() >= 10.0 {
        format!("{} Hz", trim(f64::from(hz), 1))
    } else {
        format!("{} Hz", trim(f64::from(hz), 2))
    }
}

/// A time: `0 ms`, `2.5 ms`, `350 ms`, `1.5 s`.
#[must_use]
pub fn seconds(seconds: f32) -> String {
    let seconds = f64::from(seconds);
    if seconds.abs() >= 1.0 {
        format!("{} s", trim(seconds, 2))
    } else if seconds.abs() >= 0.01 {
        format!("{} ms", trim(seconds * 1_000.0, 0))
    } else {
        format!("{} ms", trim(seconds * 1_000.0, 1))
    }
}

/// A running time on a clock face, in whole seconds: `0:07`, `3:20`,
/// `1:02:03`. Not a number, or below zero, reads as `0:00`.
#[must_use]
pub fn clock(seconds: f64) -> String {
    // Held to what a u64 of seconds holds; a recording is hours at most.
    let whole = if seconds.is_finite() {
        seconds.max(0.0).floor().min(u64::MAX as f64) as u64
    } else {
        0
    };
    let (hours, minutes, seconds) = (whole / 3_600, whole % 3_600 / 60, whole % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// A length in beats: `1 beat`, `0.5 beats`, `4 beats`.
#[must_use]
pub fn beats(beats: f64) -> String {
    let text = trim(beats, 2);
    if text == "1" {
        "1 beat".to_string()
    } else {
        format!("{text} beats")
    }
}

/// A tempo: `96 BPM`, `92.5 BPM`.
#[must_use]
pub fn bpm(bpm: f64) -> String {
    format!("{} BPM", trim(bpm, 1))
}

/// A plain number with two decimals at most: `0.4`, `-1`, `0.25`.
#[must_use]
pub fn number(value: f32) -> String {
    trim(f64::from(value), 2)
}

/// A number with a sign: `+3`, `-12.5`, `0`.
fn signed(value: f32) -> String {
    let text = trim(f64::from(value), 2);
    if value > 0.0 && text != "0" {
        format!("+{text}")
    } else {
        text
    }
}

/// `value` with at most `places` decimals, trailing zeros removed, and no
/// negative zero.
fn trim(value: f64, places: usize) -> String {
    let text = format!("{value:.places$}");
    let text = if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        text
    };
    if text == "-0" { "0".to_string() } else { text }
}

/// A pitch as a note name with octave: `A2`, `C#4`.
#[must_use]
pub fn note_name(hz: f32) -> Option<String> {
    if !(hz.is_finite() && hz > 0.0) {
        return None;
    }
    let midi = 12.0f32.mul_add((hz / 440.0).log2(), 69.0).round();
    if !(0.0..=127.0).contains(&midi) {
        return None;
    }
    // In 0..=127 and rounded: the cast is exact.
    let midi = midi as i32;
    let octave = midi / 12 - 1;
    Some(format!(
        "{}{octave}",
        crate::catalogue::NOTE_NAMES[(midi % 12) as usize]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::catalogue::Kind;

    fn show(kind: Kind, name: &str, value: f32) -> String {
        let spec = kind.spec();
        knob_value(&spec.knobs[spec.knob_index(name).unwrap()], value)
    }

    #[test]
    fn frequencies_read_in_hz_and_khz() {
        assert_eq!(hertz(420.0), "420 Hz");
        assert_eq!(hertz(42.5), "42.5 Hz");
        assert_eq!(hertz(0.25), "0.25 Hz");
        assert_eq!(hertz(1_200.0), "1.2 kHz");
        assert_eq!(hertz(18_000.0), "18 kHz");
        assert_eq!(hertz(1_250.0), "1.25 kHz");
    }

    #[test]
    fn times_read_in_ms_and_s() {
        assert_eq!(seconds(0.0), "0 ms");
        assert_eq!(seconds(0.0025), "2.5 ms");
        assert_eq!(seconds(0.35), "350 ms");
        assert_eq!(seconds(1.5), "1.5 s");
        assert_eq!(seconds(20.0), "20 s");
    }

    #[test]
    fn running_times_read_on_a_clock_face() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(7.9), "0:07");
        assert_eq!(clock(200.5), "3:20");
        assert_eq!(clock(3_723.0), "1:02:03");
        assert_eq!(clock(-4.0), "0:00");
        assert_eq!(clock(f64::NAN), "0:00");
    }

    #[test]
    fn beats_and_tempo_read_plainly() {
        assert_eq!(beats(1.0), "1 beat");
        assert_eq!(beats(4.0), "4 beats");
        assert_eq!(beats(0.5), "0.5 beats");
        assert_eq!(bpm(96.0), "96 BPM");
        assert_eq!(bpm(92.5), "92.5 BPM");
    }

    #[test]
    fn knobs_read_with_their_units() {
        assert_eq!(show(Kind::VCF, "cutoff", 800.0), "800 Hz");
        assert_eq!(show(Kind::VCO, "tune", 3.0), "+3 st");
        assert_eq!(show(Kind::VCO, "tune", -0.0), "0 st");
        assert_eq!(show(Kind::VCO, "octave", -2.0), "-2 oct");
        assert_eq!(show(Kind::VCO, "shape", 2.0), "saw");
        assert_eq!(show(Kind::VCO, "shape", 2.5), "2.5 (saw→square)");
        assert_eq!(show(Kind::CLOCK, "division", 1.0), "1/8");
        assert_eq!(show(Kind::LFO, "sync", 0.0), "free");
        assert_eq!(show(Kind::LFO, "sync", 5.0), "1 bar");
        assert_eq!(show(Kind::QUANT, "scale", 3.0), "dorian");
        assert_eq!(show(Kind::QUANT, "root", 9.0), "A");
        assert_eq!(show(Kind::SEQ, "steps", 8.0), "8 steps");
        assert_eq!(show(Kind::ENV, "attack", 0.375), "375 ms");
        assert_eq!(show(Kind::VCA, "gain", 0.4), "0.4");
        let effect = Kind::from_name("testgain").unwrap();
        assert_eq!(show(effect, "gain", -6.0), "-6 dB");
        assert_eq!(show(effect, "mode", 1.0), "hard");
    }

    #[test]
    fn notes_have_names_and_octaves() {
        assert_eq!(note_name(110.0).as_deref(), Some("A2"));
        assert_eq!(note_name(261.63).as_deref(), Some("C4"));
        assert_eq!(note_name(0.0), None);
        assert_eq!(note_name(f32::NAN), None);
    }
}
