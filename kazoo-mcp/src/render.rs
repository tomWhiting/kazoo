//! The wall's answers as text a seat can read: a legible listing with the
//! numbers exact.
//!
//! Values are printed twice where it helps: as the wall reads them (`800
//! Hz`, `saw`, `1/8`) and as the plain number to turn to (`=800`). Numbers
//! come from the protocol already rounded to what was meant (see
//! [`kazoo_wall::protocol::widen`]), so they print as sent.

mod cables;
mod loops;

use kazoo_wall::catalogue::{Curve, JackLaw, Signal};
use kazoo_wall::fingerprints::Shares;
use kazoo_wall::protocol::{
    CatalogueResult, Change, ChangeResult, ClockSource, EngineHealth, KindInfo, KnobView, Listen,
    ListenResult, LoadResult, LogHealth, LogPage, MAX_PROBES, Measured, ModuleView, Probe,
    ProbeResult, ReadoutReading, RecordResult, Recording, SaveResult, SavesResult, ScopeReading,
    Snapshot, TempoResult, Timings,
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
            "The wall at revision {}: {} BPM, seed {:016x}, bar {} beat {:.2} ({} beats in), clock {}, sound to {}.",
            snapshot.revision,
            snapshot.tempo,
            snapshot.seed,
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
    status_lines(&mut out, snapshot);
    line(
        &mut out,
        format_args!(
            "Master peaks: left {} dBFS, right {} dBFS.",
            round(snapshot.levels.peak_l, 1),
            round(snapshot.levels.peak_r, 1)
        ),
    );
    engine_line(&mut out, snapshot.engine.as_ref());
    match &snapshot.listen {
        Some(heard) => line(
            &mut out,
            format_args!("Sounds like: {}", listen_line(heard)),
        ),
        None => line(&mut out, format_args!("Sounds like: not heard yet.")),
    }
    probes_line(&mut out, &snapshot.probes);
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
            &loops::module(module, snapshot.tempo, snapshot.beat),
        );
    }
    loops::footer(&mut out, snapshot);
    cables::block(&mut out, snapshot);
    unheard::line(&mut out, &snapshot.timing.unheard);
    timing(&mut out, &snapshot.timing);
    out.push_str(
        "\nEvery knob is also a jack: patch an output into `module.knob` to move it. \
         Turn with wall_turn, plug with wall_patch.",
    );
    out
}

/// The save the wall last went to or came from, if the wall says, for the
/// listing.
fn save_line(out: &mut String, snapshot: &Snapshot) {
    let Some(mark) = &snapshot.save else {
        return;
    };
    let since = if mark.changed {
        "; it has changed since"
    } else {
        "; it has not changed since"
    };
    line(
        out,
        format_args!(
            "Save: last saved as or loaded from \"{}\" at revision {}{since}.",
            mark.name, mark.revision
        ),
    );
}

/// A warning when the change log is not taking changes; nothing while it
/// is.
fn log_health_line(out: &mut String, log: &LogHealth) {
    if let Some(trouble) = log.trouble() {
        line(
            out,
            format_args!("Warning: {trouble}. Nothing said to be made is lost; the wall plays on."),
        );
    }
}

