//! The wall's answers as text a seat can read: a legible listing with the
//! numbers exact.
//!
//! Values are printed twice where it helps: as the wall reads them (`800
//! Hz`, `saw`, `1/8`) and as the plain number to turn to (`=800`). Numbers
//! come from the protocol already rounded to what was meant (see
//! [`kazoo_wall::protocol::widen`]), so they print as sent.

use kazoo_wall::catalogue::{Curve, JackLaw, Signal};
use kazoo_wall::fingerprints::Shares;
use kazoo_wall::protocol::{
    CatalogueResult, Change, ChangeResult, ClockSource, KindInfo, KnobView, Listen, ListenResult,
    LogPage, ModuleView, RecordResult, Recording, Snapshot, TempoResult, Timings,
};

/// Beats in a bar, for the position line.
const BEATS_PER_BAR: f64 = 4.0;

/// The whole wall.
#[must_use]
pub fn look(snapshot: &Snapshot) -> String {
    let mut out = String::new();
    let bar = (snapshot.beat / BEATS_PER_BAR).floor();
    let beat_in_bar = bar.mul_add(-BEATS_PER_BAR, snapshot.beat);
    line(
        &mut out,
        format_args!(
            "The wall at revision {}: {} BPM, bar {} beat {:.2} ({} beats in), clock {}, sound to {}.",
            snapshot.revision,
            snapshot.tempo,
            bar + 1.0,
            beat_in_bar + 1.0,
            round(snapshot.beat, 2),
            match snapshot.clock {
                ClockSource::Desk => "from the desk",
                ClockSource::Own => "its own",
            },
            match (snapshot.heard, snapshot.on_desk) {
                (false, _) => "nobody: it plays on, silent, until a console turns the monitor on",
                (true, true) => "the kazoo-mix desk",
                (true, false) => "the audio device",
            },
        ),
    );
    line(
        &mut out,
        format_args!("Seats here: {}.", list_or(&snapshot.seats, "none")),
    );
    recording_line(&mut out, snapshot.recording.as_ref());
    line(
        &mut out,
        format_args!(
            "Master peaks: left {} dBFS, right {} dBFS.",
            round(snapshot.levels.peak_l, 1),
            round(snapshot.levels.peak_r, 1)
        ),
    );
    match &snapshot.listen {
        Some(heard) => line(
            &mut out,
            format_args!("Sounds like: {}", listen_line(heard)),
        ),
        None => line(&mut out, format_args!("Sounds like: not heard yet.")),
    }
    if snapshot.faults.count > 0 {
        line(
            &mut out,
            format_args!(
                "Faults since the wall started: {} (latest: {}).",
                snapshot.faults.count,
                list_or(&snapshot.faults.recent, "none kept")
            ),
        );
    }
    line(
        &mut out,
        format_args!(
            "\nModules ({}); knobs read name=value, `a→b` while gliding, [min..max]:",
            snapshot.modules.len()
        ),
    );
    line(
        &mut out,
        format_args!(
            "(hands: …) are fingerprints: each seat's share of who touched the module and of \
             the touches that flowed into it along the cables."
        ),
    );
    for module in &snapshot.modules {
        module_block(
            &mut out,
            module,
            snapshot.fingerprints.modules.get(&module.id),
        );
    }
    line(
        &mut out,
        format_args!("\nCables ({}):", snapshot.cables.len()),
    );
    if snapshot.cables.is_empty() {
        line(&mut out, format_args!("  none"));
    }
    for cable in &snapshot.cables {
        line(
            &mut out,
            format_args!(
                "  #{} {} → {} amount {}",
                cable.id, cable.from, cable.to, cable.amount
            ),
        );
    }
    timing(&mut out, &snapshot.timing);
    out.push_str(
        "\nEvery knob is also a jack: patch an output into `module.knob` to move it. \
         Turn with wall_turn, plug with wall_patch.",
    );
    out
}

/// Whether a recording is under way, for the listing.
fn recording_line(out: &mut String, under_way: Option<&Recording>) {
    let words = under_way.map_or_else(|| "no (wall_record starts one).".to_string(), recording);
    line(out, format_args!("Recording: {words}"));
}

