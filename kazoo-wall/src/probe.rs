//! What the wall's scopes and readouts measured, as the protocol carries
//! it.
//!
//! The engine's captures (see [`crate::dsp::measure`]) become [`Probe`]s,
//! with a readout's face and a scope's picture in plain words.
//!
//! A readout shows its input as pitch, as a plain CV, as a gate or as
//! audio. Its `mode` knob picks one, or at `auto` the wall picks by what
//! is plugged in: the output the cable comes from (an output that carries
//! pitch, as the catalogue marks it, is shown as pitch; any other CV as a
//! CV; a gate as a gate; audio as audio), followed back through any scope
//! or readout it passes through on the way. The patch says what a signal
//! is for, so the choice never flickers as the signal moves (an LFO that
//! speeds up into audio rates stays a CV); the knob overrides it.
//!
//! Every number that leaves here is finite (JSON has no other kind), and
//! a readout's every line fits [`READOUT_COLUMNS`].

use std::time::Duration;

use crate::catalogue::{
    C4_HZ, CLOCK_DIVISIONS, CLOCK_LABELS, Kind, NOTE_NAMES, PortSpec, READOUT_MODES, SCOPE_MODES,
    Signal,
};
use crate::dsp::CV_LIMIT;
use crate::dsp::ScopeMode;
use crate::dsp::measure::{Capture, Reading};
use crate::format;
use crate::listen::dbfs;
use crate::protocol::{GateMeasured, Measured, Probe, ReadoutReading, Rhythm, ScopeReading, widen};

/// The widest a readout's line is, in characters.
pub const READOUT_COLUMNS: usize = 22;

/// Whether modules of `kind` measure: scopes and readouts.
#[must_use]
pub fn measures(kind: Kind) -> bool {
    kind == Kind::SCOPE || kind == Kind::READOUT
}

/// How a readout shows its input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Show {
    /// As pitch, 1.0 per octave with 0 = C4: a note, cents and Hz.
    Pitch,
    /// As a plain CV: its value, how often it cycles, and its range.
    Cv,
    /// As a gate: its rate and duty.
    Gate,
    /// As audio: peak, RMS, DC and frequency.
    Audio,
}

impl Show {
    /// Its name, as the readout's `mode` knob names it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Pitch => READOUT_MODES[1],
            Self::Cv => READOUT_MODES[2],
            Self::Gate => READOUT_MODES[3],
            Self::Audio => READOUT_MODES[4],
        }
    }

    /// How what output `port` carries is shown.
    #[must_use]
    pub const fn of(port: &PortSpec) -> Self {
        match port.signal {
            Signal::Cv if port.pitch => Self::Pitch,
            Signal::Cv => Self::Cv,
            Signal::Gate => Self::Gate,
            Signal::Audio => Self::Audio,
        }
    }

    /// What the readout's `mode` knob at `knob` asks for: `None` at
    /// `auto`.
    #[must_use]
    pub fn chosen(knob: f32) -> Option<Self> {
        match knob.round() {
            v if v >= 4.0 => Some(Self::Audio),
            v if v >= 3.0 => Some(Self::Gate),
            v if v >= 2.0 => Some(Self::Cv),
            v if v >= 1.0 => Some(Self::Pitch),
            _ => None,
        }
    }
}

/// The probe of module `id`, of `kind`.
///
/// It is made from its latest capture and how long ago that was (none
/// yet: nothing measured), with how what is plugged into a readout is
/// shown (`source`, see the module's notes) and the wall's tempo.
#[must_use]
pub fn probe(
    id: &str,
    kind: Kind,
    latest: Option<(&Capture, Duration)>,
    source: Option<Show>,
    bpm: f64,
) -> Probe {
    let age = latest.map_or(0.0, |(_, age)| round(age.as_secs_f64(), 3));
    let capture = latest.map(|(capture, _)| capture);
    Probe {
        module: id.to_string(),
        kind: kind.name().to_string(),
        age,
        scope: capture.filter(|_| kind == Kind::SCOPE).map(scope),
        readout: capture
            .filter(|_| kind == Kind::READOUT)
            .map(|capture| readout(capture, source, bpm)),
    }
}

/// `value` to at most `places` decimals, with no negative zero; not a
/// number, or one too large to round, gives 0 (JSON carries only finite
/// numbers).
fn round(value: f64, places: i32) -> f64 {
    let scale = 10_f64.powi(places);
    let rounded = (value * scale).round() / scale;
    if !rounded.is_finite() || rounded == 0.0 {
        0.0
    } else {
        rounded
    }
}

/// A level as the wire carries it: four decimals.
fn level(value: f32) -> f64 {
    round(f64::from(value), 4)
}