/// How hard the engine is working: the share of its time it spends
/// rendering, and whether the wall takes more modules and cables. Nothing
/// from a wall older than the report.
fn engine_line(out: &mut String, engine: Option<&EngineHealth>) {
    let Some(engine) = engine else {
        return;
    };
    let load = round(engine.load * 100.0, 0);
    let room = match engine.load_guard {
        Some(guard) if engine.refusing => format!(
            "that is past what it keeps to ({}%), so it takes no more modules or cables until \
             it is well below; take something away",
            round(guard * 100.0, 0)
        ),
        Some(_) if engine.measuring => "the wall was just put up and is still being measured: \
                                        more modules and cables wait a few seconds"
            .to_string(),
        Some(guard) => format!(
            "it takes more modules and cables until {}%",
            round(guard * 100.0, 0)
        ),
        None => "it does not refuse more for it".to_string(),
    };
    line(
        out,
        format_args!("Rendering: {load}% of the time the wall has; {room}."),
    );
    if engine.leaked > 0 || engine.ignored > 0 {
        line(
            out,
            format_args!(
                "Warning: the engine refused {} commands and kept {} retired items it could not \
                 free: some of the wall may not be playing.",
                engine.ignored, engine.leaked
            ),
        );
    }
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

fn module_block(out: &mut String, module: &ModuleView, shares: Option<&Shares>, tape: &str) {
    let name = module
        .name
        .as_deref()
        .map_or_else(String::new, |name| format!(" \"{name}\""));
    let hands = shares.map_or_else(String::new, hands);
    line(
        out,
        format_args!("{} ({}){name}{hands}{tape}", module.id, module.kind),
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
            JackLaw::Span => "jack moves it through every position as cv goes 0 to 1".to_string(),
            JackLaw::Unknown => "jack law unknown to this build".to_string(),
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
        "asked of the kazoo-mix desk, which sets it for everyone when it takes it (the \
         change records the asking; wall_look shows the tempo playing)"
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

/// A save this seat made.
#[must_use]
pub fn saved(result: &SaveResult) -> String {
    let save = &result.save;
    let notes = if result.notes.is_empty() {
        String::new()
    } else {
        format!(" Note: {}.", result.notes.join("; "))
    };
    format!(
        "Saved the wall as \"{}\": {} BPM, {} modules, {} cables, saved {}. Nothing about the \
         sound changed. wall_load with this name brings the wall back to this; only Tom's \
         console can replace or delete a save.{notes}",
        save.name, save.tempo, save.modules, save.cables, save.saved_at
    )
}

/// The saves.
#[must_use]
pub fn saves(result: &SavesResult) -> String {
    if result.saves.is_empty() && result.broken.is_empty() && result.pending.is_empty() {
        return "No saves yet: wall_save saves the wall as it is.".to_string();
    }
    let mut out = String::new();
    // Saves made by hand first; the wall's own (one before every load)
    // after, so they never bury the others.
    let (own, made): (Vec<_>, Vec<_>) = result.saves.iter().partition(|save| save.auto);
    for (saves, heading) in [
        (made, "Saves, newest first:\n"),
        (
            own,
            "The wall's own saves (the wall as it was before each load), newest first:\n",
        ),
    ] {
        if !saves.is_empty() {
            out.push_str(heading);
        }
        for save in saves {
            let older = if save.version < kazoo_wall::patch::FILE_VERSION {
                " (made by an older wall: brought up to date as it loads)"
            } else {
                ""
            };
            line(
                &mut out,
                format_args!(
                    "- \"{}\" saved {}: {} BPM, {} modules, {} cables{older}",
                    save.name, save.saved_at, save.tempo, save.modules, save.cables
                ),
            );
        }
    }
    if !result.broken.is_empty() {
        out.push_str("Cannot be loaded:\n");
    }
    for broken in &result.broken {
        line(
            &mut out,
            format_args!(
                "- \"{}\" (written {}): {}",
                broken.name, broken.saved_at, broken.problem
            ),
        );
    }
    if !result.pending.is_empty() {
        line(
            &mut out,
            format_args!(
                "Not read yet (the wall reads them a few at a time; call wall_saves again in a \
                 moment): {}",
                result.pending.join(", ")
            ),
        );
    }
    out.push_str(
        "wall_load with a name replaces the whole wall with that save while it plays; the wall \
         as it is is saved first, and wall_undo of the load brings it back.",
    );
    out
}

/// A load this seat made.
#[must_use]
pub fn loaded(result: &LoadResult) -> String {
    let kept = result.backup.strip_prefix("tree:").map_or_else(
        || format!("kept as the save \"{}\"", result.backup),
        |commit| format!("moment {commit} in the tree"),
    );
    let mut out = format!(
        "Change #{}: {}. The whole wall is new: call wall_look before changing anything. \
         wall_undo with change {} brings back the wall as it was (it is also {kept}).",
        result.change.seq, result.change.summary, result.change.seq
    );
    if !result.notes.is_empty() {
        out.push_str(" Notes: ");
        out.push_str(&result.notes.join("; "));
        out.push('.');
    }
    out
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
            change.seq,
            change.at,
            change.seat,
            change.said()
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

/// The scopes and readouts in a line, for the listing: what each shows,
/// and that `wall_probe` has the numbers.
fn probes_line(out: &mut String, probes: &[Probe]) {
    if probes.is_empty() {
        return;
    }
    let each: Vec<String> = probes
        .iter()
        .map(|probe| {
            let shows = match (&probe.scope, &probe.readout) {
                (Some(scope), _) => scope.words.clone(),
                (None, Some(readout)) => readout.lines.join(", "),
                (None, None) => "no reading yet".to_string(),
            };
            format!("{} ({}): {shows}", probe.module, probe.kind)
        })
        .collect();
    line(
        out,
        format_args!(
            "Probes (wall_probe has their numbers): {}.",
            each.join("; ")
        ),
    );
}

/// The blocks a sparkline is drawn in, lowest first.
const SPARKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Most characters a trace's sparkline takes.
const SPARK_WIDTH: usize = 48;

/// Which part of each point of a trace a sparkline draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edge {
    /// Its middle (neighbouring points averaged).
    Middle,
    /// Its highest (the highest of neighbouring points).
    High,
    /// Its lowest (the lowest of neighbouring points).
    Low,
}

/// A screen's zoom that draws: the scope's own, or 1 when it is not a
/// number.
fn screen_zoom(zoom: f64) -> f64 {
    if zoom.is_finite() && zoom > 0.0 {
        zoom
    } else {
        1.0
    }
}

/// Whether a trace is a band: some point spans more than one of a
/// sparkline's eight steps from its lowest to its highest (a fast wave
/// across a long span), so its middle alone would read as flat.
fn banded(points: &[[f64; 2]], zoom: f64) -> bool {
    let zoom = screen_zoom(zoom);
    points
        .iter()
        .any(|[low, high]| (high - low) * zoom * 3.5 >= 1.0)
}

/// A trace as a sparkline: `edge` of each point, `▁` at the bottom of the
/// screen (-1 / zoom) up to `█` at its top, at most [`SPARK_WIDTH`]
/// characters (neighbouring points taken together).
fn sparkline(points: &[[f64; 2]], zoom: f64, edge: Edge) -> String {
    if points.is_empty() {
        return String::new();
    }
    let zoom = screen_zoom(zoom);
    let per = points.len().div_ceil(SPARK_WIDTH);
    points
        .chunks(per)
        .map(|chunk| {
            let value = match edge {
                Edge::Middle => {
                    chunk
                        .iter()
                        .map(|[low, high]| f64::midpoint(*low, *high))
                        .sum::<f64>()
                        / chunk.len() as f64
                }
                Edge::High => chunk.iter().map(|[_, high]| *high).fold(f64::MIN, f64::max),
                Edge::Low => chunk.iter().map(|[low, _]| *low).fold(f64::MAX, f64::min),
            };
            let height = (value * zoom).mul_add(0.5, 0.5) * 7.0;
            let level = if height.is_finite() {
                height.round().clamp(0.0, 7.0)
            } else {
                0.0
            };
            // Held to 0..=7: the cast is exact.
            SPARKS[level as usize]
        })
        .collect()
}

/// One input's numbers.
fn measured_line(input: &Measured) -> String {
    if !input.connected {
        return "unplugged.".to_string();
    }
    if input.waiting {
        return "just plugged in, not measured yet.".to_string();
    }
    let repeats = match (input.hz, input.steady()) {
        (Some(hz), _) if input.paused() => format!(
            "paused: it was cycling at {} Hz, and has gone longer than a cycle since",
            round(hz, 3)
        ),
        (Some(hz), true) => format!("{} Hz (steady)", round(hz, 3)),
        (Some(hz), false) => format!("about {} Hz (unsteady)", round(hz, 3)),
        (None, _) => "does not repeat".to_string(),
    };
    let shape = input
        .shape
        .as_ref()
        .map_or_else(String::new, |shape| format!(", {shape}"));
    let state = if input.gate.high { "high" } else { "low" };
    let gate = match (input.gate.hz, input.gate.duty) {
        (Some(hz), _) if input.gate.paused => format!(
            "as a gate paused (it was rising at {} Hz), {state} now",
            round(hz, 3)
        ),
        (Some(hz), Some(duty)) => format!(
            "as a gate {} Hz ({}/min), duty {}%, {state} now",
            round(hz, 3),
            round(hz * 60.0, 1),
            round(duty * 100.0, 1)
        ),
        (Some(hz), None) => format!(
            "as a gate {} Hz ({}/min), {state} now",
            round(hz, 3),
            round(hz * 60.0, 1)
        ),
        (None, _) => format!("as a gate no pulses, {state} now"),
    };
    let extremes = match (input.min, input.max) {
        (Some(min), Some(max)) => format!("since reset {min} to {max}"),
        _ => "nothing measured since reset".to_string(),
    };
    let faults = if input.faults > 0 || input.faults_since_reset > 0 {
        format!(
            "; {} non-numbers in the last tenth of a second, {} since its reset (a fault \
             upstream: counted, never measured)",
            input.faults, input.faults_since_reset
        )
    } else {
        String::new()
    };
    format!(
        "last {}, low {}, high {}, mean (DC) {}, RMS {} dBFS, peak {} dBFS; {repeats}{shape}; \
         {gate}; {extremes}{faults}.",
        input.last, input.low, input.high, input.mean, input.rms_db, input.peak_db
    )
}

fn scope_block(out: &mut String, scope: &ScopeReading) {
    line(out, format_args!("  {}.", scope.words));
    let trigger = match (scope.mode.as_str(), scope.triggered) {
        ("trigger", true) => format!(", trigger at {} (this sweep triggered)", scope.trigger),
        ("trigger", false) => format!(
            ", trigger at {} (nothing rose through it: this sweep ran without one)",
            scope.trigger
        ),
        _ => String::new(),
    };
    // Only a whole sweep is a sweep: one under way is said to be.
    let under_way = scope.sweeping.map_or_else(String::new, |drawn| {
        format!(
            " A sweep is under way: its first {drawn} of {} points are drawn; the rest are the \
             last sweep's.",
            scope.points
        )
    });
    line(
        out,
        format_args!(
            "  Screen: {} mode, {} s across in {} points, zoom ×{} (it shows ±{}){trigger}.\
             {under_way}",
            scope.mode,
            scope.span,
            scope.points,
            scope.zoom,
            round(1.0 / scope.zoom, 4)
        ),
    );
    line(out, format_args!("  a: {}", measured_line(&scope.input_a)));
    line(out, format_args!("  b: {}", measured_line(&scope.input_b)));
    if scope.xy.is_empty() {
        for (name, trace) in [("a", &scope.a), ("b", &scope.b)] {
            if trace.is_empty() {
                continue;
            }
            if banded(trace, scope.zoom) {
                line(
                    out,
                    format_args!(
                        "  Trace {name} is a band (each point spans a swing: a fast wave across \
                         the span), left to right (▁ bottom to █ top of the screen):"
                    ),
                );
                line(
                    out,
                    format_args!("    highs {}", sparkline(trace, scope.zoom, Edge::High)),
                );
                line(
                    out,
                    format_args!("    lows  {}", sparkline(trace, scope.zoom, Edge::Low)),
                );
            } else {
                line(
                    out,
                    format_args!(
                        "  Trace {name}, left to right (▁ bottom to █ top of the screen): {}",
                        sparkline(trace, scope.zoom, Edge::Middle)
                    ),
                );
            }
        }
    } else {
        line(
            out,
            format_args!(
                "  Figure: {} points of a (across) against b (up).",
                scope.xy.len()
            ),
        );
    }
}

fn readout_block(out: &mut String, readout: &ReadoutReading) {
    let how = if readout.auto {
        "chosen by what is plugged in"
    } else {
        "set by its mode knob"
    };
    line(
        out,
        format_args!(
            "  Shown as {} ({how}): {}.",
            readout.mode,
            readout.lines.join(" | ")
        ),
    );
    if let (Some(note), Some(cents), Some(hz)) = (&readout.note, readout.cents, readout.pitch_hz) {
        line(
            out,
            format_args!("  As pitch (1.0 per octave, 0 = C4): {note} {cents:+} cents, {hz} Hz."),
        );
    }
    line(
        out,
        format_args!("  Input: {}", measured_line(&readout.input)),
    );
}

/// What the scopes and readouts measured.
#[must_use]
pub fn probe(result: &ProbeResult) -> String {
    if result.probes.is_empty() {
        return "No scopes or readouts on the wall. wall_add a scope (it draws inputs a and b) \
                or a readout (numbers for one input), patch what you want to measure into it \
                (it passes it through, so it can sit inline), then call wall_probe again."
            .to_string();
    }
    let mut out = String::new();
    for found in &result.probes {
        match (&found.scope, &found.readout) {
            (None, None) => line(
                &mut out,
                format_args!(
                    "{} ({}): no reading yet; it hands one on every tenth of a second or so \
                     (a scope once a sweep), so ask again in a moment.",
                    found.module, found.kind
                ),
            ),
            (scope, readout) => {
                line(
                    &mut out,
                    format_args!(
                        "{} ({}), measured {} s ago:",
                        found.module, found.kind, found.age
                    ),
                );
                if let Some(scope) = scope {
                    scope_block(&mut out, scope);
                }
                if let Some(readout) = readout {
                    readout_block(&mut out, readout);
                }
            }
        }
    }
    if !result.missing.is_empty() {
        line(
            &mut out,
            format_args!(
                "Not on the wall (taken away), or not a scope or readout: {}.",
                result.missing.join(", ")
            ),
        );
    }
    if result.dropped > 0 {
        line(
            &mut out,
            format_args!(
                "The wall has dropped {} captures since it started, falling behind taking them \
                 in (a busy machine): readings come a little less often, never wrong.",
                result.dropped
            ),
        );
    }
    if !result.left_out.is_empty() {
        line(
            &mut out,
            format_args!(
                "Not listed (an answer for every one carries the first {MAX_PROBES}): {}; ask \
                 for each with module.",
                result.left_out.join(", ")
            ),
        );
    }
    out.trim_end().to_string()
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

/// The log's health, the recording, the save and the groups.
fn status_lines(out: &mut String, snapshot: &Snapshot) {
    log_health_line(out, &snapshot.log);
    recording_line(out, snapshot.recording.as_ref());
    save_line(out, snapshot);
    groups_line(out, snapshot);
}

/// `Groups: kick (osc1, klon2).`, when the wall has groups.
fn groups_line(out: &mut String, snapshot: &Snapshot) {
    if !snapshot.groups.is_empty() {
        let groups: Vec<String> = snapshot
            .groups
            .iter()
            .map(|(group, members)| format!("{group} ({})", members.join(", ")))
            .collect();
        line(out, format_args!("Groups: {}.", groups.join("; ")));
    }
}
/// What `wall_group` answers: every group now.
#[must_use]
pub fn groups(result: &kazoo_wall::protocol::GroupsResult) -> String {
    if result.groups.is_empty() {
        return "The wall has no groups now.".to_string();
    }
    let groups: Vec<String> = result
        .groups
        .iter()
        .map(|(group, members)| format!("{group}: {}", members.join(", ")))
        .collect();
    format!("The wall's groups now: {}.", groups.join("; "))
}

#[cfg(test)]
mod tests {
    use kazoo_wall::fingerprints::Fingerprints;
    use kazoo_wall::protocol::{
        CableRecord, Faults, GateMeasured, KnobInfo, Levels, PortInfo, Rhythm, What,
    };

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

    #[test]
    fn look_shows_the_seed() {
        let mut wall = snapshot();
        wall.seed = 0x5EED;
        let text = look(&wall);
        assert!(
            text.lines()
                .next()
                .unwrap()
                .contains("seed 0000000000005eed")
        );
    }

    pub(super) fn snapshot() -> Snapshot {
        Snapshot {
            loops_meet: None,
            checkpoints_paused: false,
            revision: 58,
            wall: 0,
            seed: 0,
            tempo: 96.0,
            beat: 13.5,
            clock: ClockSource::Own,
            on_desk: false,
            heard: true,
            seats: vec!["Tom".to_string(), "Waffles".to_string()],
            modules: vec![ModuleView {
                tape: None,
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
            groups: std::collections::BTreeMap::new(),
            save: None,
            log: kazoo_wall::protocol::LogHealth::default(),
            probes: Vec::new(),
            engine: None,
            tree: None,
        }
    }

    #[test]
    fn the_wall_reads_as_a_listing_with_exact_numbers() {
        let text = look(&snapshot());
        assert!(text.starts_with(
            "The wall at revision 58: 96 BPM, seed 0000000000000000, bar 4 beat 2.50 (13.5 beats in), clock its own, \
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
        assert!(!text.contains("change log"), "{text}");
        let stopped = look(&Snapshot {
            log: kazoo_wall::protocol::LogHealth {
                owed: 2,
                error: Some("No space left on device".to_string()),
            },
            ..snapshot()
        });
        assert!(
            stopped.contains(
                "Warning: the change log is not taking changes (No space left on device) and \
                 the wall owes it 2 of its own changes: every change is refused, and \
                 patch.json is not saved, until it takes them. Nothing said to be made is \
                 lost; the wall plays on.\n"
            ),
            "{stopped}"
        );
        // Owing nothing, only not synced: changes are not refused.
        let unsynced = look(&Snapshot {
            log: kazoo_wall::protocol::LogHealth {
                owed: 0,
                error: Some("I/O error".to_string()),
            },
            ..snapshot()
        });
        assert!(
            unsynced.contains(
                "Warning: the change log failed (I/O error) and has not been synced since: \
                 changes still go to it, but patch.json is not saved until it syncs."
            ),
            "{unsynced}"
        );
        assert!(!unsynced.contains("refused"), "{unsynced}");
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
            unheard: Vec::new(),
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
                    pitch: false,
                }],
                outputs: vec![PortInfo {
                    name: "out".to_string(),
                    signal: Signal::Gate,
                    about: "the ticks".to_string(),
                    pitch: false,
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
        // A law from a newer wall is said to be unknown, not left out.
        let mut newer = listing;
        newer.kinds[0].knobs[0].jack = JackLaw::Unknown;
        let text = catalogue(&newer, None).unwrap();
        assert!(text.contains(
            "  knob division: 0..2 division, default 1, stepped, jack law unknown to this build\n"
        ));
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
            clock: None,
            comment: None,
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
            clock: None,
            comment: None,
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

    fn quiet(connected: bool) -> Measured {
        Measured {
            connected,
            waiting: false,
            last: 0.0,
            low: 0.0,
            high: 0.0,
            mean: 0.0,
            rms_db: -120.0,
            peak_db: -120.0,
            hz: None,
            rhythm: Rhythm::None,
            shape: None,
            gate: GateMeasured::default(),
            min: None,
            max: None,
            faults: 0,
            faults_since_reset: 0,
        }
    }

    fn probes() -> ProbeResult {
        let ramp: Vec<[f64; 2]> = (0..96)
            .map(|point| {
                let value = f64::from(point) / 47.5 - 1.0;
                [value, value]
            })
            .collect();
        ProbeResult {
            probes: vec![
                Probe {
                    module: "scope1".to_string(),
                    kind: "scope".to_string(),
                    age: 0.04,
                    scope: Some(ScopeReading {
                        mode: "trigger".to_string(),
                        span: 0.02,
                        zoom: 1.0,
                        trigger: 0.0,
                        triggered: true,
                        points: 96,
                        sweeping: None,
                        a: ramp,
                        b: Vec::new(),
                        xy: Vec::new(),
                        input_a: Measured {
                            last: 0.5,
                            low: -0.8,
                            high: 0.8,
                            rms_db: -3.01,
                            peak_db: -1.94,
                            hz: Some(220.0),
                            rhythm: Rhythm::Steady,
                            shape: Some("sine".to_string()),
                            gate: GateMeasured {
                                hz: Some(220.0),
                                duty: Some(0.333),
                                high: true,
                                paused: false,
                            },
                            min: Some(-0.8),
                            max: Some(0.8),
                            ..quiet(true)
                        },
                        input_b: quiet(false),
                        words: "a: 220 Hz sine from -0.800 to +0.800; b unplugged; triggered \
                                on a rising through 0, 20 ms across"
                            .to_string(),
                    }),
                    readout: None,
                },
                Probe {
                    module: "readout1".to_string(),
                    kind: "readout".to_string(),
                    age: 0.1,
                    scope: None,
                    readout: Some(kazoo_wall::protocol::ReadoutReading {
                        mode: "pitch".to_string(),
                        auto: true,
                        input: Measured {
                            last: 0.7525,
                            faults: 12,
                            faults_since_reset: 12,
                            ..quiet(true)
                        },
                        note: Some("A4".to_string()),
                        cents: Some(3.0),
                        pitch_hz: Some(440.764),
                        lines: vec![
                            "! 12 not numbers".to_string(),
                            "A4 +3¢".to_string(),
                            "441 Hz".to_string(),
                        ],
                    }),
                },
                Probe {
                    module: "scope2".to_string(),
                    kind: "scope".to_string(),
                    age: 0.0,
                    scope: None,
                    readout: None,
                },
            ],
            left_out: vec!["readout17".to_string()],
            missing: Vec::new(),
            dropped: 0,
        }
    }

    #[test]
    fn probes_read_as_numbers_a_sparkline_and_words() {
        let text = probe(&probes());
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "scope1 (scope), measured 0.04 s ago:");
        assert_eq!(
            lines[1],
            "  a: 220 Hz sine from -0.800 to +0.800; b unplugged; triggered on a rising through \
             0, 20 ms across."
        );
        assert_eq!(
            lines[2],
            "  Screen: trigger mode, 0.02 s across in 96 points, zoom ×1 (it shows ±1), trigger \
             at 0 (this sweep triggered)."
        );
        assert_eq!(
            lines[3],
            "  a: last 0.5, low -0.8, high 0.8, mean (DC) 0, RMS -3.01 dBFS, peak -1.94 dBFS; \
             220 Hz (steady), sine; as a gate 220 Hz (13200/min), duty 33.3%, high now; since \
             reset -0.8 to 0.8."
        );
        assert_eq!(lines[4], "  b: unplugged.");
        let spark = lines[5]
            .strip_prefix("  Trace a, left to right (▁ bottom to █ top of the screen): ")
            .unwrap();
        assert_eq!(spark.chars().count(), 48);
        assert!(spark.starts_with('▁') && spark.ends_with('█'), "{spark}");
        assert_eq!(lines[6], "readout1 (readout), measured 0.1 s ago:");
        assert_eq!(
            lines[7],
            "  Shown as pitch (chosen by what is plugged in): ! 12 not numbers | A4 +3¢ | 441 Hz."
        );
        assert_eq!(
            lines[8],
            "  As pitch (1.0 per octave, 0 = C4): A4 +3 cents, 440.764 Hz."
        );
        assert!(
            lines[9].contains(
                "12 non-numbers in the last tenth of a second, 12 since its reset (a fault \
                 upstream"
            ),
            "{}",
            lines[9]
        );
        assert!(
            lines[10].starts_with("scope2 (scope): no reading yet"),
            "{}",
            lines[10]
        );
        assert_eq!(
            lines[11],
            "Not listed (an answer for every one carries the first 16): readout17; ask for each \
             with module."
        );
        assert!(
            probe(&ProbeResult {
                probes: Vec::new(),
                left_out: Vec::new(),
                missing: Vec::new(),
                dropped: 0,
            })
            .starts_with("No scopes or readouts on the wall.")
        );
        // A sweep under way, a b just plugged, a paused wave, and ids
        // another seat took away, all said.
        let mut partial = probes();
        if let Some(scope) = partial.probes[0].scope.as_mut() {
            scope.sweeping = Some(30);
            scope.input_b = Measured {
                waiting: true,
                ..quiet(true)
            };
            scope.input_a.rhythm = Rhythm::Paused;
        }
        partial.missing = vec!["scope9".to_string()];
        let text = probe(&partial);
        assert!(
            text.contains("A sweep is under way: its first 30 of 96 points are drawn"),
            "{text}"
        );
        assert!(
            text.contains("  b: just plugged in, not measured yet."),
            "{text}"
        );
        assert!(text.contains("paused: it was cycling at 220 Hz"), "{text}");
        assert!(
            text.contains("Not on the wall (taken away), or not a scope or readout: scope9."),
            "{text}"
        );
        // Captures the wall dropped are said, as what they mean.
        let mut busy = probes();
        busy.dropped = 7;
        assert!(probe(&busy).contains(
            "The wall has dropped 7 captures since it started, falling behind taking them in"
        ));
    }

    #[test]
    fn a_fast_wave_across_a_long_span_reads_as_a_band() {
        // Each point spans the wave from -0.8 to 0.8: its middle is flat,
        // so the highs and the lows are drawn instead.
        let mut wide = probes();
        if let Some(scope) = wide.probes[0].scope.as_mut() {
            scope.a = vec![[-0.8, 0.8]; 96];
        }
        let text = probe(&wide);
        assert!(
            text.contains("  Trace a is a band (each point spans a swing"),
            "{text}"
        );
        let highs = text
            .lines()
            .find_map(|line| line.strip_prefix("    highs "))
            .unwrap();
        let lows = text
            .lines()
            .find_map(|line| line.strip_prefix("    lows  "))
            .unwrap();
        assert_eq!(highs, "▇".repeat(48));
        assert_eq!(lows, "▂".repeat(48));
        assert!(banded(&[[-0.8, 0.8]], 1.0));
        assert!(!banded(&[[0.1, 0.12]], 1.0));
        assert!(banded(&[[0.1, 0.12]], 64.0), "a narrow band zoomed in");
    }

    #[test]
    fn a_sparkline_spans_the_screen_and_survives_nonsense() {
        assert_eq!(sparkline(&[], 1.0, Edge::Middle), "");
        assert_eq!(
            sparkline(&[[-1.0, -1.0], [0.0, 0.0], [1.0, 1.0]], 1.0, Edge::Middle),
            "▁▅█"
        );
        // Zoomed in, a small wave fills the screen; beyond it, the edge.
        assert_eq!(
            sparkline(&[[-0.25, -0.25], [0.25, 0.25]], 4.0, Edge::Middle),
            "▁█"
        );
        assert_eq!(
            sparkline(&[[-9.0, -9.0], [9.0, 9.0]], 1.0, Edge::Middle),
            "▁█"
        );
        assert_eq!(
            sparkline(&[[f64::NAN, f64::NAN]], f64::NAN, Edge::Middle),
            "▁"
        );
        assert_eq!(
            sparkline(&[[0.0, 0.0]; 96], 1.0, Edge::Middle)
                .chars()
                .count(),
            48
        );
        assert_eq!(
            sparkline(&[[0.0, 0.0]; 7], 1.0, Edge::Middle)
                .chars()
                .count(),
            7
        );
    }

    #[test]
    fn the_listing_points_at_the_probes() {
        let mut wall = snapshot();
        assert!(!look(&wall).contains("Probes"));
        wall.probes = probes().probes;
        let listing = look(&wall);
        assert!(
            listing.contains(
                "Probes (wall_probe has their numbers): scope1 (scope): a: 220 Hz sine from \
                 -0.800 to +0.800; b unplugged; triggered on a rising through 0, 20 ms across; \
                 readout1 (readout): ! 12 not numbers, A4 +3¢, 441 Hz; scope2 (scope): no \
                 reading yet."
            ),
            "{listing}"
        );
    }

    fn save_info(name: &str, auto: bool, version: u32) -> kazoo_wall::protocol::SaveInfo {
        kazoo_wall::protocol::SaveInfo {
            name: name.to_string(),
            saved_at: "2026-09-29T02:00:00Z".to_string(),
            tempo: 96.0,
            modules: 12,
            cables: 30,
            auto,
            version,
        }
    }

    #[test]
    fn saves_read_as_a_list_with_what_to_do() {
        let saved = SaveResult {
            save: save_info("tom fiddle", false, 2),
            replaced: false,
            notes: Vec::new(),
        };
        assert_eq!(
            super::saved(&saved),
            "Saved the wall as \"tom fiddle\": 96 BPM, 12 modules, 30 cables, saved \
             2026-09-29T02:00:00Z. Nothing about the sound changed. wall_load with this name \
             brings the wall back to this; only Tom's console can replace or delete a save."
        );
        let listed = SavesResult {
            saves: vec![
                save_info("tom fiddle", false, 2),
                save_info("before-load-20260929-120000", true, 2),
                save_info("airports", false, 1),
            ],
            broken: vec![kazoo_wall::protocol::BrokenSave {
                name: "torn".to_string(),
                saved_at: "2026-09-26T12:00:00Z".to_string(),
                problem: "it is not JSON".to_string(),
            }],
            pending: Vec::new(),
        };
        assert_eq!(
            saves(&listed),
            "Saves, newest first:\n\
             - \"tom fiddle\" saved 2026-09-29T02:00:00Z: 96 BPM, 12 modules, 30 cables\n\
             - \"airports\" saved 2026-09-29T02:00:00Z: 96 BPM, 12 modules, 30 cables (made by \
             an older wall: brought up to date as it loads)\n\
             The wall's own saves (the wall as it was before each load), newest first:\n\
             - \"before-load-20260929-120000\" saved 2026-09-29T02:00:00Z: 96 BPM, 12 modules, \
             30 cables\n\
             Cannot be loaded:\n\
             - \"torn\" (written 2026-09-26T12:00:00Z): it is not JSON\n\
             wall_load with a name replaces the whole wall with that save while it plays; the \
             wall as it is is saved first, and wall_undo of the load brings it back."
        );
        assert_eq!(
            saves(&SavesResult {
                saves: Vec::new(),
                broken: Vec::new(),
                pending: Vec::new(),
            }),
            "No saves yet: wall_save saves the wall as it is."
        );
        // Saves the wall has not read yet are named, to ask again.
        let reading = saves(&SavesResult {
            saves: Vec::new(),
            broken: Vec::new(),
            pending: vec!["late jam".to_string()],
        });
        assert!(
            reading.contains(
                "Not read yet (the wall reads them a few at a time; call wall_saves again in a \
                 moment): late jam\n"
            ),
            "{reading}"
        );
    }

    #[test]
    fn a_load_says_the_wall_is_new_and_how_to_undo_it() {
        let result = LoadResult {
            change: Change {
                seq: 80,
                at: "2026-09-29T02:00:00Z".to_string(),
                seat: "Waffles".to_string(),
                what: kazoo_wall::protocol::What::Load {
                    save: Some("tom fiddle".to_string()),
                    backup: "before-load-20260929-120000".to_string(),
                    modules: 12,
                    cables: 30,
                    tempo: 96.0,
                    fingerprint: "0123456789abcdef-2048".to_string(),
                    by_console: false,
                },
                summary: "Waffles loaded the save \"tom fiddle\" (12 modules, 30 cables, 96 BPM); \
                          the wall as it was is saved as \"before-load-20260929-120000\""
                    .to_string(),
                undoes: None,
                clock: None,
                comment: None,
            },
            backup: "before-load-20260929-120000".to_string(),
            notes: vec!["the desk keeps the tempo at 120 BPM".to_string()],
        };
        let text = loaded(&result);
        assert!(text.starts_with("Change #80: Waffles loaded the save \"tom fiddle\""));
        assert!(text.contains("call wall_look before changing anything"));
        assert!(text.contains("wall_undo with change 80 brings back the wall as it was"));
        assert!(text.ends_with(" Notes: the desk keeps the tempo at 120 BPM."));
        // The listing says which save the wall came from.
        let mut marked = snapshot();
        marked.save = Some(kazoo_wall::protocol::SaveMark {
            name: "tom fiddle".to_string(),
            revision: 57,
            changed: true,
        });
        assert!(look(&marked).contains(
            "Save: last saved as or loaded from \"tom fiddle\" at revision 57; it has changed \
             since.\n"
        ));
        // Said by the wall, not worked out from the revision.
        marked.save = Some(kazoo_wall::protocol::SaveMark {
            name: "tom fiddle".to_string(),
            revision: 1,
            changed: false,
        });
        assert!(look(&marked).contains(
            "Save: last saved as or loaded from \"tom fiddle\" at revision 1; it has not \
             changed since.\n"
        ));
    }
}

mod tree;
mod unheard;
pub use tree::{marked, tree, went};