/// A recording under way, for the listing.
fn recording(recording: &Recording) -> String {
    let lost = if recording.dropped > 0 {
        format!(", {} samples lost", recording.dropped)
    } else {
        String::new()
    };
    format!(
        "yes, {} so far into {} ({} Hz, 32-bit float stereo), started by {}{lost}. \
         wall_record with on=false stops it.",
        clock(recording.seconds),
        recording.path,
        recording.sample_rate,
        recording.seat
    )
}

/// A running time on a clock face.
fn clock(seconds: f64) -> String {
    kazoo_wall::format::clock(seconds)
}

/// What the wall holds back to keep its paths in step, if anything.
fn timing(out: &mut String, timing: &Timings) {
    let quiet = timing.modules.is_empty()
        && timing.cables.is_empty()
        && timing.uncompensated.is_empty()
        && timing.unsteady.is_empty();
    if quiet {
        return;
    }
    line(
        out,
        format_args!(
            "\nTiming ({} Hz; every path is kept in step):",
            timing.sample_rate
        ),
    );
    for (id, module) in &timing.modules {
        if module.latency_frames > 0 {
            line(
                out,
                format_args!(
                    "  {id} holds its sound back {} frames ({} ms)",
                    module.latency_frames,
                    round(module.latency_ms, 2)
                ),
            );
        }
        if module.arrival_frames > 0 {
            line(
                out,
                format_args!(
                    "  {id} hears its inputs {} frames late",
                    module.arrival_frames
                ),
            );
        }
    }
    for (cable, frames) in &timing.cables {
        line(
            out,
            format_args!("  cable #{cable} is held back {frames} frames"),
        );
    }
    for cable in &timing.uncompensated {
        line(
            out,
            format_args!(
                "  out of step: cable #{cable} needs more than the longest delay a cable holds"
            ),
        );
    }
    for id in &timing.unsteady {
        line(
            out,
            format_args!(
                "  out of step while patched: {id}'s latency follows a knob with a cable in it"
            ),
        );
    }
}

fn module_block(out: &mut String, module: &ModuleView, shares: Option<&Shares>) {
    let name = module
        .name
        .as_deref()
        .map_or_else(String::new, |name| format!(" \"{name}\""));
    let hands = shares.map_or_else(String::new, hands);
    line(
        out,
        format_args!("{} ({}){name}{hands}", module.id, module.kind),
    );
    let knobs = module.knobs.iter().map(knob).collect::<Vec<_>>();
    line(out, format_args!("  knobs: {}", list_or(&knobs, "none")));
    line(
        out,
        format_args!("  inputs: {}", list_or(&module.inputs, "none")),
    );
    line(
        out,
        format_args!("  outputs: {}", list_or(&module.outputs, "none")),
    );
}

/// ` (hands: Tom 62%, Waffles 30%, Cassio 8%)`, largest share first (by
/// name when equal); empty when nobody's touch reaches the module.
fn hands(shares: &Shares) -> String {
    let mut held = shares
        .iter()
        .filter(|(_, share)| share.is_finite() && **share > 0.0)
        .collect::<Vec<_>>();
    if held.is_empty() {
        return String::new();
    }
    held.sort_by(|a, b| b.1.total_cmp(a.1).then_with(|| a.0.cmp(b.0)));
    let each = held
        .iter()
        .map(|(seat, share)| {
            let percent = (**share * 100.0).round();
            if percent < 1.0 {
                format!("{seat} <1%")
            } else {
                format!("{seat} {percent}%")
            }
        })
        .collect::<Vec<_>>();
    format!(" (hands: {})", each.join(", "))
}

/// `cutoff=800 (800 Hz) [20..18000]`, or while gliding
/// `cutoff=812.5→800 (812 Hz → 800 Hz) [20..18000]`.
fn knob(knob: &KnobView) -> String {
    // Mid-glide the value is on its way; within a hair of the target it
    // has arrived.
    let gliding = (knob.value - knob.target).abs() > f64::EPSILON * knob.target.abs().max(1.0);
    let numbers = if gliding {
        format!("{}→{}", knob.value, knob.target)
    } else {
        knob.target.to_string()
    };
    let shown = if gliding {
        format!("{} → {}", knob.display, knob.target_display)
    } else {
        knob.target_display.clone()
    };
    let shown = if shown == numbers {
        String::new()
    } else {
        format!(" ({shown})")
    };
    format!(
        "{}={numbers}{shown} [{}..{}]",
        knob.name, knob.min, knob.max
    )
}