/// One input's reading, as the protocol carries it.
#[must_use]
pub fn measured(reading: &Reading) -> Measured {
    let seen = reading.min <= reading.max;
    Measured {
        connected: reading.connected,
        waiting: reading.connected && reading.frames == 0 && reading.faults == 0,
        last: level(reading.last),
        low: level(reading.low),
        high: level(reading.high),
        mean: level(reading.mean),
        rms_db: round(dbfs(f64::from(reading.rms)), 2),
        peak_db: round(dbfs(f64::from(reading.peak)), 2),
        hz: (reading.hz > 0.0).then(|| round(f64::from(reading.hz), 3)),
        rhythm: match (reading.hz > 0.0, reading.paused, reading.steady) {
            (false, _, _) => Rhythm::None,
            (true, true, _) => Rhythm::Paused,
            (true, false, true) => Rhythm::Steady,
            (true, false, false) => Rhythm::Wavering,
        },
        shape: shape(reading).map(str::to_string),
        gate: GateMeasured {
            hz: (reading.gate.hz > 0.0).then(|| round(f64::from(reading.gate.hz), 3)),
            duty: (reading.gate.duty >= 0.0).then(|| round(f64::from(reading.gate.duty), 3)),
            high: reading.gate.high,
            paused: reading.gate.paused,
        },
        min: seen.then(|| level(reading.min)),
        max: seen.then(|| level(reading.max)),
        faults: reading.faults,
        faults_since_reset: reading.faults_since,
    }
}

/// A repeating wave's shape in a word or two; `None` when it does not
/// repeat steadily.
///
/// It goes by how the wave spends its time: held at its extremes (a
/// square, or a pulse when lopsided), rising slowly and falling at once (a
/// saw) or the other way, or else by its crest factor (peak over RMS about
/// its mean): a sine's is √2, a triangle's √3.
#[must_use]
pub fn shape(reading: &Reading) -> Option<&'static str> {
    let swing = reading.high - reading.low;
    if !(reading.steady && reading.hz > 0.0 && swing >= 1.0e-3) {
        return None;
    }
    let mean = f64::from(reading.mean);
    let ac = f64::from(reading.rms)
        .mul_add(f64::from(reading.rms), -(mean * mean))
        .max(0.0)
        .sqrt();
    if ac <= 0.0 {
        return None;
    }
    if reading.held >= 0.85 {
        // Two levels, high for a share d of the time: the AC RMS is
        // swing × √(d(1 − d)), half the swing for an even square.
        return Some(if ac / f64::from(swing) >= 0.47 {
            "square"
        } else {
            "pulse"
        });
    }
    if reading.rising >= 0.85 {
        return Some("saw");
    }
    if reading.rising <= 0.15 {
        return Some("falling saw");
    }
    let crest =
        f64::from(reading.high - reading.mean).max(f64::from(reading.mean - reading.low)) / ac;
    Some(if crest < 1.55 {
        "sine"
    } else if crest < 1.9 {
        "triangle"
    } else {
        "complex wave"
    })
}

/// The scope's mode, by name.
const fn scope_mode(mode: ScopeMode) -> &'static str {
    match mode {
        ScopeMode::Trigger => SCOPE_MODES[0],
        ScopeMode::Free => SCOPE_MODES[1],
        ScopeMode::Xy => SCOPE_MODES[2],
    }
}

/// A scope's capture, as the protocol carries it.
#[must_use]
pub fn scope(capture: &Capture) -> ScopeReading {
    let trace = &capture.trace;
    let [time, zoom, trigger, mode] = capture.knobs;
    let zoom = if zoom.is_finite() && zoom > 0.0 {
        zoom
    } else {
        1.0
    };
    let mode = ScopeMode::of(mode);
    let filled = usize::from(trace.points.min(trace.capacity));
    let pairs = |channel: usize| -> Vec<[f64; 2]> {
        (0..filled)
            .map(|point| {
                [
                    level(trace.low[channel][point]),
                    level(trace.high[channel][point]),
                ]
            })
            .collect()
    };
    let (a, b, xy) = if trace.xy && !trace.b_current {
        // b only just re-plugged: no figure of half the old b.
        (Vec::new(), Vec::new(), Vec::new())
    } else if trace.xy {
        let xy = (0..filled)
            .map(|point| [level(trace.low[0][point]), level(trace.low[1][point])])
            .collect();
        (Vec::new(), Vec::new(), xy)
    } else {
        // Unplugged, or only just plugged (nothing of it measured yet):
        // no trace of b.
        let b_measured =
            capture.inputs[1].connected && capture.inputs[1].frames > 0 && trace.b_current;
        let b = if b_measured { pairs(1) } else { Vec::new() };
        (pairs(0), b, Vec::new())
    };
    let span = if trace.span > 0.0 { trace.span } else { time };
    let mut reading = ScopeReading {
        mode: scope_mode(mode).to_string(),
        span: widen(span),
        zoom: widen(zoom),
        trigger: widen(trigger / zoom),
        triggered: trace.triggered,
        points: u32::from(trace.capacity),
        sweeping: (trace.swept < trace.points).then_some(u32::from(trace.swept)),
        a,
        b,
        xy,
        input_a: measured(&capture.inputs[0]),
        input_b: measured(&capture.inputs[1]),
        words: String::new(),
    };
    reading.words = words(&reading);
    reading
}