/// Every module kind, or those of one kind or family.
///
/// # Errors
///
/// The kinds and families there are, when `only` names none of them.
pub fn catalogue(catalogue: &CatalogueResult, only: Option<&str>) -> Result<String, String> {
    let kinds = catalogue
        .kinds
        .iter()
        .filter(|kind| only.is_none_or(|only| kind.kind == only || kind.family == only))
        .collect::<Vec<_>>();
    if kinds.is_empty() {
        let names = catalogue
            .kinds
            .iter()
            .map(|kind| kind.kind.as_str())
            .collect::<Vec<_>>();
        let mut families = catalogue
            .kinds
            .iter()
            .map(|kind| kind.family.as_str())
            .collect::<Vec<_>>();
        families.sort_unstable();
        families.dedup();
        return Err(format!(
            "there is no module kind or family '{}'; kinds: {}; families: {}",
            only.unwrap_or_default().escape_default(),
            names.join(", "),
            families.join(", ")
        ));
    }
    let mut out = String::from(
        "Module kinds. Every knob is also a jack (an input by the knob's name): a cable into \
         it moves the knob by its jack law, scaled by the cable's amount (-1..1), held to the \
         knob's range. Pitch is 1.0 per octave with 0 = C4; gates are high above 0.5.\n",
    );
    for kind in kinds {
        kind_block(&mut out, kind);
    }
    Ok(out)
}

fn kind_block(out: &mut String, kind: &KindInfo) {
    line(
        out,
        format_args!("\n{} ({}): {}", kind.kind, kind.family, kind.about),
    );
    for knob in &kind.knobs {
        let unit = if knob.unit.is_empty() {
            String::new()
        } else {
            format!(" {}", knob.unit)
        };
        let travel = match (knob.stepped, knob.curve) {
            (true, _) => "stepped",
            (false, Curve::Linear) => "linear",
            (false, Curve::Log) => "log",
        };
        let jack = match knob.jack {
            JackLaw::Range => "jack adds cv × range/2".to_string(),
            JackLaw::Octaves(octaves) => {
                format!("jack moves it ±{} oct per unit cv", f64::from(octaves))
            }
        };
        line(
            out,
            format_args!(
                "  knob {}: {}..{}{unit}, default {}, {travel}, {jack}",
                knob.name, knob.min, knob.max, knob.default
            ),
        );
        if !knob.labels.is_empty() {
            let labels = knob
                .labels
                .iter()
                .enumerate()
                .map(|(step, label)| format!("{}={label}", knob.min + step as f64))
                .collect::<Vec<_>>();
            line(out, format_args!("    positions: {}", labels.join(", ")));
        }
    }
    for input in &kind.inputs {
        line(
            out,
            format_args!(
                "  in {} ({}): {}",
                input.name,
                signal(input.signal),
                input.about
            ),
        );
    }
    for output in &kind.outputs {
        line(
            out,
            format_args!(
                "  out {} ({}): {}",
                output.name,
                signal(output.signal),
                output.about
            ),
        );
    }
}

const fn signal(signal: Signal) -> &'static str {
    match signal {
        Signal::Audio => "audio",
        Signal::Gate => "gate",
        Signal::Cv => "cv",
    }
}

/// A change this seat made.
#[must_use]
pub fn changed(result: &ChangeResult) -> String {
    let mut out = format!("Change #{}: {}.", result.change.seq, result.change.summary);
    if let Some(module) = &result.module {
        out.push_str(" Module id: ");
        out.push_str(module);
        out.push('.');
    }
    if let Some(cable) = result.cable {
        out.push_str(" Cable #");
        out.push_str(&cable.to_string());
        out.push('.');
    }
    out
}

/// A tempo this seat set.
#[must_use]
pub fn tempo(result: &TempoResult) -> String {
    let how = if result.desk {
        "asked of the kazoo-mix desk, which sets it for everyone"
    } else {
        "set on the wall's own clock"
    };
    format!(
        "Change #{}: {} BPM, {how}. {}.",
        result.change.seq, result.bpm, result.change.summary
    )
}

/// A recording this seat started or stopped, or found as it was.
#[must_use]
pub fn recorded(result: &RecordResult) -> String {
    let path = result.path.as_deref().unwrap_or("");
    let lost = if result.dropped > 0 {
        format!(
            " {} samples were lost: the writer fell behind.",
            result.dropped
        )
    } else {
        String::new()
    };
    match (&result.change, result.on) {
        (Some(change), true) => format!(
            "Change #{}: {}. Recording to {path}. wall_record with on=false stops it.",
            change.seq, change.summary
        ),
        (Some(change), false) => format!(
            "Change #{}: {}. The file is {path}, {} long.{lost}",
            change.seq,
            change.summary,
            clock(result.seconds)
        ),
        (None, true) => format!(
            "Already recording to {path}, {} so far; nothing changed.{lost}",
            clock(result.seconds)
        ),
        (None, false) => "Nothing was recording; nothing changed.".to_string(),
    }
}

/// A page of the change log.
#[must_use]
pub fn log(page: &LogPage) -> String {
    if page.changes.is_empty() {
        return "No changes in the log there.".to_string();
    }
    let mut out = String::from("Changes, oldest first:\n");
    for change in &page.changes {
        log_line(&mut out, change);
    }
    if page.more {
        if let Some(first) = page.changes.first() {
            line(
                &mut out,
                format_args!(
                    "Older changes exist: call wall_log with before={}.",
                    first.seq
                ),
            );
        }
    }
    out.trim_end().to_string()
}

fn log_line(out: &mut String, change: &Change) {
    let undoes = change
        .undoes
        .map_or_else(String::new, |undone| format!(" (undoes #{undone})"));
    line(
        out,
        format_args!(
            "#{} {} {}: {}{undoes}",
            change.seq, change.at, change.seat, change.summary
        ),
    );
}

/// What the wall sounds like.
#[must_use]
pub fn listen(result: &ListenResult) -> String {
    result.listen.as_ref().map_or_else(
        || {
            "The wall has not been heard yet: its first listening comes a quarter second \
             after it starts. Ask again in a moment."
                .to_string()
        },
        |heard| format!("Heard at {}: {}", heard.at, listen_line(heard)),
    )
}

/// The words, then the numbers.
fn listen_line(heard: &Listen) -> String {
    let pitch = match (&heard.pitch, heard.pitch_hz) {
        (Some(note), Some(hz)) => format!(", pitch {note} ({} Hz)", round(hz, 2)),
        (None, Some(hz)) => format!(", pitch {} Hz", round(hz, 2)),
        (Some(note), None) => format!(", pitch {note}"),
        (None, None) => ", no clear pitch".to_string(),
    };
    format!(
        "{}. RMS {} dBFS, peak {} dBFS, centroid {} Hz, energy low {} / mid {} / high {}, \
         {} onsets per second{pitch}.",
        heard.words,
        round(heard.rms_db, 1),
        round(heard.peak_db, 1),
        round(heard.centroid_hz, 0),
        round(heard.low, 3),
        round(heard.mid, 3),
        round(heard.high, 3),
        round(heard.onsets_per_second, 2),
    )
}

/// An answer this build could not read, shown as it came.
#[must_use]
pub fn unread(why: &str, value: &serde_json::Value) -> String {
    let raw = serde_json::to_string_pretty(value).unwrap_or_else(|err| err.to_string());
    format!(
        "(This kazoo-mcp could not read the wall's answer: {why}. Rebuild kazoo-mcp from the \
         same tree as kazoo-wall. The answer as it came:)\n{raw}"
    )
}

/// `value` to at most `places` decimals, with no trailing zeros and no
/// negative zero.
fn round(value: f64, places: i32) -> f64 {
    let scale = 10_f64.powi(places);
    let rounded = (value * scale).round() / scale;
    if rounded == 0.0 { 0.0 } else { rounded }
}