/// A level with its sign and three decimals: `+0.750`, `-0.250`, `0`;
/// a thousand or more in scientific form (`+1.5e6`), so it stays short.
fn signed(value: f64) -> String {
    let text = if value.abs() >= 1_000.0 {
        format!("{value:+.1e}")
    } else {
        format!("{value:+.3}")
    };
    if text == "+0.000" || text == "-0.000" {
        "0".to_string()
    } else {
        text
    }
}

/// A count, at most five digits: `12`, `99999+`.
fn count(count: u32) -> String {
    if count > 99_999 {
        "99999+".to_string()
    } else {
        count.to_string()
    }
}

/// A frequency as people read it.
fn hertz(hz: f64) -> String {
    format::hertz(hz as f32)
}

/// One input in words.
fn input_words(name: &str, input: &Measured, zoom: f64) -> String {
    if !input.connected {
        return format!("{name} unplugged");
    }
    if input.waiting {
        return format!("{name} just plugged in, not measured yet");
    }
    let fault = if input.faults_since_reset > 0 {
        format!(
            "{name} carries non-numbers ({} in the last tenth of a second, {} since its reset: \
             a fault upstream); ",
            input.faults, input.faults_since_reset
        )
    } else {
        String::new()
    };
    let swing = input.high - input.low;
    if swing < 1.0e-3 {
        return if input.mean.abs() < 1.0e-3 {
            format!("{fault}{name} silent{}", paused(input))
        } else {
            format!(
                "{fault}{name} steady at {}{}",
                signed(input.mean),
                paused(input)
            )
        };
    }
    let range = format!("from {} to {}", signed(input.low), signed(input.high));
    let said = match (input.hz, input.steady()) {
        (Some(hz), _) if input.paused() => {
            format!("{name}: paused (it was {}) {range}", hertz(hz))
        }
        (Some(hz), true) => {
            let shape = input.shape.as_deref().unwrap_or("wave");
            format!("{name}: {} {shape} {range}", hertz(hz))
        }
        (Some(hz), false) => format!("{name}: wavering around {} {range}", hertz(hz)),
        (None, _) => format!("{name}: moving {range}, not repeating"),
    };
    let off = if input.low.abs().max(input.high.abs()) > 1.0 / zoom {
        format!(" (off the screen at ×{})", round(zoom, 3))
    } else {
        String::new()
    };
    format!("{fault}{said}{off}")
}

/// What a held input was doing before it paused, for its words.
fn paused(input: &Measured) -> String {
    match input.hz {
        Some(hz) if input.paused() => format!(" (paused: it was {})", hertz(hz)),
        _ => String::new(),
    }
}

/// A rate that has paused, as a readout's line: its last.
fn was(hz: f64) -> String {
    format!("was {}, paused", hertz(hz))
}

/// Small whole numbers `p:q` (up to 8 each) within 1% of `ratio`.
fn small_ratio(ratio: f64) -> Option<(u32, u32)> {
    (1..=8_u32)
        .flat_map(|q| (1..=8_u32).map(move |p| (p, q)))
        .find(|&(p, q)| (f64::from(p) / f64::from(q) - ratio).abs() <= 0.01 * ratio)
}

/// A scope's picture in plain words: each input, then how it is drawn.
#[must_use]
pub fn words(reading: &ScopeReading) -> String {
    let a = input_words("a", &reading.input_a, reading.zoom);
    let b = input_words("b", &reading.input_b, reading.zoom);
    let across = format::seconds(reading.span as f32);
    let drawn = match reading.mode.as_str() {
        "xy" => {
            let figure = match (reading.input_a.hz, reading.input_b.hz) {
                (Some(x), Some(y)) if reading.input_a.steady() && reading.input_b.steady() => {
                    small_ratio(x / y).map_or_else(
                        || {
                            " (no steady figure: their frequencies are not in a small ratio)"
                                .to_string()
                        },
                        |(p, q)| format!(" (a {p}:{q} figure)"),
                    )
                }
                _ => String::new(),
            };
            format!("drawn a across against b up{figure}")
        }
        "free" => format!("rolling, {across} across"),
        _ if reading.triggered => format!(
            "triggered on a rising through {}, {across} across",
            signed(reading.trigger)
        ),
        _ => format!(
            "waiting for a to rise through {}: sweeping without a trigger, {across} across",
            signed(reading.trigger)
        ),
    };
    let under_way = reading.sweeping.map_or_else(String::new, |drawn| {
        format!(
            "; a sweep is under way: {drawn} of {} points drawn, the rest the last sweep's",
            reading.points
        )
    });
    format!("{a}; {b}; {drawn}{under_way}")
}