fn list_or(items: &[String], none: &str) -> String {
    if items.is_empty() {
        none.to_string()
    } else {
        items.join(", ")
    }
}

fn line(out: &mut String, args: std::fmt::Arguments<'_>) {
    out.push_str(&args.to_string());
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use kazoo_wall::fingerprints::Fingerprints;
    use kazoo_wall::protocol::{CableRecord, Faults, KnobInfo, Levels, PortInfo, What};

    use super::*;

    fn knob_view(name: &str, value: f64, target: f64, shown: (&str, &str)) -> KnobView {
        KnobView {
            name: name.to_string(),
            value,
            target,
            min: 20.0,
            max: 18_000.0,
            unit: "Hz".to_string(),
            stepped: false,
            display: shown.0.to_string(),
            target_display: shown.1.to_string(),
        }
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            revision: 58,
            tempo: 96.0,
            beat: 13.5,
            clock: ClockSource::Own,
            on_desk: false,
            heard: true,
            seats: vec!["Tom".to_string(), "Waffles".to_string()],
            modules: vec![ModuleView {
                id: "vcf1".to_string(),
                kind: "vcf".to_string(),
                name: Some("filter".to_string()),
                knobs: vec![
                    knob_view("cutoff", 812.5, 800.0, ("812 Hz", "800 Hz")),
                    KnobView {
                        name: "resonance".to_string(),
                        value: 0.55,
                        target: 0.55,
                        min: 0.0,
                        max: 0.95,
                        unit: String::new(),
                        stepped: false,
                        display: "0.55".to_string(),
                        target_display: "0.55".to_string(),
                    },
                ],
                inputs: vec!["in".to_string()],
                outputs: vec!["out".to_string()],
            }],
            cables: vec![CableRecord {
                id: 8,
                from: "lfo1.out".to_string(),
                to: "vcf1.cutoff".to_string(),
                amount: 0.35,
            }],
            levels: Levels {
                peak_l: -6.23,
                peak_r: -120.0,
            },
            listen: None,
            faults: Faults {
                count: 0,
                recent: Vec::new(),
            },
            fingerprints: Fingerprints {
                modules: std::collections::BTreeMap::from([(
                    "vcf1".to_string(),
                    Shares::from([
                        ("Cassio".to_string(), 0.08),
                        ("Tom".to_string(), 0.62),
                        ("Waffles".to_string(), 0.30),
                        ("Vesper".to_string(), 0.001),
                    ]),
                )]),
                cables: std::collections::BTreeMap::new(),
            },
            timing: kazoo_wall::protocol::Timings::default(),
            recording: None,
            rack: None,
        }
    }

    #[test]
    fn the_wall_reads_as_a_listing_with_exact_numbers() {
        let text = look(&snapshot());
        assert!(text.starts_with(
            "The wall at revision 58: 96 BPM, bar 4 beat 2.50 (13.5 beats in), clock its own, \
             sound to the audio device.\n"
        ));
        assert!(text.contains("Seats here: Tom, Waffles.\n"));
        let silent = look(&Snapshot {
            heard: false,
            ..snapshot()
        });
        assert!(silent.contains(
            "sound to nobody: it plays on, silent, until a console turns the monitor on.\n"
        ));
        assert!(text.contains("Master peaks: left -6.2 dBFS, right -120 dBFS.\n"));
        assert!(text.contains("Recording: no (wall_record starts one).\n"));
        let recording = look(&Snapshot {
            recording: Some(Recording {
                path: "/Users/tom/Music/kazoo-wall/wall-2026-09-27-203001.wav".to_string(),
                seat: "Tom".to_string(),
                seconds: 200.5,
                dropped: 128,
                sample_rate: 48_000,
            }),
            ..snapshot()
        });
        assert!(
            recording.contains(
                "Recording: yes, 3:20 so far into \
                 /Users/tom/Music/kazoo-wall/wall-2026-09-27-203001.wav (48000 Hz, 32-bit float \
                 stereo), started by Tom, 128 samples lost. wall_record with on=false stops it.\n"
            ),
            "{recording}"
        );
        assert!(text.contains("Sounds like: not heard yet.\n"));
        assert!(text.contains(
            "vcf1 (vcf) \"filter\" (hands: Tom 62%, Waffles 30%, Cassio 8%, Vesper <1%)\n"
        ));
        assert!(text.contains("(hands: …) are fingerprints"));
        assert!(text.contains(
            "  knobs: cutoff=812.5→800 (812 Hz → 800 Hz) [20..18000], resonance=0.55 [0..0.95]\n"
        ));
        assert!(text.contains("  inputs: in\n  outputs: out\n"));
        assert!(text.contains("  #8 lfo1.out → vcf1.cutoff amount 0.35\n"));
        assert!(!text.contains("Faults"));
        // Nothing held back: nothing said about timing.
        assert!(!text.contains("Timing"));
    }

    #[test]
    fn the_timing_says_what_is_held_back_and_what_cannot_be() {
        let mut wall = snapshot();
        wall.timing = kazoo_wall::protocol::Timings {
            sample_rate: 48_000,
            modules: std::collections::BTreeMap::from([
                (
                    "flanger1".to_string(),
                    kazoo_wall::protocol::ModuleTiming {
                        latency_frames: 480,
                        latency_ms: 10.0,
                        arrival_frames: 0,
                    },
                ),
                (
                    "mix1".to_string(),
                    kazoo_wall::protocol::ModuleTiming {
                        latency_frames: 0,
                        latency_ms: 0.0,
                        arrival_frames: 480,
                    },
                ),
            ]),
            cables: std::collections::BTreeMap::from([("3".to_string(), 480)]),
            uncompensated: vec![5],
            unsteady: vec!["flanger1".to_string()],
        };
        let text = look(&wall);
        assert!(
            text.contains("\nTiming (48000 Hz; every path is kept in step):\n"),
            "{text}"
        );
        assert!(text.contains("  flanger1 holds its sound back 480 frames (10 ms)\n"));
        assert!(text.contains("  mix1 hears its inputs 480 frames late\n"));
        assert!(text.contains("  cable #3 is held back 480 frames\n"));
        assert!(
            text.contains(
                "  out of step: cable #5 needs more than the longest delay a cable holds\n"
            )
        );
        assert!(text.contains(
            "  out of step while patched: flanger1's latency follows a knob with a cable in it\n"
        ));
    }

    #[test]
    fn the_catalogue_teaches_jacks_ranges_and_positions() {
        let listing = CatalogueResult {
            kinds: vec![KindInfo {
                kind: "clock".to_string(),
                family: "time".to_string(),
                about: "a clock".to_string(),
                knobs: vec![KnobInfo {
                    name: "division".to_string(),
                    min: 0.0,
                    max: 2.0,
                    default: 1.0,
                    unit: "division".to_string(),
                    curve: Curve::Linear,
                    stepped: true,
                    labels: vec!["1/16".to_string(), "1/8".to_string(), "1/4".to_string()],
                    jack: JackLaw::Range,
                }],
                inputs: vec![PortInfo {
                    name: "reset".to_string(),
                    signal: Signal::Gate,
                    about: "back to the start".to_string(),
                }],
                outputs: vec![PortInfo {
                    name: "out".to_string(),
                    signal: Signal::Gate,
                    about: "the ticks".to_string(),
                }],
            }],
        };
        let text = catalogue(&listing, None).unwrap();
        assert!(text.contains("\nclock (time): a clock\n"));
        assert!(text.contains(
            "  knob division: 0..2 division, default 1, stepped, jack adds cv × range/2\n"
        ));
        assert!(text.contains("    positions: 0=1/16, 1=1/8, 2=1/4\n"));
        assert!(text.contains("  in reset (gate): back to the start\n"));
        assert!(text.contains("  out out (gate): the ticks\n"));
        assert_eq!(catalogue(&listing, Some("clock")).unwrap(), text);
        assert_eq!(catalogue(&listing, Some("time")).unwrap(), text);
        assert_eq!(
            catalogue(&listing, Some("vcx")).unwrap_err(),
            "there is no module kind or family 'vcx'; kinds: clock; families: time"
        );
    }

    #[test]
    fn changes_and_the_log_read_plainly() {
        let change = Change {
            seq: 59,
            at: "2026-09-26T12:00:01Z".to_string(),
            seat: "Waffles".to_string(),
            what: What::Tempo {
                from: 92.0,
                to: 96.0,
                desk: false,
            },
            summary: "Waffles added lfo3".to_string(),
            undoes: Some(57),
        };
        let result = ChangeResult {
            change: change.clone(),
            module: Some("lfo3".to_string()),
            cable: Some(12),
        };
        assert_eq!(
            changed(&result),
            "Change #59: Waffles added lfo3. Module id: lfo3. Cable #12."
        );
        let page = LogPage {
            changes: vec![change],
            more: true,
        };
        assert_eq!(
            log(&page),
            "Changes, oldest first:\n#59 2026-09-26T12:00:01Z Waffles: Waffles added lfo3 \
             (undoes #57)\nOlder changes exist: call wall_log with before=59."
        );
        assert_eq!(
            log(&LogPage {
                changes: Vec::new(),
                more: false
            }),
            "No changes in the log there."
        );
    }

    #[test]
    fn recordings_answer_with_their_file() {
        let change = |on: bool, summary: &str| Change {
            seq: 60,
            at: "2026-09-27T10:30:01Z".to_string(),
            seat: "Waffles".to_string(),
            what: What::Record {
                on,
                path: "/m/wall-2026-09-27-203001.wav".to_string(),
                seconds: None,
                dropped: 0,
                continues: None,
                reason: None,
            },
            summary: summary.to_string(),
            undoes: None,
        };
        let started = RecordResult {
            on: true,
            path: Some("/m/wall-2026-09-27-203001.wav".to_string()),
            seconds: 0.0,
            dropped: 0,
            change: Some(change(
                true,
                "Waffles started recording wall-2026-09-27-203001.wav",
            )),
        };
        assert_eq!(
            recorded(&started),
            "Change #60: Waffles started recording wall-2026-09-27-203001.wav. Recording to \
             /m/wall-2026-09-27-203001.wav. wall_record with on=false stops it."
        );
        let stopped = RecordResult {
            on: false,
            seconds: 75.2,
            dropped: 64,
            change: Some(change(
                false,
                "Waffles stopped recording wall-2026-09-27-203001.wav after 1:15",
            )),
            ..started.clone()
        };
        assert_eq!(
            recorded(&stopped),
            "Change #60: Waffles stopped recording wall-2026-09-27-203001.wav after 1:15. The \
             file is /m/wall-2026-09-27-203001.wav, 1:15 long. 64 samples were lost: the writer \
             fell behind."
        );
        let already = RecordResult {
            seconds: 12.0,
            change: None,
            ..started
        };
        assert_eq!(
            recorded(&already),
            "Already recording to /m/wall-2026-09-27-203001.wav, 0:12 so far; nothing changed."
        );
        let idle = RecordResult {
            on: false,
            path: None,
            seconds: 0.0,
            dropped: 0,
            change: None,
        };
        assert_eq!(recorded(&idle), "Nothing was recording; nothing changed.");
    }

    #[test]
    fn listening_gives_words_and_numbers() {
        let heard = Listen {
            at: "2026-09-26T12:00:01Z".to_string(),
            rms_db: -18.345,
            peak_db: -6.0,
            centroid_hz: 420.4,
            low: 0.6204,
            mid: 0.33,
            high: 0.0496,
            onsets_per_second: 1.5,
            pitch_hz: Some(110.0),
            pitch: Some("A2".to_string()),
            words: "dark, sparse, slow pulse around A2, quiet".to_string(),
        };
        assert_eq!(
            listen(&ListenResult {
                listen: Some(heard)
            }),
            "Heard at 2026-09-26T12:00:01Z: dark, sparse, slow pulse around A2, quiet. RMS \
             -18.3 dBFS, peak -6 dBFS, centroid 420 Hz, energy low 0.62 / mid 0.33 / high \
             0.05, 1.5 onsets per second, pitch A2 (110 Hz)."
        );
        assert!(listen(&ListenResult { listen: None }).contains("not been heard yet"));
    }
}