/// The nearest note to a pitch `cv` (1.0 per octave, 0 = C4), and the
/// cents from it; none beyond the ±8 octaves pitch is held to.
#[must_use]
pub fn note(cv: f64) -> Option<(String, f64)> {
    if !(cv.is_finite() && cv.abs() <= f64::from(CV_LIMIT)) {
        return None;
    }
    let semitones = cv * 12.0;
    let nearest = semitones.round();
    // Pitch CV is held to ±8 octaves: the cast is exact.
    let index = nearest as i64;
    let name = NOTE_NAMES[index.rem_euclid(12) as usize];
    let octave = 4 + index.div_euclid(12);
    let cents = round((semitones - nearest) * 100.0, 1);
    Some((format!("{name}{octave}"), cents))
}

/// A share as a whole percentage.
fn percent(share: f64) -> String {
    format!("{}%", round(share * 100.0, 0))
}

/// The line that says how a gate's rate sits on the wall's beat.
fn on_the_beat(gate_hz: f64, bpm: f64) -> Option<String> {
    if !(gate_hz > 0.0 && bpm > 0.0) {
        return None;
    }
    let beats = bpm / 60.0 / gate_hz;
    CLOCK_DIVISIONS
        .iter()
        .zip(CLOCK_LABELS)
        .find(|(division, _)| (beats - **division).abs() <= 0.01 * **division)
        .map_or_else(
            || Some(format!("{} beats apart", round(beats, 2))),
            |(_, label)| Some(format!("{label} at {}", format::bpm(bpm))),
        )
}

/// The lowest and highest since the last reset: what it has `seen`.
fn extremes(input: &Measured) -> String {
    match (input.min, input.max) {
        (Some(min), Some(max)) => format!("seen {}..{}", signed(min), signed(max)),
        _ => "seen nothing".to_string(),
    }
}

/// How often something cycles, as a readout's line: a rate under 20 Hz
/// with how many a minute too.
fn rate(hz: f64) -> String {
    if hz < 20.0 {
        format!("{} · {}/min", hertz(hz), round(hz * 60.0, 1))
    } else {
        hertz(hz)
    }
}

/// A readout's capture, as the protocol carries it: shown as its `mode`
/// knob asks, or at `auto` as `source` (what is plugged in) says, as audio
/// when that is not known.
#[must_use]
pub fn readout(capture: &Capture, source: Option<Show>, bpm: f64) -> ReadoutReading {
    let chosen = Show::chosen(capture.knobs[0]);
    let show = chosen.or(source).unwrap_or(Show::Audio);
    let input = measured(&capture.inputs[0]);
    let pitch = if show == Show::Pitch {
        note(input.last)
    } else {
        None
    };
    let pitch_hz = pitch
        .as_ref()
        .map(|_| round(f64::from(C4_HZ) * input.last.exp2(), 3));
    let mut lines = Vec::new();
    if input.faults_since_reset > 0 {
        lines.push(format!("! {} not numbers", count(input.faults_since_reset)));
    }
    if input.connected {
        match show {
            Show::Pitch => {
                match &pitch {
                    Some((name, cents)) => {
                        let cents = round(*cents, 0);
                        lines.push(if cents == 0.0 {
                            name.clone()
                        } else {
                            format!("{name} {cents:+}¢")
                        });
                    }
                    None => lines.push("out of range".to_string()),
                }
                if let Some(hz) = pitch_hz {
                    lines.push(hertz(hz));
                }
                lines.push(format!("cv {}", signed(input.last)));
            }
            Show::Cv => {
                lines.push(format!("cv {}", signed(input.last)));
                lines.push(match (input.hz, input.steady()) {
                    (Some(hz), _) if input.paused() => was(hz),
                    (Some(hz), true) => rate(hz),
                    (Some(hz), false) => format!("~{}", rate(hz)),
                    (None, _) => "not cycling".to_string(),
                });
                lines.push(format!("now {}..{}", signed(input.low), signed(input.high)));
            }
            Show::Gate => {
                lines.push(match input.gate.hz {
                    Some(hz) if input.gate.paused => was(hz),
                    Some(hz) => rate(hz),
                    None => "no pulses".to_string(),
                });
                let state = if input.gate.high { "high" } else { "low" };
                lines.push(input.gate.duty.map_or_else(
                    || state.to_string(),
                    |duty| format!("duty {} · {state}", percent(duty)),
                ));
                if let Some(beat) = input.gate.hz.and_then(|hz| on_the_beat(hz, bpm)) {
                    lines.push(beat);
                }
            }
            Show::Audio => {
                lines.push(format!("peak {} dB", round(input.peak_db, 1)));
                lines.push(format!("rms {} dB", round(input.rms_db, 1)));
                lines.push(format!("dc {}", signed(input.mean)));
                lines.push(match (input.hz, input.steady()) {
                    (Some(hz), _) if input.paused() => was(hz),
                    (Some(hz), true) => hertz(hz),
                    (Some(hz), false) => format!("~{}", hertz(hz)),
                    (None, _) => "no pitch".to_string(),
                });
            }
        }
        lines.push(extremes(&input));
    } else {
        lines.push("nothing plugged in".to_string());
    }
    ReadoutReading {
        mode: show.name().to_string(),
        auto: chosen.is_none(),
        note: pitch.as_ref().map(|(name, _)| name.clone()),
        cents: pitch.as_ref().map(|(_, cents)| *cents),
        pitch_hz,
        input,
        lines,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::measure::{Meter, TRACE_POINTS};

    /// What a meter makes of `frames` of `signal` at 48 kHz.
    fn metered(frames: usize, signal: impl Fn(usize) -> f32) -> Reading {
        let mut meter = Meter::new(48_000.0);
        meter.set_connected(true);
        for frame in 0..frames {
            meter.feed(signal(frame));
        }
        *meter.latest()
    }

    fn sine(hz: f64, amplitude: f64) -> impl Fn(usize) -> f32 {
        move |frame| {
            (amplitude * (std::f64::consts::TAU * hz * frame as f64 / 48_000.0).sin()) as f32
        }
    }

    fn readout_of(reading: Reading, mode: f32) -> Capture {
        Capture {
            knobs: [mode, 0.0, 0.0, 0.0],
            inputs: [reading, Reading::NONE],
            ..Capture::NONE
        }
    }

    /// A test signal, frame by frame.
    type Wave = Box<dyn Fn(usize) -> f32>;

    #[test]
    fn shapes_are_told_apart() {
        let period = 48_000.0 / 300.0;
        let phase = move |frame: usize| (frame as f64 / period).fract();
        let cases: [(&str, Wave); 6] = [
            ("sine", Box::new(sine(300.0, 0.7))),
            (
                "triangle",
                Box::new(move |frame| ((phase(frame) * 4.0 - 2.0).abs() - 1.0) as f32 * 0.7),
            ),
            (
                "saw",
                Box::new(move |frame| (phase(frame) * 2.0 - 1.0) as f32),
            ),
            (
                "falling saw",
                Box::new(move |frame| (1.0 - phase(frame) * 2.0) as f32),
            ),
            (
                "square",
                Box::new(move |frame| if phase(frame) < 0.5 { 0.5 } else { -0.5 }),
            ),
            (
                "pulse",
                Box::new(move |frame| if phase(frame) < 0.1 { 0.5 } else { -0.5 }),
            ),
        ];
        for (name, signal) in cases {
            let reading = metered(24_000, signal);
            assert_eq!(shape(&reading), Some(name), "{reading:?}");
        }
        // What does not repeat has no shape.
        let mut rng = crate::dsp::Rng::new(3);
        let noise: Vec<f32> = (0..24_000).map(|_| rng.bipolar()).collect();
        assert_eq!(shape(&metered(noise.len(), |frame| noise[frame])), None);
        assert_eq!(shape(&metered(24_000, |_| 0.4)), None);
    }

    #[test]
    fn pitch_reads_as_a_note_with_cents_and_hz() {
        assert_eq!(note(0.0), Some(("C4".to_string(), 0.0)));
        assert_eq!(note(0.75), Some(("A4".to_string(), 0.0)));
        assert_eq!(note(-1.0), Some(("C3".to_string(), 0.0)));
        assert_eq!(note(-1.0 / 12.0), Some(("B3".to_string(), 0.0)));
        let (name, cents) = note(0.75 + 0.03 / 12.0).unwrap();
        assert_eq!(name, "A4");
        assert!((cents - 3.0).abs() < 0.05, "{cents}");
        assert_eq!(note(f64::NAN), None);

        // A4 held: the readout says so, in cents and Hz.
        let reading = metered(4_800, |_| 0.75);
        let shown = readout(&readout_of(reading, 0.0), Some(Show::Pitch), 120.0);
        assert_eq!(shown.mode, "pitch");
        assert!(shown.auto);
        assert_eq!(shown.note.as_deref(), Some("A4"));
        assert!((shown.pitch_hz.unwrap() - 440.0).abs() < 0.01);
        assert_eq!(shown.lines[0], "A4");
        assert_eq!(shown.lines[1], "440 Hz");
        assert_eq!(shown.lines[2], "cv +0.750");
        assert_eq!(shown.lines[3], "seen +0.750..+0.750");
        // Three cents sharp.
        let reading = metered(4_800, |_| (0.75 + 0.03 / 12.0) as f32);
        let shown = readout(&readout_of(reading, 1.0), None, 120.0);
        assert!(!shown.auto);
        assert_eq!(shown.lines[0], "A4 +3¢");
        // Beyond the eight octaves pitch is held to, no note.
        let reading = metered(4_800, |_| 1.0e30);
        let shown = readout(&readout_of(reading, 1.0), None, 120.0);
        assert_eq!(shown.lines[0], "out of range");
        assert_eq!(shown.lines[1], "cv +1.0e30");
        assert_eq!((shown.note.clone(), shown.pitch_hz), (None, None));
        assert!(serde_json::to_string(&shown).is_ok());
    }

    #[test]
    fn a_plain_cv_reads_its_value_rate_and_range() {
        let reading = metered(48_000 * 10, |frame| {
            (0.4 * (std::f64::consts::TAU * 0.5 * frame as f64 / 48_000.0).sin()) as f32
        });
        let shown = readout(&readout_of(reading, 0.0), Some(Show::Cv), 120.0);
        assert_eq!(shown.mode, "cv");
        assert!(shown.note.is_none(), "an LFO is not a note");
        assert!(shown.lines[0].starts_with("cv "), "{:?}", shown.lines);
        assert_eq!(shown.lines[1], "0.5 Hz · 30/min");
        assert!(shown.lines[2].starts_with("now "), "{:?}", shown.lines);
        assert_eq!(shown.lines[3], "seen -0.400..+0.400");
    }

    #[test]
    fn a_wave_that_stops_reads_as_paused_with_its_last_rate() {
        let mut meter = Meter::new(48_000.0);
        meter.set_connected(true);
        for frame in 0..48_000 * 7 {
            meter.feed(
                (0.4 * (std::f64::consts::TAU * 0.5 * f64::from(frame) / 48_000.0).sin()) as f32,
            );
        }
        for _ in 0..48_000 * 3 {
            meter.feed(0.0);
        }
        let reading = *meter.latest();
        let shown = readout(&readout_of(reading, 0.0), Some(Show::Cv), 120.0);
        assert_eq!(shown.lines[1], "was 0.5 Hz, paused");
        assert!(shown.input.paused() && !shown.input.steady());
        let audio = readout(&readout_of(reading, 4.0), None, 120.0);
        assert_eq!(audio.lines[3], "was 0.5 Hz, paused");
        let mut capture = Capture::NONE;
        capture.knobs = [0.02, 1.0, 0.0, 1.0];
        capture.inputs[0] = reading;
        let words = scope(&capture).words;
        assert!(
            words.starts_with("a silent (paused: it was 0.5 Hz); "),
            "{words}"
        );
        // A clock that stops, likewise.
        let mut gate = metered(
            48_000 * 2,
            |frame| if frame % 6_000 < 3_000 { 1.0 } else { 0.0 },
        );
        gate.gate.paused = true;
        let shown = readout(&readout_of(gate, 3.0), None, 120.0);
        assert_eq!(shown.lines[0], "was 8 Hz, paused");
    }

    #[test]
    fn every_line_fits_the_face_however_wild_the_numbers() {
        let mut wild = metered(
            4_800,
            |frame| if frame % 3 == 0 { f32::NAN } else { -3.0e38 },
        );
        wild.faults_since = u32::MAX;
        wild.hz = 23_999.0;
        wild.gate.hz = 19.99;
        wild.paused = true;
        wild.gate.duty = 1.0;
        wild.max = 3.0e38;
        for mode in 0..=4 {
            for source in [
                None,
                Some(Show::Pitch),
                Some(Show::Cv),
                Some(Show::Gate),
                Some(Show::Audio),
            ] {
                let shown = readout(
                    &readout_of(wild, f32::from(u8::try_from(mode).unwrap())),
                    source,
                    300.0,
                );
                for line in &shown.lines {
                    assert!(line.chars().count() <= READOUT_COLUMNS, "{line:?}");
                }
                let value = serde_json::to_value(&shown).unwrap();
                assert!(serde_json::from_value::<ReadoutReading>(value).is_ok());
            }
        }
        let shown = readout(&readout_of(wild, 3.0), None, 120.0);
        assert_eq!(shown.lines[0], "! 99999+ not numbers");
    }

    #[test]
    fn a_gate_reads_its_rate_duty_and_place_on_the_beat() {
        // Sixteenths at 120 BPM, high half the time.
        let reading = metered(
            48_000,
            |frame| if frame % 6_000 < 3_000 { 1.0 } else { 0.0 },
        );
        let shown = readout(&readout_of(reading, 0.0), Some(Show::Gate), 120.0);
        assert_eq!(shown.mode, "gate");
        assert_eq!(shown.lines[0], "8 Hz · 480/min");
        assert!(
            shown.lines[1].starts_with("duty 50% · "),
            "{:?}",
            shown.lines
        );
        assert_eq!(shown.lines[2], "1/16 at 120 BPM");
        assert!((shown.input.gate.duty.unwrap() - 0.5).abs() < 1e-3);
        // Not on a division: how far apart, in beats.
        let reading = metered(48_000, |frame| if frame % 7_000 < 700 { 1.0 } else { 0.0 });
        let shown = readout(&readout_of(reading, 3.0), Some(Show::Audio), 120.0);
        assert_eq!(shown.mode, "gate");
        assert_eq!(shown.lines[2], "0.29 beats apart");
        assert!(
            shown.lines[1].starts_with("duty 10% · "),
            "{:?}",
            shown.lines
        );
        // A gate held low has no rate.
        let reading = metered(4_800, |_| 0.0);
        let shown = readout(&readout_of(reading, 3.0), None, 120.0);
        assert_eq!(shown.lines[0], "no pulses");
        assert_eq!(shown.lines[1], "low");
    }

    #[test]
    fn audio_reads_its_levels_offset_and_frequency() {
        let reading = metered(48_000, |frame| sine(440.0, 0.5)(frame) + 0.1);
        let shown = readout(&readout_of(reading, 0.0), Some(Show::Audio), 120.0);
        assert_eq!(shown.mode, "audio");
        let input = &shown.input;
        assert!((input.peak_db - dbfs(0.6)).abs() < 0.1, "{input:?}");
        assert!(
            (input.rms_db - dbfs((0.125 + 0.01_f64).sqrt())).abs() < 0.1,
            "{input:?}"
        );
        assert!((input.mean - 0.1).abs() < 1e-3);
        assert!((input.hz.unwrap() - 440.0).abs() < 0.05);
        assert_eq!(input.shape.as_deref(), Some("sine"));
        assert_eq!(shown.lines[0], "peak -4.4 dB");
        assert_eq!(shown.lines[2], "dc +0.100");
        assert_eq!(shown.lines[3], "440 Hz");
        assert_eq!(shown.lines[4], "seen -0.400..+0.600");
        // Unknown source: audio.
        assert_eq!(
            readout(&readout_of(reading, 0.0), None, 120.0).mode,
            "audio"
        );
    }

    #[test]
    fn a_fault_is_shown_as_one_and_unplugged_says_so() {
        let reading = metered(4_800, |frame| if frame % 2 == 0 { f32::NAN } else { 0.25 });
        let shown = readout(&readout_of(reading, 4.0), None, 120.0);
        assert_eq!(shown.lines[0], "! 2400 not numbers");
        assert_eq!(shown.input.faults, 2_400);
        assert!((shown.input.mean - 0.25).abs() < 1e-6);
        let mut unplugged = metered(4_800, |_| 0.0);
        unplugged.connected = false;
        let shown = readout(&readout_of(unplugged, 0.0), None, 120.0);
        assert_eq!(shown.lines, vec!["nothing plugged in".to_string()]);
        // Nothing measured since the last reset: no extremes.
        let measured = measured(&Reading::NONE);
        assert_eq!((measured.min, measured.max), (None, None));
        assert_eq!(extremes(&measured), "seen nothing");
        // Numbers too large to round travel as 0, never as a non-number.
        assert!(round(1.0e308, 4).abs() < f64::EPSILON);
        assert!(round(f64::INFINITY, 2).abs() < f64::EPSILON);
    }

    #[test]
    fn a_scope_capture_reads_as_points_and_words() {
        let mut capture = Capture::NONE;
        capture.knobs = [0.01, 2.0, 0.5, 0.0];
        capture.inputs[0] = metered(48_000, sine(220.0, 0.4));
        capture.inputs[1] = Reading::NONE;
        capture.trace.capacity = 96;
        capture.trace.points = 96;
        capture.trace.swept = 96;
        capture.trace.span = 0.01;
        capture.trace.triggered = true;
        for point in 0..TRACE_POINTS {
            capture.trace.low[0][point] = -0.123_456;
            capture.trace.high[0][point] = 0.25;
        }
        let shown = scope(&capture);
        assert_eq!(shown.mode, "trigger");
        assert_eq!(shown.points, 96);
        assert_eq!(shown.a.len(), 96);
        assert!((shown.a[0][0] + 0.1235).abs() < 1e-12 && (shown.a[0][1] - 0.25).abs() < 1e-12);
        assert!(shown.b.is_empty(), "b is unplugged");
        assert!(shown.xy.is_empty());
        assert!((shown.trigger - 0.25).abs() < 1e-9, "level over zoom");
        assert_eq!(
            shown.words,
            "a: 220 Hz sine from -0.400 to +0.400; b unplugged; triggered on a rising \
             through +0.250, 10 ms across"
        );
        // At ×4 the wave runs off the screen, and the words say so.
        capture.knobs[1] = 4.0;
        assert!(scope(&capture).words.contains("(off the screen at ×4)"));

        // XY: pairs, and the figure the frequencies make.
        capture.knobs = [0.01, 1.0, 0.0, 2.0];
        capture.trace.xy = true;
        capture.inputs[1] = metered(48_000, sine(330.0, 0.4));
        let shown = scope(&capture);
        assert_eq!(shown.mode, "xy");
        assert!(shown.a.is_empty() && shown.b.is_empty());
        assert_eq!(shown.xy.len(), 96);
        assert!(
            shown
                .words
                .ends_with("drawn a across against b up (a 2:3 figure)"),
            "{}",
            shown.words
        );

        // Free running, and a sweep that found no trigger.
        capture.knobs = [0.5, 1.0, 0.0, 1.0];
        capture.trace.xy = false;
        assert!(scope(&capture).words.ends_with("rolling, 10 ms across"));
        capture.knobs[3] = 0.0;
        capture.trace.triggered = false;
        assert!(scope(&capture).words.contains("sweeping without a trigger"));
    }

    #[test]
    fn a_sweep_under_way_and_a_b_just_plugged_are_said() {
        let mut capture = Capture::NONE;
        capture.knobs = [10.0, 1.0, 0.0, 0.0];
        capture.inputs[0] = metered(4_800, |_| 0.25);
        capture.inputs[1] = Reading {
            connected: true,
            ..Reading::NONE
        };
        capture.trace.capacity = 96;
        capture.trace.points = 96;
        capture.trace.swept = 30;
        capture.trace.triggered = true;
        let shown = scope(&capture);
        assert_eq!(shown.sweeping, Some(30));
        assert!(shown.b.is_empty(), "b is not measured yet");
        assert!(shown.input_b.waiting);
        assert!(
            shown.words.contains("b just plugged in, not measured yet"),
            "{}",
            shown.words
        );
        assert!(
            shown.words.ends_with(
                "; a sweep is under way: 30 of 96 points drawn, the rest the last sweep's"
            ),
            "{}",
            shown.words
        );
        capture.trace.swept = 96;
        assert_eq!(scope(&capture).sweeping, None);
    }

    #[test]
    fn silence_and_steady_levels_read_plainly() {
        let mut capture = Capture::NONE;
        capture.knobs = [0.02, 1.0, 0.0, 0.0];
        capture.inputs[0] = metered(4_800, |_| 0.0);
        capture.inputs[1] = metered(4_800, |_| -0.5);
        let words = scope(&capture).words;
        assert!(
            words.starts_with("a silent; b steady at -0.500; "),
            "{words}"
        );
    }

    #[test]
    fn a_probe_carries_what_its_kind_measures() {
        let capture = readout_of(metered(4_800, |_| 0.5), 0.0);
        let found = probe(
            "readout1",
            Kind::READOUT,
            Some((&capture, Duration::from_millis(1_234))),
            Some(Show::Cv),
            120.0,
        );
        assert_eq!(found.kind, "readout");
        assert!((found.age - 1.234).abs() < 1e-9);
        assert!(found.scope.is_none() && found.readout.is_some());
        let waiting = probe("scope1", Kind::SCOPE, None, None, 120.0);
        assert!(waiting.scope.is_none() && waiting.readout.is_none());
        assert!(measures(Kind::SCOPE) && measures(Kind::READOUT) && !measures(Kind::VCO));
        assert_eq!(Show::chosen(0.0), None);
        assert_eq!(Show::chosen(1.0), Some(Show::Pitch));
        assert_eq!(Show::chosen(2.0), Some(Show::Cv));
        assert_eq!(Show::chosen(3.0), Some(Show::Gate));
        assert_eq!(Show::chosen(4.0), Some(Show::Audio));
        for (show, name) in [
            (Show::Pitch, "pitch"),
            (Show::Cv, "cv"),
            (Show::Gate, "gate"),
            (Show::Audio, "audio"),
        ] {
            assert_eq!(show.name(), name);
        }
        let port = |name: &str| {
            let spec = Kind::from_name(name).unwrap().spec();
            &spec.outputs[0]
        };
        assert_eq!(Show::of(port("seq")), Show::Pitch);
        assert_eq!(Show::of(port("quant")), Show::Pitch);
        assert_eq!(Show::of(port("lfo")), Show::Cv);
        assert_eq!(Show::of(port("env")), Show::Cv);
        assert_eq!(Show::of(port("clock")), Show::Gate);
        assert_eq!(Show::of(port("vco")), Show::Audio);
        assert_eq!(Show::chosen(f32::NAN), None);
    }
}
