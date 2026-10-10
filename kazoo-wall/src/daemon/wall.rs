//! The daemon's state: the patch, the change log, the engine it drives, and
//! everything a request can ask about.
//!
//! Sockets live elsewhere ([`super::control`]); this is plain state with
//! methods, so it can be tested without any.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use kazoo_core::ipc::link::{HubLink, RequestError};
use kazoo_fx::Effect;
use ringbuf::HeapCons;
use ringbuf::traits::{Consumer, Observer};

use super::record::{Ending, Finished, Recorder};
use super::speech::Speech;
use super::timing::{Timing, timing};
use crate::adapters::{self, Adapter};
use crate::catalogue::{Builder, Curve, Jack, Kind, KnobSpec, PortSpec, Signal};
use crate::change::{summarise_with, utc, utc_now};
use crate::dsp;
use crate::dsp::measure::{Capture, Reading};
use crate::engine::{
    CableTable, Command, EngineControl, Order, ProbeFrame, Route, clamp_bpm, processing_order,
};
use crate::fingerprints::Fingerprints;
use crate::format;
use crate::listen::dbfs;
use crate::patch::{
    Cable, FILE_VERSION, Module, Patch, Words, hash_bytes, never_undone, printable,
};
use crate::probe;
use crate::protocol::{
    ArrangeResult, BrokenSave, CatalogueResult, Change, ChangeResult, ClockSource,
    DeleteSaveResult, ErrorCode, Event, Faults, KindInfo, KnobInfo, KnobView, Levels, Listen,
    ListenResult, LoadResult, LogHealth, LogPage, MAX_PROBES, MAX_SAVE_NAME, ModuleTiming,
    ModuleView, Place, PortInfo, Probe, ProbeResult, RecordResult, Recording, Request, SaveInfo,
    SaveMark, SaveResult, SavesResult, Snapshot, TempoResult, Timings, WallError, What,
    is_auto_save, valid_save_name, widen,
};
use crate::store::{AUTO_SAVES_KEPT, LoadWriteError, Need, SaveError, Store};
use crate::{MAX_CABLES, MAX_INPUTS, MAX_KNOBS, MAX_MODULES};

/// The seat the daemon's own changes (bringing the patch up to date,
/// carrying a recording on or stopping one) are logged under.
pub const LOADER_SEAT: &str = "kazoo-wall";

/// Changes kept in memory for `log` and `undo`.
pub const MEMORY_CHANGES: usize = 10_000;

/// Changes a seat may make in a burst...
pub const FLOOD_BURST: f64 = 30.0;

/// ...refilled at this many per second (30 per 10 s).
pub const FLOOD_REFILL_PER_SECOND: f64 = 3.0;

/// The most changes one `log` request returns.
pub const MAX_LOG_PAGE: u32 = 500;

/// Changes `log` returns when not told how many.
pub const DEFAULT_LOG_PAGE: u32 = 50;

/// The patch is saved at most this often.
pub const SAVE_INTERVAL: Duration = Duration::from_secs(1);

/// A module's faults are reported at most this often.
pub const FAULT_REPORT_INTERVAL: Duration = Duration::from_secs(5);

/// Recent faults kept for `look`.
const RECENT_FAULTS: usize = 10;

/// How long an effect's knobs must rest before its latency is measured
/// again (and the cables re-timed), so a sweep of a knob the latency
/// follows is re-timed once when it settles, not at every step.
pub const RETIME_AFTER: Duration = Duration::from_millis(100);

/// Meter fall per tick of the control loop.
const METER_DECAY: f32 = 0.85;

/// Listings of the saves a seat may ask for in a burst...
pub const SAVES_BURST: f64 = 10.0;

/// ...refilled at this many per second.
///
/// A listing looks at every file in the saves directory (and reads whole,
/// as a load would, up to [`crate::store::HEADER_READS_PER_LISTING`] of
/// those not read since they last changed), so a seat asking in a loop
/// must not hold up the wall's control thread; a person opening the list,
/// or a console following saves as they happen, asks far less often.
pub const SAVES_REFILL_PER_SECOND: f64 = 1.0;

/// Saves a seat that is not a console may make in a burst...
pub const SAVE_BURST: f64 = 3.0;

/// ...refilled at this many per second: three a minute. A save is a whole
/// wall (up to about 100 KB) written to disk; a seat keeping its work
/// saves now and then, never in a loop.
pub const SAVE_REFILL_PER_SECOND: f64 = 0.05;

/// The most saves made by hand a seat that is not a console may bring the
/// saves to.
///
/// Beyond it, only replacing one (Tom's console) or deleting some makes
/// room. With a full wall at about 100 KB, at most about 20 MB.
pub const MAX_SAVES: usize = 200;

/// Walls a seat that is not a console may put up in a burst...
///
/// A load, a new wall, or a load undone.
pub const LOAD_BURST: f64 = 3.0;

/// ...refilled at this many per second: three a minute.
///
/// Each replaces the whole wall and keeps the one it replaced in a save of
/// the wall's own;
/// a seat loading in a loop would otherwise push the console's way back
/// out of the saves kept.
pub const LOAD_REFILL_PER_SECOND: f64 = 0.05;

/// A seat's allowance of changes, or of listings of the saves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bucket {
    tokens: f64,
    last: Instant,
    burst: f64,
    refill: f64,
}

impl Bucket {
    /// A full bucket of changes at `now` ([`FLOOD_BURST`], refilled at
    /// [`FLOOD_REFILL_PER_SECOND`]).
    #[must_use]
    pub const fn new(now: Instant) -> Self {
        Self::of(now, FLOOD_BURST, FLOOD_REFILL_PER_SECOND)
    }

    /// A full bucket at `now` holding `burst`, refilled at `refill` a
    /// second.
    #[must_use]
    pub const fn of(now: Instant, burst: f64, refill: f64) -> Self {
        Self {
            tokens: burst,
            last: now,
            burst,
            refill,
        }
    }

    /// Give back what [`Self::take`] took, for something refused that cost
    /// nothing.
    pub const fn refund(&mut self) {
        self.tokens = (self.tokens + 1.0).min(self.burst);
    }

    /// Take one change's worth at `now`, if there is one.
    pub fn take(&mut self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.tokens = elapsed.mul_add(self.refill, self.tokens).min(self.burst);
        self.last = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Which engine slot holds which module.
#[derive(Debug, Clone, Default)]
struct Rack {
    slots: Vec<Option<(String, u32)>>,
    next_tag: u32,
}

impl Rack {
    fn new() -> Self {
        Self {
            slots: vec![None; MAX_MODULES],
            next_tag: 0,
        }
    }

    fn slot_of(&self, id: &str) -> Option<usize> {
        self.slots
            .iter()
            .position(|placed| placed.as_ref().is_some_and(|(placed, _)| placed == id))
    }

    /// Give `id` a free slot and a fresh tag.
    fn place(&mut self, id: &str) -> Option<(usize, u32)> {
        let slot = self.slots.iter().position(Option::is_none)?;
        self.next_tag = self.next_tag.wrapping_add(1);
        self.slots[slot] = Some((id.to_string(), self.next_tag));
        Some((slot, self.next_tag))
    }

    fn free(&mut self, id: &str) -> Option<usize> {
        let slot = self.slot_of(id)?;
        self.slots[slot] = None;
        Some(slot)
    }

    /// Free every slot, returning those that were taken. Tags carry on
    /// from where they were, so a fault from a module just taken out is
    /// never put down to the module that takes its slot.
    fn empty(&mut self) -> Vec<usize> {
        let mut taken = Vec::new();
        for (slot, placed) in self.slots.iter_mut().enumerate() {
            if placed.take().is_some() {
                taken.push(slot);
            }
        }
        taken
    }

    /// The module in `slot`.
    fn id_at(&self, slot: usize) -> Option<&str> {
        self.slots
            .get(slot)
            .and_then(Option::as_ref)
            .map(|(id, _)| id.as_str())
    }

    /// The module in `slot`, if it still carries `tag`.
    fn id_of(&self, slot: usize, tag: u32) -> Option<&str> {
        self.slots
            .get(slot)
            .and_then(Option::as_ref)
            .filter(|(_, placed)| *placed == tag)
            .map(|(id, _)| id.as_str())
    }
}

/// Faults counted and reported, at most one report per module per
/// [`FAULT_REPORT_INTERVAL`].
#[derive(Debug, Default)]
struct FaultLog {
    count: u64,
    recent: VecDeque<String>,
    last_report: HashMap<String, Instant>,
    unreported: HashMap<String, u64>,
    engine_seen: u64,
}

/// What a request did.
#[derive(Debug)]
pub struct Outcome {
    /// The response's result.
    pub result: serde_json::Value,
    /// The change it made, to tell the other seats.
    pub change: Option<Change>,
    /// What to tell every subscriber, the asker included, right after the
    /// change: the fingerprints it moved, or the rack's rows after a move.
    pub event: Option<Event>,
    /// For a request answered later (`speak`, while its words render): the
    /// ticket its answer will come back under from [`Wall::spoken`]. The
    /// result is empty until then.
    pub pending: Option<u64>,
}

/// The daemon's state.
#[derive(Debug)]
pub struct Wall {
    patch: Patch,
    rack: Rack,
    engine: EngineControl,
    desk: Option<HubLink>,
    log: VecDeque<Change>,
    next_seq: u64,
    /// The latest change made (and logged): what `revision` says. Behind
    /// `next_seq` while a number was given to a change that was not made.
    made: u64,
    store: Option<Store>,
    dirty: bool,
    last_save: Option<Instant>,
    faults: FaultLog,
    levels: (f32, f32),
    listen: Option<Listen>,
    /// What the engine's scopes and readouts hand on.
    probe_ring: Option<ProbeRing>,
    /// Each scope's and readout's latest capture, and when it came, by
    /// module id.
    readings: HashMap<String, (Instant, Capture)>,
    /// Cable tables sent to this engine (it counts those it takes in the
    /// same way: see [`ProbeFrame::cables`]).
    tables_sent: u64,
    /// What each scope's and readout's inputs come from (source slot,
    /// port and amount, by input), as the tables last sent have it.
    wiring: HashMap<String, Wiring>,
    /// The first table each scope's and readout's inputs are as they are
    /// now under, input by input: a capture measured under an earlier one
    /// is of what was plugged in before. Its first input (a scope's `a`,
    /// which its sweep follows; a readout's `in`) moved, it is let go
    /// whole; another, only what it says of that input.
    fresh_from: HashMap<String, [u64; MAX_INPUTS]>,
    fingerprints: Fingerprints,
    speech: Speech,
    recorder: Recorder,
    /// Changes the daemon made on its own (to a recording), for the next
    /// [`Self::tick`] to tell everyone.
    told: Vec<Event>,
    /// Latency and delays as the tables were last sent.
    timing: Timing,
    /// Each cable's delay line and the tag it owns it under, by cable id.
    lines: BTreeMap<u32, (u16, u32)>,
    /// The last tag given to a delay line.
    line_tags: u32,
    /// One prepared effect of each kind on the wall, and the rate it was
    /// prepared for, to ask a module's latency with its knobs applied.
    probes: HashMap<Kind, (u32, Box<dyn Effect>)>,
    /// The knob values each effect's latency is measured with, by module
    /// id: its knobs as they last settled.
    settled: HashMap<String, Vec<f32>>,
    /// Effect knobs turned by the request being answered, and how long each
    /// takes to settle (to glide home, or for a stepped knob, to land on
    /// its new step).
    turned: Vec<(String, usize, Duration)>,
    /// When each turned effect knob counts as settled, by module and knob.
    settling: HashMap<(String, usize), Instant>,
    buckets: HashMap<String, Bucket>,
    /// Each seat's allowance of listings of the saves.
    saves_buckets: HashMap<String, Bucket>,
    /// Each seat's allowance of saves (consoles have none: they are not
    /// held to it).
    save_buckets: HashMap<String, Bucket>,
    /// Each seat's allowance of walls put up (consoles have none: they are
    /// not held to it).
    load_buckets: HashMap<String, Bucket>,
    /// The daemon's own changes (a recording stopped or carried on, a
    /// change said not to have happened) that the log could not take yet,
    /// oldest first, by seat: unnumbered and untold until written, before
    /// anything else; until then no change is made and `patch.json` is not
    /// written.
    owed: VecDeque<(String, What)>,
    /// The wall up just before the first change kept in memory (see
    /// [`walls_of`]).
    memory_opens_on: u64,
    /// Why the log last failed, until a line is written again.
    log_error: Option<String>,
    /// The log's health as last told to everyone.
    log_told: LogHealth,
    /// Whether the wall is too large for its file, as last said (see
    /// [`Self::save`]).
    too_large: bool,
    /// Set when a change could not be logged and so was not made, for the
    /// request that made it to put the patch back (see [`Self::commit`]).
    unmade: bool,
    seats: BTreeMap<String, usize>,
    /// The save the wall was last saved as or loaded from, and what the
    /// wall was then ([`Print`]): `look` says it changed when the wall is
    /// no longer that (see [`Self::still`]), whatever the revision did.
    save_mark: Option<(SaveMark, Print)>,
    /// A save known to hold exactly the wall as it is, durably, if any,
    /// with what the wall was then. Replacing the wall while it still is
    /// that, and the save's file still has its bytes, keeps it in that save
    /// rather than writing another copy.
    kept_in: Option<(Kept, Print)>,
    /// Swaps of the whole wall sent to this engine (see
    /// [`crate::engine::Shared::unhushed`]): until the engine has taken
    /// them all in, its knobs are the old wall's, and `look` shows the
    /// targets instead.
    swaps_sent: u64,
}

/// What each input of a module comes from: the source's slot, its port,
/// and the cable's amount (as bits), or nothing plugged in.
type Wiring = [Option<(usize, usize, u32)>; MAX_INPUTS];

/// Say in `capture` that nothing is known of its `input` (re-plugged, and
/// not measured since): no reading, plugged in or not.
fn unmeasured(capture: &mut Capture, input: usize, plugged: bool) {
    if let Some(reading) = capture.inputs.get_mut(input) {
        *reading = Reading {
            connected: plugged,
            ..Reading::NONE
        };
    }
}

/// The reading end of the engine's probe ring.
struct ProbeRing(HeapCons<ProbeFrame>);

impl std::fmt::Debug for ProbeRing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProbeRing")
            .field("waiting", &self.0.occupied_len())
            .finish()
    }
}

fn error(code: ErrorCode, message: impl Into<String>) -> WallError {
    WallError::new(code, message)
}

/// A request's answer that changed nothing.
const fn unchanged(result: serde_json::Value) -> Outcome {
    Outcome {
        result,
        change: None,
        event: None,
        pending: None,
    }
}

fn encode(value: &impl serde::Serialize) -> Result<serde_json::Value, WallError> {
    serde_json::to_value(value).map_err(|err| {
        error(
            ErrorCode::Internal,
            format!("the result could not be encoded: {err}"),
        )
    })
}

/// The error for a change that could not be written to the log, and so was
/// not made.
fn unlogged(err: &std::io::Error) -> WallError {
    error(
        ErrorCode::Internal,
        format!(
            "the change log could not be written ({err}), so nothing was changed; the wall \
             plays on as it was, and tries the log again with the next change"
        ),
    )
}

/// The note for a save that is `done` ("in place", "gone") but whose
/// directory could not be flushed after (`unsynced`), so a crash could
/// `undo` it ("take it back"): nothing when it was flushed.
fn unsynced_note(unsynced: Option<&std::io::Error>, done: &str, undo: &str) -> Vec<String> {
    unsynced
        .map(|err| {
            eprintln!("kazoo-wall: the saves directory could not be flushed: {err}");
            format!(
                "the save is {done}, but its directory could not be flushed to the disk after \
                 ({err}), so a crash before the next flush could {undo}"
            )
        })
        .into_iter()
        .collect()
}

/// Why `seat`'s request to `how` (`load`, `new`, `undo`) replaced nothing,
/// as the wall as it is could not be kept in a save first (`err`).
fn not_kept(seat: &str, how: &str, err: &SaveError) -> WallError {
    eprintln!("kazoo-wall: {seat} asked to {how}, and the wall could not be kept first: {err}");
    match err {
        // Too large to keep, so too large to replace: nothing is lost.
        SaveError::TooLarge(_) => error(
            ErrorCode::Full,
            format!(
                "the wall as it is cannot be kept in a save first: {err}; so nothing was \
                 replaced (take some modules, cables or words away, then ask again)"
            ),
        ),
        _ => error(
            ErrorCode::Internal,
            format!("the wall as it is could not be saved first, so nothing was replaced: {err}"),
        ),
    }
}

/// Why `seat`'s request to `how` replaced nothing, as its batch stopped
/// before its line was written (`err`; see [`LoadWriteError`]).
fn not_written(seat: &str, how: &str, err: &LoadWriteError) -> WallError {
    eprintln!("kazoo-wall: {seat} asked to {how}, and nothing was written: {err}");
    match err {
        LoadWriteError::LogNotReady(err) => error(
            ErrorCode::Internal,
            format!(
                "the change log could not be made ready for the change ({err}), so nothing was \
                 replaced; the wall plays on as it was"
            ),
        ),
        LoadWriteError::TooLarge(_) => {
            error(ErrorCode::Full, format!("{err}, so nothing was replaced"))
        }
        LoadWriteError::NotLogged(io) | LoadWriteError::MaybeLogged(io) => error(
            ErrorCode::Internal,
            format!("the wall as it is could not be saved first, so nothing was replaced: {io}"),
        ),
    }
}

/// The error for a wall that keeps no state on disk (as in some tests).
fn no_saves() -> WallError {
    error(
        ErrorCode::Internal,
        "this wall keeps no state on disk, so it has no saves",
    )
}

/// A save on disk, recognised by its file: its name, and
/// [`crate::patch::hash_bytes`] of its file's bytes as written (before any
/// migration a later build would make of them).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Kept {
    name: String,
    hash: String,
}

/// What of the patch a request may touch before its change is logged,
/// kept so that a change the log refuses leaves the patch exactly as it
/// was.
///
/// A knob turn (the one change that comes many times a second, from a
/// knob dragged or swept) keeps only the module's knob targets, a few
/// floats; a structural change (a cable or a module added or taken
/// away, by hand, a few times a minute at most) keeps the whole patch,
/// as id and cable numbering, the rows and the words all move with it.
/// Everything else touches the patch only once its change is logged.
#[derive(Debug)]
enum Before {
    /// Nothing to put back.
    Nothing,
    /// A module's knob targets, by its id.
    Knobs(String, Vec<f32>),
    /// The whole patch.
    Whole(Box<Patch>),
}

/// What the wall was when it went to or came from a save, to tell later
/// whether it still is: its [`Patch::fingerprint_but_tempo`] and its
/// tempo apart, as the tempo is the desk's while the desk keeps it.
#[derive(Debug, Clone, PartialEq)]
struct Print {
    body: String,
    tempo: f64,
}

impl Print {
    fn of(patch: &Patch) -> Self {
        Self {
            body: patch.fingerprint_but_tempo(),
            tempo: patch.tempo,
        }
    }
}

/// Where a wall that replaces the one up comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Source {
    /// This save.
    Save(Kept),
    /// Nowhere: an empty wall.
    Empty,
    /// Undoing load `load`: the wall it replaced, from the save it was
    /// kept in.
    Undo {
        /// The load undone.
        load: u64,
        /// The save that held the wall.
        kept: Kept,
    },
}

impl Source {
    /// How the automatic save before it is named: `before-<how>-…`.
    const fn how(&self) -> &'static str {
        match self {
            Self::Save(_) => "load",
            Self::Empty => "new",
            Self::Undo { .. } => "undo",
        }
    }

    /// The save loaded, if one was.
    fn save(&self) -> Option<&str> {
        match self {
            Self::Save(kept) => Some(&kept.name),
            Self::Empty | Self::Undo { .. } => None,
        }
    }

    /// The load undone, if one was.
    const fn undoes(&self) -> Option<u64> {
        match self {
            Self::Undo { load, .. } => Some(*load),
            Self::Save(_) | Self::Empty => None,
        }
    }

    /// The save that holds exactly the wall put up, if one does.
    const fn kept(&self) -> Option<&Kept> {
        match self {
            Self::Save(kept) | Self::Undo { kept, .. } => Some(kept),
            Self::Empty => None,
        }
    }
}

/// Which wall each change in memory was made to, and how the walls came
/// from each other.
///
/// Every load (and every new wall, and every undone load) puts up a wall
/// of its own, named by the load's number; 0 is the wall the log starts
/// on. A wall put up by undoing load L is a copy of the wall L replaced as
/// it stood just before L, so the changes made to that wall before L hold
/// on the copy too, and nothing else does. Undoing a change is allowed
/// only on a wall it holds on: a change aimed at another wall would land on
/// modules that only share its modules' ids.
#[derive(Debug, Default)]
struct Walls {
    /// The wall up when each change was made, by change number.
    made_to: HashMap<u64, u64>,
    /// For a wall put up by undoing a load: the wall that load replaced,
    /// and the load's number (the copy is of that wall before it).
    copies: HashMap<u64, (u64, u64)>,
    /// The wall up now.
    up: u64,
}

impl Walls {
    /// Whether change `seq` holds on the wall up now: it was made to it,
    /// or to a wall it is a copy of, before the copy was taken (and so on
    /// back through copies of copies).
    fn holds(&self, seq: u64) -> bool {
        let Some(&made_to) = self.made_to.get(&seq) else {
            return false;
        };
        let mut wall = self.up;
        let mut before = u64::MAX;
        loop {
            if made_to == wall && seq < before {
                return true;
            }
            // Each copy is of a wall put up before it: this ends.
            let Some(&(original, at)) = self.copies.get(&wall) else {
                return false;
            };
            wall = original;
            before = before.min(at);
        }
    }
}

/// The changes said not to have happened ([`What::Abandoned`]) among
/// `changes`.
fn abandoned_changes<'a>(changes: impl Iterator<Item = &'a Change>) -> HashSet<u64> {
    changes
        .filter_map(|change| match change.what {
            What::Abandoned { change } => Some(change),
            _ => None,
        })
        .collect()
}

/// A wall no change is numbered by: the one the log read starts on when it
/// puts up walls of its own, and the wall before it has gone with older
/// logs.
const UNNAMED_WALL: u64 = u64::MAX;

/// The wall up before the first change of `history` (all of the log that
/// was read, oldest first), for a wall now on `patch`.
///
/// Wall 0 when the log starts at the first change. Else the first change
/// in it that puts up a wall says: none, and it is the wall `patch.json`
/// names (its own load gone with older logs); the wall starting again on
/// a wall numbered before the log read ([`What::Resumed`] with such a
/// `from`), and it is that wall, which the changes before the note were
/// made to; a load or anything else, and it is one this log cannot name,
/// which no number holds.
fn log_opens_on(patch: &Patch, history: &[Change]) -> u64 {
    let Some(first) = history.first().filter(|first| first.seq > 1) else {
        return 0;
    };
    let abandoned = abandoned_changes(history.iter());
    let opening = history.iter().find(|change| match change.what {
        What::Load { .. } => !abandoned.contains(&change.seq),
        What::Resumed { .. } => true,
        _ => false,
    });
    match opening.map(|change| &change.what) {
        None => patch.wall,
        Some(What::Resumed {
            from: Some(wall), ..
        }) if *wall < first.seq => *wall,
        Some(_) => UNNAMED_WALL,
    }
}

/// Which wall each of `changes` (oldest first) was made to, and how the
/// walls came from each other (see [`Walls`]), with wall `start` up before
/// the first. A load that did not happen puts up nothing. The wall started
/// again on ([`What::Resumed`]) is a wall of its own: a copy of the wall
/// `patch.json` held as it stood after the change it had, when the log
/// knows that wall.
fn walls_of<'a>(start: u64, changes: impl Iterator<Item = &'a Change> + Clone) -> Walls {
    let abandoned = abandoned_changes(changes.clone());
    let mut walls = Walls {
        up: start,
        ..Walls::default()
    };
    for change in changes {
        walls.made_to.insert(change.seq, walls.up);
        match &change.what {
            What::Load { .. } if !abandoned.contains(&change.seq) => {
                // Every load puts up a wall of its own, under its number;
                // undoing load L puts up a copy of the wall L replaced, as
                // it stood just before L.
                if let Some(undone) = change.undoes {
                    if let Some(&replaced) = walls.made_to.get(&undone) {
                        walls.copies.insert(change.seq, (replaced, undone));
                    }
                }
                walls.up = change.seq;
            }
            What::Resumed { from, through, .. } => {
                if let Some(from) = from {
                    walls
                        .copies
                        .insert(change.seq, (*from, through.saturating_add(1)));
                }
                walls.up = change.seq;
            }
            _ => {}
        }
    }
    walls
}

/// What the daemon must say as it starts on `patch` with the change log
/// `history` (oldest first), if anything.
///
/// A [`What::Resumed`] when `patch.json` is not the wall the log ends on.
/// That is when the log has
/// changes `patch.json` does not (it stopped before saving them, or the
/// file was restored from an older copy), when the log has loads after
/// the wall `patch.json` holds, when that wall is one the log does not
/// know (a wall numbered before the oldest change read is taken as known),
/// and when the wall was `seeded` afresh. Loads said not to have
/// happened, and loads an earlier note left out already, are not left out
/// again.
#[must_use]
pub fn resumed(patch: &Patch, history: &[Change], seeded: bool) -> Option<What> {
    let last_logged = history.last()?.seq;
    let abandoned = abandoned_changes(history.iter());
    let said: HashSet<u64> = history
        .iter()
        .flat_map(|change| match &change.what {
            What::Resumed { left_out, .. } => left_out.clone(),
            _ => Vec::new(),
        })
        .collect();
    // A wall put up before the oldest change read (its load gone with
    // older logs, on a wall that lived long) is taken as known: the log
    // can no longer say otherwise.
    let known = patch.wall == 0
        || history.first().is_some_and(|first| patch.wall < first.seq)
        || history.iter().any(|change| {
            change.seq == patch.wall
                && match change.what {
                    What::Load { .. } => !abandoned.contains(&change.seq),
                    What::Resumed { .. } => true,
                    _ => false,
                }
        });
    let from = (!seeded && known).then_some(patch.wall);
    // On a wall from outside the log, no load in it put up what plays.
    let after = from.unwrap_or(0);
    let left_out: Vec<u64> = history
        .iter()
        .filter(|change| {
            matches!(change.what, What::Load { .. })
                && change.seq > after
                && !abandoned.contains(&change.seq)
                && !said.contains(&change.seq)
        })
        .map(|change| change.seq)
        .collect();
    let behind = patch.last_seq < last_logged;
    (from.is_none() || behind || !left_out.is_empty()).then_some(What::Resumed {
        from,
        through: patch.last_seq,
        left_out,
    })
}

/// Log what the daemon must say as it starts, and save `patch` as that
/// leaves it.
///
/// First that `patch.json` could not be used and was moved aside, when it
/// was (`set_aside`: why, and the file it went to; see
/// [`What::SetAside`]); then what [`resumed`] says; then what the daemon
/// did to `patch` as it
/// loaded it (`migrated`: the file's version and the notes, see
/// [`crate::migrate`]), as changes by the seat `kazoo-wall`, each synced.
/// Each is added to `history`, and said in the daemon's log. This is the
/// only place a start writes `patch.json` (a seeded wall, or one brought
/// up to date, included), and only after the notes: a start stopped
/// before then leaves the file as it found it, to be read the same way
/// again next time.
///
/// # Errors
///
/// Fails if a note cannot be written to the log, or `patch.json` cannot
/// be written after: the wall must not start then, as it would play a
/// wall the log does not account for. A wall too large for its file is no
/// reason not to start: why, when it is, is returned, for the wall to say
/// to everyone (see [`Wall::starts_too_large`]).
pub fn resume(
    store: &mut Store,
    patch: &mut Patch,
    history: &mut Vec<Change>,
    seeded: bool,
    set_aside: Option<(String, String)>,
    migrated: Option<(u32, Vec<String>)>,
) -> std::io::Result<Option<String>> {
    let notes: Vec<What> = set_aside
        .map(|(reason, moved_to)| What::SetAside { reason, moved_to })
        .into_iter()
        .chain(resumed(patch, history, seeded))
        .chain(migrated.map(|(from_version, notes)| What::Migrate {
            from_version,
            to_version: FILE_VERSION,
            notes,
        }))
        .collect();
    // A seeded wall, or one brought up to date, is written here and only
    // here, after the notes that account for it.
    if notes.is_empty() && !seeded {
        return Ok(None);
    }
    for what in notes {
        let last_logged = history.last().map_or(0, |change| change.seq);
        let seq = patch.last_seq.max(last_logged) + 1;
        let change = Change {
            seq,
            at: utc_now(),
            seat: LOADER_SEAT.to_string(),
            summary: summarise_with(LOADER_SEAT, &what, None, |id| {
                patch.module(id).map(|m| m.kind)
            }),
            what,
            undoes: None,
        };
        store.append_synced(&change)?;
        eprintln!("kazoo-wall: {}", change.summary);
        patch.last_seq = seq;
        if matches!(change.what, What::Resumed { .. }) {
            patch.wall = seq;
        }
        history.push(change);
    }
    match store.save_patch(patch) {
        Ok(()) => Ok(None),
        // Not a reason to stay silent: the wall plays, and says to everyone
        // that it is not being kept (see `Wall::starts_too_large`).
        Err(err) if err.kind() == std::io::ErrorKind::FileTooLarge => Ok(Some(err.to_string())),
        Err(err) => Err(err),
    }
}

/// `bad_name` unless `name` is a save's name (see [`valid_save_name`]).
fn check_save_name(name: &str) -> Result<(), WallError> {
    if valid_save_name(name) {
        Ok(())
    } else {
        Err(error(
            ErrorCode::BadName,
            format!(
                "'{}' is not a save's name: use 1 to {MAX_SAVE_NAME} of A-Z a-z 0-9 space _ . -, \
                 starting with a letter or digit and not ending in a space or .",
                printable(name)
            ),
        ))
    }
}

/// A save that could not be read, written or deleted, as the protocol says
/// it.
fn save_error(err: &SaveError) -> WallError {
    match err {
        SaveError::Unknown(_) => error(ErrorCode::UnknownSave, format!("{err}; saves lists them")),
        SaveError::Exists(_) => error(
            ErrorCode::SaveExists,
            format!("{err}: choose another name (only a console can replace a save)"),
        ),
        SaveError::Broken { .. } => error(
            ErrorCode::BrokenSave,
            format!("{err}; its file is left as it is"),
        ),
        SaveError::Io(io) => error(
            ErrorCode::Internal,
            format!("the saves could not be used: {io}"),
        ),
        SaveError::TooLarge(_) => error(
            ErrorCode::Full,
            format!("{err}; take some modules, cables or words away first"),
        ),
    }
}

/// `at` in UTC, as `2026-09-26T12:00:01Z` (a time before 1970 reads as
/// 1970).
fn utc_of(at: SystemTime) -> String {
    utc(at
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs()))
}

/// The save `name` holding `patch`, saved at `at` in a file of `version`.
fn save_info(name: &str, patch: &Patch, at: SystemTime, version: u32) -> SaveInfo {
    SaveInfo {
        name: name.to_string(),
        saved_at: utc_of(at),
        tempo: patch.tempo,
        modules: u32::try_from(patch.modules().len()).unwrap_or(u32::MAX),
        cables: u32::try_from(patch.cables().len()).unwrap_or(u32::MAX),
        auto: is_auto_save(name),
        version,
    }
}

impl Wall {
    /// A wall playing `patch` on `engine`, remembering `history` (oldest
    /// first) and saving to `store`.
    #[must_use]
    pub fn new(
        patch: Patch,
        mut engine: EngineControl,
        desk: Option<HubLink>,
        history: Vec<Change>,
        store: Option<Store>,
    ) -> Self {
        let last_logged = history.last().map_or(0, |change| change.seq);
        let next_seq = patch.last_seq.max(last_logged) + 1;
        let skip = history.len().saturating_sub(MEMORY_CHANGES);
        let start = log_opens_on(&patch, &history);
        let memory_opens_on = walls_of(start, history[..skip].iter()).up;
        // Rendered speech is cached with the wall's state, or (with no
        // state to keep, as in tests) in a directory of this process's own.
        let speech_cache = store.as_ref().map_or_else(
            || std::env::temp_dir().join(format!("kazoo-wall-speech-{}", std::process::id())),
            |store| store.dir().join("speech"),
        );
        // Recordings likewise, until the daemon says where they go.
        let recordings = store.as_ref().map_or_else(
            || std::env::temp_dir().join(format!("kazoo-wall-recordings-{}", std::process::id())),
            |store| store.dir().join("recordings"),
        );
        let recorder = Recorder::new(recordings, &mut engine);
        let probe_ring = engine.take_probes().map(ProbeRing);
        let mut wall = Self {
            speech: Speech::new(speech_cache),
            recorder,
            told: Vec::new(),
            timing: Timing::default(),
            lines: BTreeMap::new(),
            line_tags: 0,
            probes: HashMap::new(),
            settled: HashMap::new(),
            turned: Vec::new(),
            settling: HashMap::new(),
            fingerprints: patch.dye.flow(&patch),
            patch,
            rack: Rack::new(),
            engine,
            desk,
            log: history.into_iter().skip(skip).collect(),
            next_seq,
            made: next_seq - 1,
            store,
            dirty: false,
            last_save: None,
            faults: FaultLog::default(),
            levels: (0.0, 0.0),
            listen: None,
            probe_ring,
            readings: HashMap::new(),
            tables_sent: 0,
            wiring: HashMap::new(),
            fresh_from: HashMap::new(),
            buckets: HashMap::new(),
            seats: BTreeMap::new(),
            save_mark: None,
            kept_in: None,
            save_buckets: HashMap::new(),
            saves_buckets: HashMap::new(),
            load_buckets: HashMap::new(),
            memory_opens_on,
            owed: VecDeque::new(),
            log_error: None,
            log_told: LogHealth::default(),
            too_large: false,
            unmade: false,
            swaps_sent: 0,
        };
        wall.load_engine();
        wall
    }

    /// Move to a new engine (and desk link), after the audio backend was
    /// rebuilt: every module goes in afresh, knobs at their targets. A
    /// recording under way ends with the old engine, and carries on in a
    /// new file on the new one (whose rate may differ); both are logged,
    /// and told at the next [`Self::tick`].
    pub fn attach(&mut self, mut engine: EngineControl, desk: Option<HubLink>) {
        let before = self.recorder.sample_rate();
        let stopped = self.recorder.attach(&mut engine);
        self.probe_ring = engine.take_probes().map(ProbeRing);
        self.engine = engine;
        self.desk = desk;
        // A new engine has no swap to take in, and has taken in no cable
        // table: everything goes in afresh.
        self.swaps_sent = 0;
        self.tables_sent = 0;
        self.wiring.clear();
        self.fresh_from.clear();
        self.load_engine();
        if let Some(finished) = stopped {
            let rate = self.engine.sample_rate();
            let why = if rate == before {
                format!("the audio restarted at {rate} Hz")
            } else {
                format!("the audio restarted at {rate} Hz (it was {before} Hz)")
            };
            self.carry_on(&finished, &why, true);
        }
    }

    /// The wall started too large for its file (`why`, see [`resume`]):
    /// said to everyone at the first tick, and held as for a save that
    /// found it so (see [`Self::save`]).
    pub fn starts_too_large(&mut self, why: &str) {
        self.dirty = true;
        self.say_too_large(why);
    }

    /// The wall is too large for its file (`why`): from now on changes
    /// that would make it larger are refused (see [`Self::grows`]), and it
    /// is said once, to everyone, that changes are not being kept.
    fn say_too_large(&mut self, why: &str) {
        if self.too_large {
            return;
        }
        self.too_large = true;
        let summary = format!(
            "the wall is too large to keep, so changes are not being kept: {why}; changes that \
             would make it larger are refused until something is taken away"
        );
        eprintln!("kazoo-wall: {summary}");
        self.told.push(Event::Fault {
            summary,
            seq: Some(self.revision()),
        });
    }

    /// Put recordings in `dir` from the next one on.
    pub fn set_recordings_dir(&mut self, dir: std::path::PathBuf) {
        self.recorder.set_dir(dir);
    }

    /// The recorder, to adjust it.
    pub const fn recorder_mut(&mut self) -> &mut Recorder {
        &mut self.recorder
    }

    /// The engine's control half.
    pub const fn engine_mut(&mut self) -> &mut EngineControl {
        &mut self.engine
    }

    /// The patch.
    #[must_use]
    pub const fn patch(&self) -> &Patch {
        &self.patch
    }

    /// The store, for a test to make a step of writing fail (see
    /// [`Store::fail_once`]).
    #[cfg(test)]
    pub(crate) const fn store_mut(&mut self) -> Option<&mut Store> {
        self.store.as_mut()
    }

    fn load_engine(&mut self) {
        self.rack = Rack::new();
        // What the old engine's modules measured is gone with them.
        self.readings.clear();
        // Every knob goes in at its target, with nothing gliding.
        self.settled.clear();
        self.settling.clear();
        let modules = self.patch.modules().to_vec();
        for module in &modules {
            if self.insert(module) {
                self.put_back_words(&module.id);
            }
        }
        self.send_tables();
        // An engine started afresh (after the audio was rebuilt, maybe
        // mid-swap) plays the patch's own tempo, not the one it was
        // started at, unless the desk keeps the tempo.
        if !self.on_desk() {
            self.engine.send(Command::Tempo(self.patch.tempo));
        }
    }

    /// Swap the wall the engine plays for the patch, as it plays: the
    /// master fades out, every module comes out and the patch's go in,
    /// knobs at their targets and nothing gliding, with fresh delay lines
    /// for every cable, and the master fades back in (see
    /// [`Command::Hush`]). The engine's clock, the desk link and a
    /// recording under way carry on through it; the wall's own tempo
    /// becomes the patch's, unless the desk keeps the tempo.
    fn swap_engine(&mut self) {
        self.engine.send(Command::Hush);
        for slot in self.rack.empty() {
            self.engine.send(Command::Remove { slot });
        }
        self.speech.replace_wall();
        self.lines.clear();
        self.settled.clear();
        self.settling.clear();
        // Faults not yet told, and readings, belong to modules that have
        // gone.
        self.faults.unreported.clear();
        self.faults.last_report.clear();
        self.readings.clear();
        let modules = self.patch.modules().to_vec();
        for module in &modules {
            if self.insert(module) {
                self.put_back_words(&module.id);
            }
        }
        self.send_tables();
        if !self.on_desk() {
            self.engine.send(Command::Tempo(self.patch.tempo));
        }
        self.engine.send(Command::Unhush);
        self.swaps_sent += 1;
    }

    /// Whether the desk is linked, and so keeps the tempo.
    fn on_desk(&self) -> bool {
        self.desk.as_ref().is_some_and(HubLink::is_connected)
    }

    /// Whether the wall is still what `print` says it was: everything the
    /// same, the tempo too unless the desk keeps it (the wall's saved
    /// tempo then follows the desk's, which is no change of the wall's).
    fn still(&self, print: &Print) -> bool {
        let now = Print::of(&self.patch);
        !now.body.is_empty()
            && now.body == print.body
            && (self.on_desk() || (now.tempo - print.tempo).abs() < 1e-9)
    }

    /// Render a speaker's saved words again (its phrase is gone, or was
    /// made for another sample rate).
    fn put_back_words(&mut self, module: &str) {
        let Some(words) = self.patch.speech.get(module).cloned() else {
            return;
        };
        let rate = self.engine.sample_rate();
        self.speech.put_back(module, words, rate);
    }

    /// Put `module` in the engine. Returns whether it is a speaker that
    /// needs its words rendered.
    fn insert(&mut self, module: &Module) -> bool {
        let Some((slot, tag)) = self.rack.place(&module.id) else {
            // The patch holds at most MAX_MODULES modules, and the rack as
            // many slots: never expected.
            eprintln!("kazoo-wall: no engine slot for {}", module.id);
            return false;
        };
        let mut knobs = [0.0; MAX_KNOBS];
        for (target, value) in knobs.iter_mut().zip(&module.knobs) {
            *target = *value;
        }
        // The engine's rate is at most a few hundred kHz: exact in f32.
        let rate = self.engine.sample_rate() as f32;
        let speaks = matches!(module.kind.spec().build, Builder::Adapted(Adapter::Speak));
        let (built, needs_words) = if speaks {
            let (built, feed) = adapters::build_speaker(rate);
            let needs = self.speech.attach(&module.id, feed);
            (built, needs)
        } else {
            (dsp::build(module.kind, rate), false)
        };
        self.engine.send(Command::Insert {
            slot,
            tag,
            kind: module.kind,
            module: built,
            knobs,
        });
        needs_words
    }

    fn send_tables(&mut self) {
        let mut edges = Vec::with_capacity(self.patch.cables().len());
        for cable in self.patch.cables() {
            if let (Some(from), Some(to)) = (
                self.rack.slot_of(&cable.from_module),
                self.rack.slot_of(&cable.to_module),
            ) {
                edges.push((from, to));
            }
        }
        let slots: Vec<usize> = self
            .patch
            .modules()
            .iter()
            .filter_map(|module| self.rack.slot_of(&module.id))
            .collect();
        let order: Order = processing_order(&slots, &edges);
        let rendered: Vec<String> = order
            .slots()
            .iter()
            .filter_map(|slot| self.rack.id_at(usize::from(*slot)).map(str::to_string))
            .collect();
        let latencies = self.latencies();
        self.timing = timing(&self.patch, &rendered, |id| {
            latencies.get(id).copied().unwrap_or(0)
        });
        self.timing.unsteady = self.unsteady();
        self.assign_lines();
        let mut table = CableTable::new();
        for cable in self.patch.cables() {
            let (Some(from), Some(to), Some(&(line, tag))) = (
                self.rack.slot_of(&cable.from_module),
                self.rack.slot_of(&cable.to_module),
                self.lines.get(&cable.id),
            ) else {
                continue;
            };
            let jack = match cable.to_jack {
                Jack::Input(index) => index,
                Jack::Knob(index) => MAX_INPUTS + index,
            };
            let route = Route {
                delay: self.timing.delays.get(&cable.id).copied().unwrap_or(0),
                gate: self.carries_gate(cable),
                // Slots and ports are below MAX_MODULES (at most 256) and 4: they fit.
                ..Route::new(from as u8, cable.from_port as u8, cable.amount, line, tag)
            };
            table.set(to, jack, Some(route));
        }
        self.engine.send(Command::Cables(Box::new(table)));
        self.engine.send(Command::Order(Box::new(order)));
        self.tables_sent = self.tables_sent.wrapping_add(1);
        self.note_rewiring();
    }

    /// Note each scope and readout whose inputs the table just sent moves
    /// (see [`Self::fresh_from`]), letting go of what it measured of what
    /// was plugged in before.
    fn note_rewiring(&mut self) {
        let mut rewired = Vec::new();
        for module in self.patch.modules() {
            if !probe::measures(module.kind) {
                continue;
            }
            let mut wiring = [None; MAX_INPUTS];
            for cable in self.patch.cables() {
                if let (true, Jack::Input(input), Some(from)) = (
                    cable.to_module == module.id,
                    cable.to_jack,
                    self.rack.slot_of(&cable.from_module),
                ) {
                    if let Some(entry) = wiring.get_mut(input) {
                        *entry = Some((from, cable.from_port, cable.amount.to_bits()));
                    }
                }
            }
            let before = self.wiring.get(&module.id);
            let moved: [bool; MAX_INPUTS] =
                std::array::from_fn(|input| before.is_none_or(|old| old[input] != wiring[input]));
            if moved.contains(&true) {
                rewired.push((module.id.clone(), wiring, moved));
            }
        }
        for (id, wiring, moved) in rewired {
            let fresh = self.fresh_from.entry(id.clone()).or_insert([0; MAX_INPUTS]);
            for (from, moved) in fresh.iter_mut().zip(moved) {
                if moved {
                    *from = self.tables_sent;
                }
            }
            if moved[0] {
                self.readings.remove(&id);
            } else if let Some((_, capture)) = self.readings.get_mut(&id) {
                for (input, moved) in moved.iter().enumerate() {
                    if *moved {
                        unmeasured(capture, input, wiring[input].is_some());
                    }
                }
            }
            self.wiring.insert(id, wiring);
        }
        let patch = &self.patch;
        self.wiring.retain(|id, _| patch.module(id).is_some());
        self.fresh_from.retain(|id, _| patch.module(id).is_some());
    }

    /// Whether `cable` carries a gate (it comes from a gate output, or goes
    /// into a gate input), which must switch delay between pulses rather
    /// than fade.
    fn carries_gate(&self, cable: &Cable) -> bool {
        let from_gate = self.patch.module(&cable.from_module).is_some_and(|module| {
            module
                .kind
                .spec()
                .outputs
                .get(cable.from_port)
                .map(|port| port.signal)
                == Some(Signal::Gate)
        });
        from_gate
            || match cable.to_jack {
                Jack::Input(index) => self.patch.module(&cable.to_module).is_some_and(|module| {
                    module.kind.spec().inputs.get(index).map(|port| port.signal)
                        == Some(Signal::Gate)
                }),
                Jack::Knob(_) => false,
            }
    }

    /// Give every cable a delay line of its own, kept for its life: a cable
    /// keeps its line and tag across table swaps, a new cable takes the
    /// lowest free line under a tag never used before.
    fn assign_lines(&mut self) {
        let present: Vec<u32> = self.patch.cables().iter().map(|cable| cable.id).collect();
        self.lines.retain(|id, _| present.contains(id));
        for id in present {
            if self.lines.contains_key(&id) {
                continue;
            }
            let taken: Vec<u16> = self.lines.values().map(|(line, _)| *line).collect();
            // The wall holds at most MAX_CABLES cables: a line is free.
            let lines = u16::try_from(MAX_CABLES).unwrap_or(u16::MAX);
            let Some(line) = (0..lines).find(|line| !taken.contains(line)) else {
                continue;
            };
            self.line_tags = self.line_tags.wrapping_add(1).max(1);
            self.lines.insert(id, (line, self.line_tags));
        }
    }

    /// Every module's latency in frames, with its knobs as they last
    /// settled (modules with none are left out). Probes of kinds no longer
    /// on the wall, and settled knobs of modules no longer on it, are let
    /// go.
    fn latencies(&mut self) -> BTreeMap<String, u32> {
        let rate = self.engine.sample_rate();
        let modules = self.patch.modules();
        self.probes
            .retain(|kind, _| modules.iter().any(|module| module.kind == *kind));
        self.settled
            .retain(|id, _| modules.iter().any(|module| module.id == *id));
        let mut latencies = BTreeMap::new();
        for module in modules {
            let knobs = self
                .settled
                .entry(module.id.clone())
                .or_insert_with(|| module.knobs.clone());
            let latency = latency_of(&mut self.probes, module.kind, knobs, rate);
            if latency > 0 {
                latencies.insert(module.id.clone(), latency);
            }
        }
        latencies
    }

    /// Effects whose latency follows a knob with a cable in it: the
    /// cable moves the latency as it plays, faster than any re-timing, so
    /// they are compensated only for the knob as set.
    fn unsteady(&mut self) -> Vec<String> {
        let rate = self.engine.sample_rate();
        let mut unsteady = Vec::new();
        for module in self.patch.modules() {
            let spec = module.kind.spec();
            let Some(knobs) = self.settled.get(&module.id) else {
                continue;
            };
            let patched = self
                .patch
                .cables()
                .iter()
                .filter_map(|cable| match cable.to_jack {
                    Jack::Knob(index) if cable.to_module == module.id => Some(index),
                    Jack::Knob(_) | Jack::Input(_) => None,
                });
            let mut moves = false;
            for index in patched {
                let Some(knob) = spec.knobs.get(index) else {
                    continue;
                };
                let base = latency_of(&mut self.probes, module.kind, knobs, rate);
                let mut varied = knobs.clone();
                moves |= [0.0, 0.25, 0.5, 0.75, 1.0].iter().any(|position| {
                    if let Some(value) = varied.get_mut(index) {
                        *value = knob.denormalise(*position);
                    }
                    latency_of(&mut self.probes, module.kind, &varied, rate) != base
                });
            }
            if moves {
                unsteady.push(module.id.clone());
            }
        }
        unsteady
    }

    /// Take in the effect knobs that have settled by `now`, and re-time
    /// the cables if that moved any latency.
    fn settle(&mut self, now: Instant) {
        let due: Vec<(String, usize)> = self
            .settling
            .iter()
            .filter(|(_, at)| now >= **at)
            .map(|(key, _)| key.clone())
            .collect();
        if due.is_empty() {
            return;
        }
        for key in due {
            self.settling.remove(&key);
            let (module, knob) = key;
            let target = self
                .patch
                .module(&module)
                .and_then(|m| m.knobs.get(knob).copied());
            let settled = self
                .settled
                .get_mut(&module)
                .and_then(|knobs| knobs.get_mut(knob));
            if let (Some(target), Some(settled)) = (target, settled) {
                *settled = target;
            }
        }
        self.retime_now();
    }

    /// Measure the effects' latencies again, and re-time the cables if any
    /// moved.
    fn retime_now(&mut self) {
        if self.latencies() != self.timing.latency {
            self.send_tables();
        }
    }

    /// Mirror a change into the engine.
    fn mirror(&mut self, what: &What) {
        match what {
            What::Turn {
                module,
                knob,
                from,
                to,
                glide_beats,
            } => {
                let slot = self.rack.slot_of(module);
                let index = self
                    .patch
                    .module(module)
                    .and_then(|m| m.kind.spec().knob_index(knob));
                if let (Some(slot), Some(knob)) = (slot, index) {
                    let seconds = glide_beats * 60.0 / self.engine.shared().bpm();
                    let rate = f64::from(self.engine.sample_rate().max(1));
                    // `max` turns a NaN (a nonsense logged glide) into none.
                    let frames = (seconds * rate).max(0.0).min(f64::from(u32::MAX));
                    self.engine.send(Command::Knob {
                        slot,
                        knob,
                        // Knob targets are held to f32 ranges.
                        target: *to as f32,
                        glide_frames: frames as u32,
                    });
                    // An effect's latency may follow its knobs: measure it
                    // again once this one has settled.
                    let spec = self
                        .patch
                        .module(module)
                        .filter(|m| matches!(m.kind.spec().build, Builder::Effect(_)))
                        .and_then(|m| m.kind.spec().knobs.get(knob));
                    if let Some(spec) = spec {
                        let landing = landing(spec, *from, *to);
                        // Finite and at most u32::MAX / rate seconds.
                        let settles = Duration::from_secs_f64(frames * landing / rate);
                        self.turned.push((module.clone(), knob, settles));
                    }
                }
            }
            What::Patch { .. } | What::Unpatch { .. } => self.send_tables(),
            What::Add { module } | What::Restore { module, .. } => {
                if let Some(module) = self.patch.module(&module.id).cloned() {
                    // A new speaker has no words yet; a speaker brought
                    // back has its old ones again.
                    if self.insert(&module) {
                        self.put_back_words(&module.id);
                    }
                }
                self.send_tables();
            }
            What::Remove { module, .. } => {
                self.speech.forget(&module.id);
                // What it measured goes with it: brought back, it measures
                // afresh.
                self.readings.remove(&module.id);
                let slot = self.rack.free(&module.id);
                self.send_tables();
                if let Some(slot) = slot {
                    self.engine.send(Command::Remove { slot });
                }
            }
            // Applied before the engine was loaded, (new words) already
            // handed to the player, or (a recording) started or stopped by
            // the recorder itself: nothing to mirror.
            What::Migrate { .. }
            | What::Speak { .. }
            | What::Record { .. }
            | What::Abandoned { .. }
            | What::Resumed { .. }
            | What::SetAside { .. }
            | What::Unknown => {}
            What::Tempo { to, desk, .. } => {
                if !desk {
                    self.engine.send(Command::Tempo(*to));
                }
            }
            // The patch is the new wall already.
            What::Load { .. } => self.swap_engine(),
        }
    }

    /// Log `what` as `seat`'s change, mirror it into the engine, and move
    /// the fingerprints. Returns the change and, if the fingerprints moved,
    /// the event saying how.
    ///
    /// # Errors
    ///
    /// `internal` when the log cannot be written (the daemon's own changes
    /// owed to it first, or this one): nothing is changed here then, the
    /// wall is marked `unmade`, and the caller puts the patch back as it
    /// was (see [`Self::request_as`]).
    fn commit(
        &mut self,
        seat: &str,
        what: What,
        undoes: Option<u64>,
    ) -> Result<(Change, Option<Event>), WallError> {
        let change = self.next_change(seat, what, undoes)?;
        self.record(&change)?;
        Ok(self.apply(change))
    }

    /// `what`, by `seat`, as the next change, once the daemon's own changes
    /// owed to the log are written (they take their numbers first).
    ///
    /// # Errors
    ///
    /// `internal` when they cannot be: the wall is marked `unmade` for the
    /// request to put the patch back.
    fn next_change(
        &mut self,
        seat: &str,
        what: What,
        undoes: Option<u64>,
    ) -> Result<Change, WallError> {
        if let Err(err) = self.pay_owed() {
            self.unmade = true;
            return Err(err);
        }
        Ok(self.new_change(seat, what, undoes))
    }

    /// Write `change` (from [`Self::next_change`]) to the log, without
    /// making it: the caller makes it (see [`Self::apply`]) once whatever
    /// else it hangs on has gone through, or calls [`Self::not_made`] if
    /// that fails.
    ///
    /// # Errors
    ///
    /// `internal` when the log cannot be written: the wall is marked
    /// `unmade` for the request to put the patch back, and a change whose
    /// line may be on disk is said not to have happened.
    fn record(&mut self, change: &Change) -> Result<(), WallError> {
        let Some(store) = self.store.as_mut() else {
            return Ok(());
        };
        match store.append(change) {
            Ok(()) => {
                self.log_error = None;
                Ok(())
            }
            Err(err) => {
                self.unmade = true;
                eprintln!(
                    "kazoo-wall: change {} could not be logged, so it was not made: {err}",
                    change.seq
                );
                self.log_error = Some(err.to_string());
                self.not_made(change.seq);
                Err(unlogged(&err))
            }
        }
    }

    /// Change `seq` was not made, though its line may be in the log (a
    /// failed write may leave all of it but the end, which reads whole; a
    /// failed sync may leave all of it): its number is never given again,
    /// and the log says it did not happen, as the daemon's own change (see
    /// [`Self::commit_own`]). The caller puts back what it changed.
    fn not_made(&mut self, seq: u64) {
        self.next_seq = self.next_seq.max(seq + 1);
        let note = self.commit_own(LOADER_SEAT, What::Abandoned { change: seq });
        self.tell_paid(note.into_iter().collect());
    }

    /// Log `what` as the daemon's own change (as `seat`), synced, and make
    /// it: what it records (a recording stopped or carried on, a change
    /// that did not happen) has happened already. It is told at the next
    /// [`Self::tick`] and returned. If the log cannot take it, it is owed
    /// instead (`None`): it has no number until it is written, before any
    /// other line, and is told then; until then no change is made and
    /// `patch.json` is not written, so nothing the log lacks is ever
    /// numbered, told or saved.
    fn commit_own(&mut self, seat: &str, what: What) -> Option<(Change, Option<Event>)> {
        self.owed.push_back((seat.to_string(), what));
        let mut paid = self.pay_owed_all();
        // This one is the last owed: when all went, it is the last paid,
        // for the caller to tell; those paid before it were owed already.
        let this = if self.owed.is_empty() {
            paid.pop()
        } else {
            None
        };
        self.tell_paid(paid);
        this
    }

    /// Tell everyone, at the next [`Self::tick`], the daemon's own changes
    /// written to the log after being owed to it.
    fn tell_paid(&mut self, paid: Vec<(Change, Option<Event>)>) {
        for (change, fingerprints) in paid {
            self.told.push(Event::Change {
                change: Box::new(change),
            });
            // And the fingerprints it moved, if it moved any, after it.
            self.told.extend(fingerprints);
        }
    }

    /// Write the daemon's own changes owed to the log (see
    /// [`Self::commit_own`]), oldest first, each synced, numbered and made
    /// as it goes.
    ///
    /// # Errors
    ///
    /// `internal` when one cannot be written yet; it stays owed, with
    /// those after it.
    fn pay_owed(&mut self) -> Result<(), WallError> {
        let paid = self.pay_owed_all();
        self.tell_paid(paid);
        match (&self.log_error, self.owed.is_empty()) {
            (_, true) => Ok(()),
            (Some(err), false) => Err(error(
                ErrorCode::Internal,
                format!(
                    "the change log could not be written ({err}), so nothing was changed; the \
                     wall plays on as it was, and tries the log again with the next change"
                ),
            )),
            (None, false) => Err(error(
                ErrorCode::Internal,
                "the change log is behind, so nothing was changed; the wall tries it again \
                 with the next change",
            )),
        }
    }

    /// Pay what is owed to the log, as far as it will go (see
    /// [`Self::pay_owed`]); returns what was paid, each made, not yet told.
    fn pay_owed_all(&mut self) -> Vec<(Change, Option<Event>)> {
        let mut paid = Vec::new();
        while let Some((seat, what)) = self.owed.front() {
            let change = self.new_change(seat, what.clone(), None);
            if let Some(store) = self.store.as_mut() {
                if let Err(err) = store.append_synced(&change) {
                    eprintln!(
                        "kazoo-wall: the wall's own change ({}) could not be logged yet, and is \
                         written before any other: {err}",
                        change.summary
                    );
                    self.log_error = Some(err.to_string());
                    return paid;
                }
            }
            self.owed.pop_front();
            self.log_error = None;
            eprintln!("kazoo-wall: {}", change.summary);
            paid.push(self.apply(change));
        }
        paid
    }

    /// `what`, by `seat`, as the next change (not made yet: see
    /// [`Self::apply`]).
    fn new_change(&self, seat: &str, what: What, undoes: Option<u64>) -> Change {
        Change {
            seq: self.next_seq,
            at: utc_now(),
            seat: seat.to_string(),
            summary: summarise_with(seat, &what, undoes, |id| {
                self.patch.module(id).map(|m| m.kind)
            }),
            what,
            undoes,
        }
    }

    /// Make `change` (logged already, and made to the patch): mirror it
    /// into the engine, move the fingerprints, and remember it. Returns
    /// the change and, if the fingerprints moved, the event saying how.
    fn apply(&mut self, change: Change) -> (Change, Option<Event>) {
        self.mirror(&change.what);
        let mut dye = std::mem::take(&mut self.patch.dye);
        dye.touch(&change.seat, &change.what, &self.patch);
        self.patch.dye = dye;
        let prints = self.patch.dye.flow(&self.patch);
        let moved = prints.changed_since(&self.fingerprints);
        self.fingerprints = prints;
        let moved = (!moved.is_empty()).then_some(Event::Fingerprints {
            seq: change.seq,
            modules: moved.modules,
            cables: moved.cables,
        });
        self.next_seq = change.seq + 1;
        self.made = change.seq;
        self.patch.last_seq = change.seq;
        self.dirty = true;
        self.log.push_back(change.clone());
        while self.log.len() > MEMORY_CHANGES {
            let Some(gone) = self.log.pop_front() else {
                break;
            };
            // The wall up where memory starts moves on past what it let go.
            let abandoned = || {
                self.log.iter().any(
                    |note| matches!(note.what, What::Abandoned { change } if change == gone.seq),
                )
            };
            match gone.what {
                What::Resumed { .. } => self.memory_opens_on = gone.seq,
                What::Load { .. } if !abandoned() => self.memory_opens_on = gone.seq,
                _ => {}
            }
        }
        (change, moved)
    }

    // -----------------------------------------------------------------
    // Seats
    // -----------------------------------------------------------------

    /// A connection for `seat` said hello. Returns whether the seat just
    /// came online.
    pub fn seat_joined(&mut self, seat: &str) -> bool {
        let count = self.seats.entry(seat.to_string()).or_insert(0);
        *count += 1;
        *count == 1
    }

    /// A connection for `seat` closed. Returns whether the seat went away.
    pub fn seat_left(&mut self, seat: &str) -> bool {
        let Some(count) = self.seats.get_mut(seat) else {
            return false;
        };
        *count = count.saturating_sub(1);
        if *count == 0 {
            self.seats.remove(seat);
            true
        } else {
            false
        }
    }

    /// Seats online, sorted.
    #[must_use]
    pub fn seats(&self) -> Vec<String> {
        self.seats.keys().cloned().collect()
    }

    /// The latest change's sequence number: the latest made, never a
    /// number given to a change that was not (see [`Self::not_made`]).
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.made
    }

    // -----------------------------------------------------------------
    // Requests
    // -----------------------------------------------------------------

    /// Handle a request from `seat` as a console (see
    /// [`Self::request_as`]).
    ///
    /// # Errors
    ///
    /// The request's error, for the response.
    pub fn request(
        &mut self,
        seat: &str,
        request: &Request,
        now: Instant,
    ) -> Result<Outcome, WallError> {
        self.request_as(seat, true, request, now)
    }

    /// Handle a request from `seat`, a `console` or not (after hello;
    /// connection requests are the caller's). Change requests count
    /// against the seat's flood guard; a seat that is not a console is
    /// held to its allowance of saves too.
    ///
    /// # Errors
    ///
    /// The request's error, for the response.
    pub fn request_as(
        &mut self,
        seat: &str,
        console: bool,
        request: &Request,
        now: Instant,
    ) -> Result<Outcome, WallError> {
        if request.is_change() {
            let bucket = self
                .buckets
                .entry(seat.to_string())
                .or_insert_with(|| Bucket::new(now));
            if !bucket.take(now) {
                return Err(error(
                    ErrorCode::SlowDown,
                    format!(
                        "{seat} has made more than {FLOOD_BURST} changes in 10 seconds; wait a moment"
                    ),
                ));
            }
        }
        if matches!(request, Request::Saves) {
            let bucket = self
                .saves_buckets
                .entry(seat.to_string())
                .or_insert_with(|| Bucket::of(now, SAVES_BURST, SAVES_REFILL_PER_SECOND));
            if !bucket.take(now) {
                return Err(error(
                    ErrorCode::SlowDown,
                    format!(
                        "{seat} has asked for the saves more than {SAVES_BURST} times in a few \
                         seconds; wait a moment"
                    ),
                ));
            }
        }
        // A wall too large to keep takes nothing that makes it larger; what
        // takes something away goes through, and lets it be kept again.
        if self.too_large && self.grows(request) {
            return Err(error(
                ErrorCode::Full,
                "the wall is too large to keep, so changes are not being kept; this would make \
                 it larger, so it was refused: take some modules, cables or words away first",
            ));
        }
        let replaces = !console && self.puts_up_wall(request);
        if replaces {
            let bucket = self
                .load_buckets
                .entry(seat.to_string())
                .or_insert_with(|| Bucket::of(now, LOAD_BURST, LOAD_REFILL_PER_SECOND));
            if !bucket.take(now) {
                return Err(error(
                    ErrorCode::SlowDown,
                    format!(
                        "{seat} has replaced the wall more than {LOAD_BURST} times in a minute; \
                         wait a moment"
                    ),
                ));
            }
        }
        // A change that cannot be logged is not made: what of the patch it
        // touched goes back as it was.
        let before = self.before(request);
        let outcome = self.answer((seat, console), request, now);
        if std::mem::take(&mut self.unmade) {
            self.put_back(before);
        }
        // A refused load (no such save, a name that is not one, a save
        // changed since) replaced nothing, and costs none of the allowance;
        // one the disk refused did try.
        if replaces {
            if let Err(refused) = &outcome {
                if refused.code != ErrorCode::Internal {
                    if let Some(bucket) = self.load_buckets.get_mut(seat) {
                        bucket.refund();
                    }
                }
            }
        }
        for (module, knob, settles) in std::mem::take(&mut self.turned) {
            let due = now
                .checked_add(settles + RETIME_AFTER)
                .unwrap_or(now + RETIME_AFTER);
            // The latest turn of a knob is the one that counts.
            self.settling.insert((module, knob), due);
        }
        outcome
    }

    /// What of the patch `request` may touch before its change is logged,
    /// kept to put back if the log refuses it (see [`Before`]).
    fn before(&self, request: &Request) -> Before {
        let knobs = |module: &str| {
            self.patch.module(module).map_or(Before::Nothing, |found| {
                Before::Knobs(found.id.clone(), found.knobs.clone())
            })
        };
        match request {
            Request::Turn { module, .. } => knobs(module),
            Request::Patch { .. }
            | Request::Unpatch { .. }
            | Request::Add { .. }
            | Request::Remove { .. } => Before::Whole(Box::new(self.patch.clone())),
            Request::Undo { change } => {
                match self.log.iter().find(|logged| logged.seq == *change) {
                    Some(Change {
                        what: What::Turn { module, .. },
                        ..
                    }) => knobs(module),
                    Some(Change {
                        what:
                            What::Patch { .. }
                            | What::Unpatch { .. }
                            | What::Add { .. }
                            | What::Remove { .. }
                            | What::Restore { .. },
                        ..
                    }) => Before::Whole(Box::new(self.patch.clone())),
                    // A tempo, a load and words touch nothing before they
                    // are logged; the rest cannot be undone at all.
                    _ => Before::Nothing,
                }
            }
            // Nothing else touches the patch before its change is logged.
            _ => Before::Nothing,
        }
    }

    /// Put back what [`Self::before`] kept, after a change the log refused:
    /// the patch as it was but for the latest change's number (a note
    /// saying it did not happen may have been logged since).
    fn put_back(&mut self, before: Before) {
        match before {
            Before::Nothing => return,
            Before::Knobs(module, knobs) => self.patch.put_knobs_back(&module, &knobs),
            Before::Whole(mut patch) => {
                patch.last_seq = self.patch.last_seq;
                self.patch = *patch;
            }
        }
        self.fingerprints = self.patch.dye.flow(&self.patch);
    }

    /// Whether `request` would make the wall's file larger: a module, a
    /// cable or words added, or a removal or unplugging undone. (A turn or
    /// a tempo changes numbers, not size; a load is checked as it is
    /// written; the dye is held to its seats.)
    fn grows(&self, request: &Request) -> bool {
        match request {
            Request::Add { .. } | Request::Patch { .. } | Request::Speak { .. } => true,
            Request::Undo { change } => self.log.iter().any(|logged| {
                logged.seq == *change
                    && matches!(logged.what, What::Remove { .. } | What::Unpatch { .. })
            }),
            _ => false,
        }
    }

    /// Whether `request` would put up another wall: a load, a new wall, or
    /// undoing a load.
    fn puts_up_wall(&self, request: &Request) -> bool {
        match request {
            Request::Load { .. } | Request::New => true,
            Request::Undo { change } => self
                .log
                .iter()
                .any(|logged| logged.seq == *change && matches!(logged.what, What::Load { .. })),
            _ => false,
        }
    }

    /// Answer `request` from `seat`, once it has passed the flood check.
    fn answer(
        &mut self,
        (seat, console): (&str, bool),
        request: &Request,
        now: Instant,
    ) -> Result<Outcome, WallError> {
        let read = |result: serde_json::Value| Outcome {
            result,
            change: None,
            event: None,
            pending: None,
        };
        match request {
            Request::Look => encode(&self.snapshot()).map(read),
            Request::Catalogue => encode(&catalogue()).map(read),
            Request::Listen => encode(&ListenResult {
                listen: self.listen.clone(),
            })
            .map(read),
            Request::Probe { module, modules } => self
                .probe(module.as_deref(), modules, now)
                .and_then(|result| encode(&result))
                .map(read),
            Request::Log { before, limit } => encode(&self.log_page(*before, *limit)).map(read),
            Request::Turn {
                module,
                knob,
                value,
                glide_beats,
            } => {
                let what = self.patch.turn(module, knob, *value, *glide_beats)?;
                self.changed(seat, what, None, None, None)
            }
            Request::Patch { from, to, amount } => {
                let what = self.patch.plug(from, to, *amount, None)?;
                let cable = match &what {
                    What::Patch { cable, .. } => Some(cable.id),
                    _ => None,
                };
                self.changed(seat, what, None, None, cable)
            }
            Request::Unpatch { cable, to } => {
                let what = match (cable, to) {
                    (Some(cable), None) => self.patch.unplug(*cable, None)?,
                    (None, Some(to)) => self.patch.unplug_jack(to)?,
                    _ => {
                        return Err(error(
                            ErrorCode::BadRequest,
                            "unpatch takes a cable number or an input, not both",
                        ));
                    }
                };
                self.changed(seat, what, None, None, None)
            }
            Request::Add { kind, name, place } => {
                let what = self.patch.add(kind, name.as_deref(), place.as_ref())?;
                let module = match &what {
                    What::Add { module } => Some(module.id.clone()),
                    _ => None,
                };
                self.changed(seat, what, None, module, None)
            }
            Request::Remove { module } => {
                let what = self.patch.remove(module, &[])?;
                self.changed(seat, what, None, None, None)
            }
            Request::Arrange {
                module,
                row,
                before,
                own,
            } => self.arrange(
                module,
                &Place {
                    row: *row,
                    before: before.clone(),
                    own: *own,
                },
            ),
            Request::Undo { change } => self.undo((seat, console), *change),
            Request::Tempo { bpm } => self.tempo(seat, *bpm, None),
            Request::Speak {
                module,
                text,
                voice,
            } => self.speak(seat, module, text, voice.as_deref()),
            Request::Record { on: true } => self.start_recording(seat),
            Request::Record { on: false } => self.stop_recording(seat),
            Request::Save { name, overwrite } => {
                self.save_as((seat, console), name, *overwrite, now)
            }
            Request::Saves => self.saves(),
            Request::Load { name } => self.load((seat, console), name),
            Request::New => self.blank((seat, console)),
            Request::DeleteSave { name } => self.delete_save(seat, name),
            Request::Hello { .. }
            | Request::Subscribe
            | Request::Monitor { .. }
            | Request::Shutdown => Err(error(
                ErrorCode::Internal,
                "connection requests are handled by the connection",
            )),
        }
    }

    fn changed(
        &mut self,
        seat: &str,
        what: What,
        undoes: Option<u64>,
        module: Option<String>,
        cable: Option<u32>,
    ) -> Result<Outcome, WallError> {
        let module = module.or_else(|| match &what {
            What::Restore { module, .. } => Some(module.id.clone()),
            _ => None,
        });
        let cable = cable.or(match &what {
            What::Patch { cable, .. } => Some(cable.id),
            _ => None,
        });
        let (change, fingerprints) = self.commit(seat, what, undoes)?;
        let result = encode(&ChangeResult {
            change: change.clone(),
            module,
            cable,
        })?;
        Ok(Outcome {
            result,
            change: Some(change),
            event: fingerprints,
            pending: None,
        })
    }

    /// Move a module on the rack: not a change (nothing is logged, the
    /// revision stays), but saved with the patch and told to every
    /// subscriber as the rows now stand.
    fn arrange(&mut self, module: &str, place: &Place) -> Result<Outcome, WallError> {
        if self.patch.arrange(module, place)? {
            self.dirty = true;
        }
        let rows = self.patch.rows().to_vec();
        let result = encode(&ArrangeResult { rows: rows.clone() })?;
        Ok(Outcome {
            result,
            change: None,
            event: Some(Event::Rack { rows }),
            pending: None,
        })
    }

    fn undo(&mut self, (seat, console): (&str, bool), seq: u64) -> Result<Outcome, WallError> {
        // A change that did not happen (whether or not its line is in the
        // log), or a load whose wall did not come back up when the wall
        // started again, left nothing to undo.
        if let Some(note) = self.passed_over(seq) {
            let why = if matches!(note.what, What::Abandoned { .. }) {
                "could not be written to the log for certain, so it did not happen"
            } else {
                "put up a wall that did not come back up when the wall started again"
            };
            return Err(error(
                ErrorCode::NotAllowed,
                format!(
                    "change {seq} {why} (change {} says so): there is nothing of it to undo",
                    note.seq
                ),
            ));
        }
        let what = self
            .log
            .iter()
            .find(|change| change.seq == seq)
            .map(|change| change.what.clone())
            .ok_or_else(|| {
                let oldest = self.log.front().map_or(0, |change| change.seq);
                error(
                    ErrorCode::UnknownChange,
                    format!(
                        "no change {seq} in the log (it holds changes {oldest} to {})",
                        self.revision()
                    ),
                )
            })?;
        // A note, a recording or words can never be undone: said as that,
        // before anything about which wall is up.
        if let Some(refused) = never_undone(&what) {
            return Err(refused);
        }
        // A load keeps the wall it replaced in a save: it can be brought
        // back whatever is up now, as long as that save still holds it.
        if let What::Load {
            backup,
            fingerprint,
            ..
        } = what
        {
            return self.undo_load((seat, console), seq, &backup, &fingerprint);
        }
        if !self.walls().holds(seq) {
            return Err(self.replaced_since(seq));
        }
        if let What::Tempo { from, .. } = what {
            return self.tempo(seat, from, Some(seq));
        }
        let inverse = self.patch.undo(&what)?;
        self.changed(seat, inverse, Some(seq), None, None)
    }

    fn tempo(&mut self, seat: &str, bpm: f64, undoes: Option<u64>) -> Result<Outcome, WallError> {
        if !bpm.is_finite() {
            return Err(error(ErrorCode::BadRequest, "the tempo must be a number"));
        }
        let bpm = clamp_bpm(bpm);
        let desk = self.desk.as_ref().is_some_and(HubLink::is_connected);
        // What plays now: on the desk, the desk's; off it, the wall's own
        // as last changed (the engine hears a change a block later, and
        // through a load's fade later still).
        let from = if desk {
            clamp_bpm(self.engine.shared().bpm())
        } else {
            self.patch.tempo
        };
        // The log first: the desk is asked only once the change is in it,
        // so a tempo the log cannot take never moves the desk (whose tempo
        // the wall then follows and saves).
        let change = self.next_change(
            seat,
            What::Tempo {
                from,
                to: bpm,
                desk,
            },
            undoes,
        )?;
        self.record(&change)?;
        if desk {
            // Tempos are 20-300 BPM: exact enough in f32.
            let asked = self
                .desk
                .as_ref()
                .map_or(Err(RequestError::NotConnected), |link| {
                    link.request_tempo(bpm as f32)
                });
            if let Err(refused) = asked {
                // In the log, but the desk never heard it: it did not
                // happen, and the log says so.
                self.not_made(change.seq);
                return Err(match refused {
                    RequestError::Full => error(
                        ErrorCode::SlowDown,
                        "the desk has too many requests waiting; try again",
                    ),
                    RequestError::NotConnected => error(
                        ErrorCode::SlowDown,
                        "the desk went away as the tempo was asked; try again (the wall keeps \
                         its own tempo while it is off the desk)",
                    ),
                    RequestError::Invalid => {
                        error(ErrorCode::BadRequest, "the desk refused that tempo")
                    }
                });
            }
        } else {
            self.patch.tempo = bpm;
        }
        let (change, fingerprints) = self.apply(change);
        let result = encode(&TempoResult {
            bpm,
            desk,
            change: change.clone(),
        })?;
        Ok(Outcome {
            result,
            change: Some(change),
            event: fingerprints,
            pending: None,
        })
    }

    /// Which wall each change in memory was made to, and how the walls
    /// came from each other (see [`Walls`]).
    fn walls(&self) -> Walls {
        walls_of(self.memory_opens_on, self.log.iter())
    }

    /// The note saying change `seq` did not happen ([`What::Abandoned`]),
    /// or, for a load, that the wall it put up did not come back up when
    /// the wall started again ([`What::Resumed`]), if one does.
    fn passed_over(&self, seq: u64) -> Option<&Change> {
        self.log.iter().find(|note| match &note.what {
            What::Abandoned { change } => *change == seq,
            What::Resumed { left_out, .. } => left_out.contains(&seq),
            _ => false,
        })
    }

    /// The refusal to undo change `seq`, made to a wall that is not up:
    /// saying what replaced it last (a load that happened, or the wall
    /// starting again on another).
    fn replaced_since(&self, seq: u64) -> WallError {
        let abandoned = abandoned_changes(self.log.iter());
        let since = self
            .log
            .iter()
            .rev()
            .take_while(|change| change.seq > seq)
            .find(|change| match change.what {
                What::Load { .. } => !abandoned.contains(&change.seq),
                What::Resumed { .. } => true,
                _ => false,
            })
            .map_or_else(String::new, |up| {
                if matches!(up.what, What::Load { .. }) {
                    format!(
                        " (change {}: {}); undo change {} to bring back the wall before it",
                        up.seq, up.summary, up.seq
                    )
                } else {
                    format!(" (change {}: {})", up.seq, up.summary)
                }
            });
        error(
            ErrorCode::NotAllowed,
            format!("change {seq} was made to a wall that has been replaced since{since}"),
        )
    }

    /// Refuse `request` from a client that last looked at wall `seen`
    /// (see [`crate::protocol::RequestLine::wall`]) when it names modules
    /// and another wall is up now.
    ///
    /// # Errors
    ///
    /// `wall_replaced`.
    pub fn check_wall(&self, seen: Option<u64>, request: &Request) -> Result<(), WallError> {
        match seen {
            Some(seen) if request.acts_on_modules() && seen != self.patch.wall => Err(error(
                ErrorCode::WallReplaced,
                format!(
                    "the wall was replaced since this was asked (it was asked of wall {seen}; \
                     change {} put up the wall playing now), so it was not done: look again \
                     before changing it",
                    self.patch.wall
                ),
            )),
            Some(_) | None => Ok(()),
        }
    }

    // -----------------------------------------------------------------
    // Saves
    // -----------------------------------------------------------------

    /// The store the saves are in.
    fn saves_store(&mut self) -> Result<&mut Store, WallError> {
        self.store.as_mut().ok_or_else(no_saves)
    }

    /// Hold a seat that is not a console to its allowance of saves, and the
    /// saves made by hand to [`MAX_SAVES`], so a seat saving in a loop
    /// cannot fill the disk. Consoles (Tom) are held to neither.
    ///
    /// The saves are counted first: a save refused for that (or for its
    /// name being taken, which a seat cannot overwrite) costs no allowance.
    fn allow_save(&mut self, seat: &str, name: &str, now: Instant) -> Result<(), WallError> {
        let store = self.saves_store()?;
        if let Some((stored, _)) = store
            .find_save(name)
            .map_err(|err| save_error(&SaveError::Io(err)))?
        {
            return Err(save_error(&SaveError::Exists(stored)));
        }
        let count = store
            .count_saves()
            .map_err(|err| save_error(&SaveError::Io(err)))?;
        if count >= MAX_SAVES {
            return Err(error(
                ErrorCode::Full,
                format!(
                    "there are {count} saves, as many as a seat may make ({MAX_SAVES}); Tom's \
                     console can delete some"
                ),
            ));
        }
        let bucket = self
            .save_buckets
            .entry(seat.to_string())
            .or_insert_with(|| Bucket::of(now, SAVE_BURST, SAVE_REFILL_PER_SECOND));
        if !bucket.take(now) {
            return Err(error(
                ErrorCode::SlowDown,
                format!(
                    "{seat} has saved the wall more than {SAVE_BURST} times in a minute; wait a \
                     moment"
                ),
            ));
        }
        Ok(())
    }

    /// Save the wall as it is now as `name`, for `seat` (only a console
    /// gets as far as `overwrite`), held to its allowance unless it is a
    /// `console`. Not a change: nothing is logged and the sound is
    /// untouched; every subscriber is told.
    fn save_as(
        &mut self,
        (seat, console): (&str, bool),
        name: &str,
        overwrite: bool,
        now: Instant,
    ) -> Result<Outcome, WallError> {
        check_save_name(name)?;
        if is_auto_save(name) {
            return Err(error(
                ErrorCode::BadName,
                format!(
                    "names starting with \"before-\" are the wall's own saves (made before it \
                     replaces the wall); choose another name than \"{name}\""
                ),
            ));
        }
        if !console {
            self.allow_save(seat, name, now)?;
        }
        let Some(store) = self.store.as_mut() else {
            return Err(no_saves());
        };
        let written = store
            .write_save(name, &self.patch, overwrite)
            .map_err(|err| save_error(&err))?;
        let (stored, replaced) = (written.name.clone(), written.replaced);
        let saved_at = store.saved_at(&stored).unwrap_or_else(|err| {
            eprintln!("kazoo-wall: when {stored} was saved cannot be read ({err}); saying now");
            SystemTime::now()
        });
        let save = save_info(&stored, &self.patch, saved_at, FILE_VERSION);
        // In place, whether or not its directory could be flushed: the
        // wall is that save now, and everyone is told.
        let notes = unsynced_note(written.unsynced.as_ref(), "in place", "take it back");
        let print = Print::of(&self.patch);
        self.save_mark = Some((
            SaveMark {
                name: stored.clone(),
                revision: self.revision(),
                changed: false,
            },
            print.clone(),
        ));
        // Trusted to keep the wall for a load only once durable: a save a
        // crash could take back is no backup.
        self.kept_in = written.unsynced.is_none().then_some((
            Kept {
                name: written.name,
                hash: written.hash,
            },
            print,
        ));
        eprintln!(
            "kazoo-wall: {seat} saved the wall as {stored}{}",
            if replaced { ", replacing it" } else { "" }
        );
        let result = encode(&SaveResult {
            save,
            replaced,
            notes,
        })?;
        Ok(Outcome {
            result,
            change: None,
            event: Some(Event::Saved {
                seat: seat.to_string(),
                name: stored,
                replaced,
            }),
            pending: None,
        })
    }

    /// Every save, newest first, with the files that cannot be loaded.
    fn saves(&mut self) -> Result<Outcome, WallError> {
        let listed = self.saves_store()?.list_saves().map_err(|err| {
            error(
                ErrorCode::Internal,
                format!("the saves could not be read: {err}"),
            )
        })?;
        let mut saves = Vec::new();
        let mut broken = Vec::new();
        let mut pending = Vec::new();
        for entry in listed {
            match entry.read {
                Some(Ok(header)) => saves.push(SaveInfo {
                    auto: is_auto_save(&entry.name),
                    name: entry.name,
                    saved_at: utc_of(entry.saved_at),
                    tempo: header.tempo,
                    modules: header.modules,
                    cables: header.cables,
                    version: header.version,
                }),
                Some(Err(problem)) => broken.push(BrokenSave {
                    name: printable(&entry.name),
                    saved_at: utc_of(entry.saved_at),
                    problem,
                }),
                None => pending.push(printable(&entry.name)),
            }
        }
        // patch.json as it was when the wall could not use it at a start:
        // never loaded, but never out of sight.
        let set_aside = self.saves_store()?.set_aside_files().map_err(|err| {
            error(
                ErrorCode::Internal,
                format!("the wall's state could not be read: {err}"),
            )
        })?;
        for (file, at) in set_aside {
            broken.push(BrokenSave {
                name: printable(&file),
                saved_at: utc_of(at),
                problem: "patch.json as it was when the wall could not use it at a start, set \
                          aside there (the log says why); it is not a save, and is left as it is"
                    .to_string(),
            });
        }
        encode(&SavesResult {
            saves,
            broken,
            pending,
        })
        .map(unchanged)
    }

    /// Replace the wall with the save `name`, for `seat`.
    fn load(&mut self, (seat, console): (&str, bool), name: &str) -> Result<Outcome, WallError> {
        check_save_name(name)?;
        let read = self
            .saves_store()?
            .read_save(name)
            .map_err(|err| save_error(&err))?;
        let mut notes = Vec::with_capacity(read.notes.len() + 1);
        if read.from_version != FILE_VERSION {
            notes.push(format!(
                "the save was made by an older wall (version {}): it was brought up to version \
                 {FILE_VERSION} as it loaded, and its file is left as it was",
                read.from_version
            ));
        }
        notes.extend(read.notes);
        for note in &notes {
            eprintln!("kazoo-wall: loading {}: {note}", read.name);
        }
        let kept = Kept {
            name: read.name,
            hash: read.hash,
        };
        self.replace((seat, console), *read.patch, &Source::Save(kept), notes)
    }

    /// Replace the wall with an empty one at the same tempo, for `seat`.
    fn blank(&mut self, (seat, console): (&str, bool)) -> Result<Outcome, WallError> {
        let empty = Patch::empty(self.patch.tempo);
        self.replace((seat, console), empty, &Source::Empty, Vec::new())
    }

    /// Undo load `seq`, for `seat`: bring back the wall it replaced, from
    /// the save `backup` it was kept in, as long as that save's file still
    /// has the very bytes it had then (`fingerprint`, a hash of them as
    /// written, compared before any migration: a later build that reads
    /// the file differently still recognises it).
    fn undo_load(
        &mut self,
        (seat, console): (&str, bool),
        seq: u64,
        backup: &str,
        fingerprint: &str,
    ) -> Result<Outcome, WallError> {
        let gone = |why: &str| {
            error(
                ErrorCode::NotAllowed,
                format!(
                    "the wall change {seq} replaced was kept as \"{backup}\", {why}, so it cannot \
                     come back exactly"
                ),
            )
        };
        let read = match self.saves_store()?.read_save(backup) {
            Ok(read) => read,
            Err(SaveError::Unknown(_)) => {
                return Err(gone(&format!(
                    "which is no longer among the saves (the wall's own are let go beyond the \
                     newest {AUTO_SAVES_KEPT}, and a console can delete any)"
                )));
            }
            Err(SaveError::Broken { reason, .. }) => {
                return Err(gone(&format!("which cannot be read ({reason})")));
            }
            Err(err) => return Err(save_error(&err)),
        };
        if fingerprint.is_empty() || read.hash != fingerprint {
            return Err(gone("and that save has changed since"));
        }
        self.replace(
            (seat, console),
            *read.patch,
            &Source::Undo {
                load: seq,
                kept: Kept {
                    name: read.name,
                    hash: read.hash,
                },
            },
            read.notes,
        )
    }

    /// Where the wall as it is will be kept before it is replaced: the
    /// save that already holds exactly it, when the wall has not changed
    /// since (see [`Self::still`]) and the save's file still has the
    /// bytes it had (so a file replaced from outside is never trusted);
    /// else a new automatic save (`before-<how>-<time>`), with the bytes to
    /// write.
    fn keeping(&self, how: &str) -> Result<(Kept, Option<Vec<u8>>), SaveError> {
        let Some(store) = self.store.as_ref() else {
            return Err(SaveError::Io(std::io::Error::other(
                "this wall keeps no state on disk",
            )));
        };
        let held = self.kept_in.as_ref().filter(|(_, print)| self.still(print));
        if let Some((kept, _)) = held {
            if store.hash_save(&kept.name)?.as_deref() == Some(kept.hash.as_str()) {
                return Ok((kept.clone(), None));
            }
            eprintln!(
                "kazoo-wall: the save {} no longer holds the wall as it is (changed from \
                 outside?); saving the wall afresh",
                kept.name
            );
        }
        let name = store.auto_name(how)?;
        let bytes = crate::store::encode(&self.patch)?;
        // A backup that could not be read back keeps nothing.
        if bytes.len() as u64 > crate::store::MAX_WALL_FILE {
            return Err(SaveError::TooLarge(bytes.len()));
        }
        let hash = hash_bytes(&bytes);
        Ok((Kept { name, hash }, Some(bytes)))
    }

    /// Let go of the wall's own saves beyond those kept (see
    /// [`Store::prune_auto_saves`]): the ones a load in the log still
    /// needs to be undone (a console's before a seat's), and the one
    /// holding the wall as it is, stay.
    fn prune(&mut self) {
        let mut needed: HashMap<String, Need> = HashMap::new();
        for change in &self.log {
            if let What::Load {
                backup, by_console, ..
            } = &change.what
            {
                let need = if *by_console {
                    Need::Console
                } else {
                    Need::Seat
                };
                let was = needed.entry(backup.clone()).or_insert(need);
                if need == Need::Console {
                    *was = Need::Console;
                }
            }
        }
        // The wall as it is: as dear as anything the console did.
        if let Some((kept, _)) = &self.kept_in {
            needed.insert(kept.name.clone(), Need::Console);
        }
        let Some(store) = self.store.as_mut() else {
            return;
        };
        match store.prune_auto_saves(&|name| needed.get(name).copied().unwrap_or(Need::Not)) {
            Ok(deleted) => {
                for name in deleted {
                    eprintln!("kazoo-wall: let go of the automatic save {name}");
                }
            }
            Err(err) => eprintln!("kazoo-wall: the automatic saves could not be pruned: {err}"),
        }
    }

    /// The tempo that will play once `incoming` is up: its own, or the
    /// desk's while the wall follows it. What else a load should say about
    /// what goes on through it goes in `notes`.
    fn playing_after(&self, incoming: &Patch, notes: &mut Vec<String>) -> f64 {
        let playing = if self.on_desk() {
            let desk = clamp_bpm(self.engine.shared().bpm());
            if (incoming.tempo - desk).abs() > 1e-9 {
                notes.push(format!(
                    "the desk keeps the tempo at {}; this wall's own {} is not used while it \
                     follows the desk",
                    format::bpm(desk),
                    format::bpm(incoming.tempo)
                ));
            }
            desk
        } else {
            incoming.tempo
        };
        if self.recorder.status().is_some() {
            notes.push(
                "the recording under way goes on through the change (with its short fade)"
                    .to_string(),
            );
        }
        playing
    }

    /// Replace the wall with `incoming` (from `source`) as `seat`'s change,
    /// while it plays, as one batch on disk (see [`Store::commit_load`]):
    /// the wall as it is kept in a save, the change in the log, the new
    /// wall in `patch.json`. Only once the change is durably in the log is
    /// anything changed here: the patch and the engine swapped, the change
    /// remembered. If the batch fails before that, nothing is replaced and
    /// the answer says why. `notes` go into the answer.
    fn replace(
        &mut self,
        (seat, console): (&str, bool),
        mut incoming: Patch,
        source: &Source,
        mut notes: Vec<String>,
    ) -> Result<Outcome, WallError> {
        if self.store.is_none() {
            return Err(no_saves());
        }
        let how = source.how();
        // The daemon's own changes owed to the log go first (they take
        // their numbers before this one), or nothing is replaced.
        self.pay_owed()?;
        let (backup, backup_bytes) = self.keeping(how).map_err(|err| not_kept(seat, how, &err))?;
        let playing = self.playing_after(&incoming, &mut notes);
        let incoming_print = Print::of(&incoming);
        let what = What::Load {
            save: source.save().map(str::to_string),
            backup: backup.name.clone(),
            fingerprint: backup.hash.clone(),
            modules: u32::try_from(incoming.modules().len()).unwrap_or(u32::MAX),
            cables: u32::try_from(incoming.cables().len()).unwrap_or(u32::MAX),
            tempo: playing,
            by_console: console,
        };
        let change = self.new_change(seat, what, source.undoes());
        // The wall put up is named by the change that puts it up, and goes
        // to patch.json as of that change.
        incoming.last_seq = change.seq;
        incoming.wall = change.seq;
        let Some(store) = self.store.as_mut() else {
            return Err(no_saves());
        };
        let batch = backup_bytes
            .as_deref()
            .map(|bytes| (backup.name.as_str(), bytes));
        let in_patch = match store.commit_load(batch, &incoming, &change) {
            Ok(written) => written.unsaved.is_none_or(|err| {
                eprintln!("kazoo-wall: the loaded wall is not in patch.json yet: {err}");
                notes.push(format!(
                    "the load is in the log, but the wall could not be written to patch.json \
                     yet ({err}); it is written again within a second"
                ));
                false
            }),
            Err(LoadWriteError::MaybeLogged(err)) => {
                // The line may be on disk: its number is never given again,
                // and the log says at once that it did not happen (or owes
                // it that, when it cannot be written yet either).
                eprintln!(
                    "kazoo-wall: {seat} asked to {how}, and the change could not be logged: {err}"
                );
                self.not_made(change.seq);
                return Err(error(
                    ErrorCode::Internal,
                    format!(
                        "the change could not be written to the log for certain ({err}), so \
                         nothing was replaced"
                    ),
                ));
            }
            Err(other) => return Err(not_written(seat, how, &other)),
        };
        // In the log for good: the load happens.
        self.patch = incoming;
        let (change, _) = self.apply(change);
        eprintln!("kazoo-wall: {}", change.summary);
        // Saved with the batch, or again at the next save.
        self.dirty = !in_patch;
        self.last_save = Some(Instant::now());
        let save = source.save().map(str::to_string);
        self.save_mark = save.as_ref().map(|name| {
            (
                SaveMark {
                    name: name.clone(),
                    revision: change.seq,
                    changed: false,
                },
                incoming_print.clone(),
            )
        });
        self.kept_in = source.kept().map(|kept| (kept.clone(), incoming_print));
        self.prune();
        let result = encode(&LoadResult {
            change: change.clone(),
            backup: backup.name.clone(),
            notes,
        })?;
        Ok(Outcome {
            result,
            event: Some(Event::Loaded {
                seq: change.seq,
                seat: seat.to_string(),
                save,
                backup: backup.name,
            }),
            change: Some(change),
            pending: None,
        })
    }

    /// Delete the save `name` for `seat` (a console). Not a change; every
    /// subscriber is told.
    fn delete_save(&mut self, seat: &str, name: &str) -> Result<Outcome, WallError> {
        check_save_name(name)?;
        let (stored, unsynced) = self
            .saves_store()?
            .delete_save(name)
            .map_err(|err| save_error(&err))?;
        // Gone, whether or not its directory could be flushed: the wall
        // forgets it, and everyone is told.
        let notes = unsynced_note(unsynced.as_ref(), "gone", "bring it back");
        if self
            .save_mark
            .as_ref()
            .is_some_and(|(mark, _)| mark.name == stored)
        {
            self.save_mark = None;
        }
        if self
            .kept_in
            .as_ref()
            .is_some_and(|(kept, _)| kept.name == stored)
        {
            self.kept_in = None;
        }
        eprintln!("kazoo-wall: {seat} deleted the save {stored}");
        let result = encode(&DeleteSaveResult {
            name: stored.clone(),
            notes,
        })?;
        Ok(Outcome {
            result,
            change: None,
            event: Some(Event::SaveDeleted {
                seat: seat.to_string(),
                name: stored,
            }),
            pending: None,
        })
    }

    fn speak(
        &mut self,
        seat: &str,
        module: &str,
        text: &str,
        voice: Option<&str>,
    ) -> Result<Outcome, WallError> {
        let found = self.patch.find(module)?;
        if !matches!(found.kind.spec().build, Builder::Adapted(Adapter::Speak)) {
            return Err(error(
                ErrorCode::BadRequest,
                format!(
                    "{module} is a {} and cannot speak; add a speak module",
                    found.kind
                ),
            ));
        }
        let words = Words {
            text: text.to_string(),
            voice: voice.map(str::to_string),
        };
        let rate = self.engine.sample_rate();
        let ticket = self
            .speech
            .submit(module, words, rate, Some(seat.to_string()))?;
        Ok(Outcome {
            result: serde_json::Value::Null,
            change: None,
            event: None,
            pending: Some(ticket),
        })
    }

    /// Words that finished rendering since the last call, each answered
    /// (for a `speak` request) with its outcome under the ticket
    /// [`Outcome::pending`] gave. A seat's words are logged as its change
    /// and only then given to their module's player, so a player never
    /// says words the wall does not have; saved words put back are given
    /// at once, and not logged.
    pub fn spoken(&mut self) -> Vec<(u64, Result<Outcome, WallError>)> {
        let mut answers = Vec::new();
        for spoken in self.speech.finished() {
            let Some(seat) = spoken.seat else {
                if let Err(err) = spoken.result {
                    eprintln!(
                        "kazoo-wall: {}'s saved words could not be put back: {}",
                        spoken.module, err.message
                    );
                }
                continue;
            };
            let answer = match (spoken.result, spoken.phrase) {
                (Ok(seconds), Some(phrase)) => {
                    self.speak_logged(&seat, spoken.module, spoken.words, (phrase, seconds))
                }
                (Ok(_), None) => Err(error(
                    ErrorCode::Internal,
                    format!("{}'s words came back without a phrase", spoken.module),
                )),
                (Err(err), _) => Err(err),
            };
            answers.push((spoken.ticket, answer));
        }
        answers
    }

    /// Log `seat`'s `words` for `module` as its change, then give the
    /// rendered phrase (lasting `seconds`) to the module's player, and
    /// only then make the change. If the log cannot take it, or the player
    /// cannot take the phrase, nothing changes: the player keeps what it
    /// was saying, and the patch the words it had.
    fn speak_logged(
        &mut self,
        seat: &str,
        module: String,
        words: Words,
        (phrase, seconds): (kazoo_speech::Phrase, f64),
    ) -> Result<Outcome, WallError> {
        let what = What::Speak {
            module: module.clone(),
            words: u32::try_from(words.text.split_whitespace().count()).unwrap_or(u32::MAX),
            seconds,
        };
        let recorded = self
            .next_change(seat, what, None)
            .and_then(|change| self.record(&change).map(|()| change));
        // Nothing of the patch was touched to put back.
        self.unmade = false;
        let change = recorded?;
        if let Err(err) = self.speech.hand_over(&module, phrase) {
            // In the log, but no player says it: it did not happen.
            self.not_made(change.seq);
            return Err(err);
        }
        self.patch.speech.insert(module.clone(), words);
        let (change, fingerprints) = self.apply(change);
        let result = encode(&ChangeResult {
            change: change.clone(),
            module: Some(module),
            cable: None,
        })?;
        Ok(Outcome {
            result,
            change: Some(change),
            event: fingerprints,
            pending: None,
        })
    }

    /// Start recording for `seat`; a recording already under way is
    /// answered as it is, with no change.
    fn start_recording(&mut self, seat: &str) -> Result<Outcome, WallError> {
        if let Some(status) = self.recorder.status() {
            return encode(&RecordResult {
                on: true,
                path: Some(status.path.to_string_lossy().into_owned()),
                seconds: status.seconds,
                dropped: status.dropped,
                change: None,
            })
            .map(unchanged);
        }
        // A recording starts only when the log can take it.
        self.pay_owed()?;
        let path = self.recorder.start(seat, true).map_err(|why| {
            eprintln!("kazoo-wall: {seat} asked to record, and it could not start: {why}");
            error(
                ErrorCode::Internal,
                format!("the recording could not start: {why}"),
            )
        })?;
        let path = path.to_string_lossy().into_owned();
        eprintln!("kazoo-wall: {seat} started recording {path}");
        let what = What::Record {
            on: true,
            path: path.clone(),
            seconds: None,
            dropped: 0,
            continues: None,
            reason: None,
        };
        let (change, fingerprints) = match self.commit(seat, what, None) {
            Ok(committed) => committed,
            Err(err) => {
                // Not logged, so not started: the file (a moment long) goes.
                if let Some(finished) = self.recorder.stop() {
                    if let Err(gone) = std::fs::remove_file(&finished.path) {
                        eprintln!(
                            "kazoo-wall: the recording {} that could not be logged could not be \
                             removed: {gone}",
                            finished.path.display()
                        );
                    }
                }
                return Err(err);
            }
        };
        let result = encode(&RecordResult {
            on: true,
            path: Some(path),
            seconds: 0.0,
            dropped: 0,
            change: Some(change.clone()),
        })?;
        Ok(Outcome {
            result,
            change: Some(change),
            event: fingerprints,
            pending: None,
        })
    }

    /// Stop the recording under way for `seat`; with none, say so, with no
    /// change.
    fn stop_recording(&mut self, seat: &str) -> Result<Outcome, WallError> {
        let Some(finished) = self.recorder.stop() else {
            return encode(&RecordResult {
                on: false,
                path: None,
                seconds: 0.0,
                dropped: 0,
                change: None,
            })
            .map(unchanged);
        };
        let reason = match &finished.ending {
            Ending::Failed(why) => Some(why.clone()),
            Ending::Asked | Ending::Full => None,
        };
        // Stopped whatever the log says; while it owes, the change has no
        // number yet and is told once written.
        let (change, fingerprints) = self.log_stop(seat, &finished, reason).unzip();
        let result = encode(&RecordResult {
            on: false,
            path: Some(finished.path.to_string_lossy().into_owned()),
            seconds: finished.seconds,
            dropped: finished.dropped,
            change: change.clone(),
        })?;
        Ok(Outcome {
            result,
            change,
            event: fingerprints.flatten(),
            pending: None,
        })
    }

    /// Log `finished` as stopped by `seat`, for `reason` when the wall
    /// stopped it on its own (see [`Self::commit_own`]: `None` while the
    /// log owes it).
    fn log_stop(
        &mut self,
        seat: &str,
        finished: &Finished,
        reason: Option<String>,
    ) -> Option<(Change, Option<Event>)> {
        let what = What::Record {
            on: false,
            path: finished.path.to_string_lossy().into_owned(),
            seconds: Some(finished.seconds),
            dropped: finished.dropped,
            continues: None,
            reason,
        };
        self.commit_own(seat, what)
    }

    /// A recording ended that nobody asked to stop: log that as the
    /// wall's own change, then (when `carry` is true, or its file filled)
    /// carry it on into a new file, logging that too. `why` says what
    /// ended it when writing did not fail. The changes are told at the
    /// next [`Self::tick`].
    fn carry_on(&mut self, finished: &Finished, why: &str, carry: bool) {
        let (reason, carry, fresh) = match &finished.ending {
            Ending::Failed(failed) => (failed.clone(), carry, true),
            Ending::Full => (
                format!(
                    "the file reached the WAV format's limit of {} frames",
                    super::record::MAX_FILE_FRAMES
                ),
                true,
                false,
            ),
            Ending::Asked => (why.to_string(), carry, true),
        };
        let stopped = self.log_stop(LOADER_SEAT, finished, Some(reason.clone()));
        self.tell_paid(stopped.into_iter().collect());
        if !carry {
            return;
        }
        match self.recorder.start(&finished.seat, fresh) {
            Ok(path) => {
                let what = What::Record {
                    on: true,
                    path: path.to_string_lossy().into_owned(),
                    seconds: None,
                    dropped: 0,
                    continues: Some(finished.path.to_string_lossy().into_owned()),
                    reason: Some(reason),
                };
                let started = self.commit_own(LOADER_SEAT, what);
                self.tell_paid(started.into_iter().collect());
            }
            Err(failed) => {
                let summary = format!(
                    "the recording {} could not carry on into a new file: {failed}",
                    finished.path.display()
                );
                eprintln!("kazoo-wall: {summary}");
                self.told.push(Event::Fault {
                    summary,
                    seq: Some(self.revision()),
                });
            }
        }
    }

    /// Stop the recording under way as the daemon stops, finishing its
    /// file, and log it. Returns the change to tell everyone, if there was
    /// a recording and the log took it (see [`Self::close`] for what
    /// becomes of it if not).
    pub fn end_recording(&mut self) -> Option<Event> {
        let finished = self.recorder.stop()?;
        let reason = match &finished.ending {
            Ending::Failed(why) => why.clone(),
            Ending::Asked | Ending::Full => "the wall stopped".to_string(),
        };
        let (change, _) = self.log_stop(LOADER_SEAT, &finished, Some(reason))?;
        Some(Event::Change {
            change: Box::new(change),
        })
    }

    /// The daemon is stopping: write what the log is owed (trying once
    /// more), and save the patch. Returns the events to tell everyone
    /// before the connections close. What the log still cannot take is
    /// lost, and said in the daemon's log: it was never numbered or told,
    /// and `patch.json` stays where the log is, so the next start numbers
    /// on from what was written and nothing told is ever numbered again.
    pub fn close(&mut self, now: Instant) -> Vec<Event> {
        self.save(now);
        if !self.owed.is_empty() {
            eprintln!(
                "kazoo-wall: stopping with {} of the wall's own changes the change log could not \
                 take ({}); they are lost:",
                self.owed.len(),
                self.log_error.as_deref().unwrap_or("no reason given")
            );
            for (seat, what) in &self.owed {
                let summary =
                    summarise_with(seat, what, None, |id| self.patch.module(id).map(|m| m.kind));
                eprintln!("kazoo-wall:   {summary}");
            }
        }
        let mut events = std::mem::take(&mut self.told);
        let health = self.log_health();
        if health != self.log_told {
            self.log_told = health.clone();
            events.push(Event::Log { log: health });
        }
        events
    }

    fn log_page(&self, before: Option<u64>, limit: Option<u32>) -> LogPage {
        let limit = limit.unwrap_or(DEFAULT_LOG_PAGE).clamp(1, MAX_LOG_PAGE) as usize;
        let older: Vec<&Change> = self
            .log
            .iter()
            .filter(|change| before.is_none_or(|before| change.seq < before))
            .collect();
        let start = older.len().saturating_sub(limit);
        LogPage {
            changes: older[start..]
                .iter()
                .map(|change| (*change).clone())
                .collect(),
            more: start > 0,
        }
    }

    // -----------------------------------------------------------------
    // Looking
    // -----------------------------------------------------------------

    /// The whole wall.
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        let shared = self.engine.shared();
        let modules = self
            .patch
            .modules()
            .iter()
            .map(|module| self.module_view(module))
            .collect();
        Snapshot {
            revision: self.revision(),
            wall: self.patch.wall,
            tempo: shared.bpm(),
            beat: shared.beat(),
            clock: if shared.following_desk() {
                ClockSource::Desk
            } else {
                ClockSource::Own
            },
            on_desk: shared.to_desk(),
            heard: shared.audible(),
            seats: self.seats(),
            modules,
            cables: self.patch.cable_records(),
            levels: Levels {
                peak_l: dbfs(f64::from(self.levels.0)),
                peak_r: dbfs(f64::from(self.levels.1)),
            },
            listen: self.listen.clone(),
            faults: Faults {
                count: self.faults.count,
                recent: self.faults.recent.iter().cloned().collect(),
            },
            fingerprints: self.fingerprints.clone(),
            timing: self.timings(),
            recording: self.recorder.status().map(|status| Recording {
                path: status.path.to_string_lossy().into_owned(),
                seat: status.seat,
                seconds: status.seconds,
                dropped: status.dropped,
                sample_rate: status.sample_rate,
            }),
            rack: Some(self.patch.rows().to_vec()),
            save: self.save_mark.as_ref().map(|(mark, print)| SaveMark {
                changed: !self.still(print),
                ..mark.clone()
            }),
            log: self.log_health(),
            probes: self.probes_now(Instant::now()).0,
        }
    }

    /// What the scopes and readouts measured, as of `now`: the first
    /// [`MAX_PROBES`] of them in the wall's order, and the ids of the
    /// rest.
    fn probes_now(&self, now: Instant) -> (Vec<Probe>, Vec<String>) {
        let mut shown = Vec::new();
        let mut left_out = Vec::new();
        for module in self.patch.modules() {
            if !probe::measures(module.kind) {
                continue;
            }
            if shown.len() < MAX_PROBES {
                shown.push(self.probe_of(module, now));
            } else {
                left_out.push(module.id.clone());
            }
        }
        (shown, left_out)
    }

    /// What `module` (a scope or readout) measured last, as of `now`.
    fn probe_of(&self, module: &Module, now: Instant) -> Probe {
        let latest = self
            .readings
            .get(&module.id)
            .map(|(at, capture)| (capture, now.saturating_duration_since(*at)));
        probe::probe(
            &module.id,
            module.kind,
            latest,
            self.source_port(&module.id).map(probe::Show::of),
            self.engine.shared().bpm(),
        )
    }

    /// The output plugged into readout `id`'s input: the one its cable
    /// comes from, followed back through any scope, readout, slew limiter
    /// or sample and hold it passes through (each passes its input on).
    /// None when nothing is plugged in, or the trail goes round in a loop.
    fn source_port(&self, id: &str) -> Option<&'static PortSpec> {
        let mut module = self.patch.module(id)?;
        let mut input = 0;
        // What follows its input to its output: a scope's output a is its
        // input a, b is b; a readout's out is its in; a slew limiter's and
        // a sample and hold's out is their in, slewed or held (so pitch
        // through them is still pitch).
        let passes = |kind: Kind, port: usize| {
            if probe::measures(kind) {
                Some(port)
            } else if kind == Kind::SLEW || kind == Kind::SH {
                Some(0)
            } else {
                None
            }
        };
        // Each hop goes back one module: a trail longer than the wall is a
        // loop.
        for _ in 0..=self.patch.modules().len() {
            let cable = self.patch.cables().iter().find(|cable| {
                cable.to_module == module.id && cable.to_jack == Jack::Input(input)
            })?;
            let from = self.patch.module(&cable.from_module)?;
            let own = from.kind.spec().outputs.get(cable.from_port);
            let Some(through) = passes(from.kind, cable.from_port) else {
                return own;
            };
            let followed =
                self.patch.cables().iter().any(|cable| {
                    cable.to_module == from.id && cable.to_jack == Jack::Input(through)
                });
            if !followed {
                // Nothing goes in (a sample and hold samples its own noise):
                // what its output is said to carry.
                return own.filter(|_| !probe::measures(from.kind));
            }
            module = from;
            input = through;
        }
        None
    }

    /// Answer `probe`: what `module` (a scope or readout) measured (one
    /// not there, or measuring nothing, refused); what each of `modules`
    /// did (those not there passed over and listed); or with neither,
    /// every one on the wall (up to [`MAX_PROBES`]).
    fn probe(
        &self,
        module: Option<&str>,
        modules: &[String],
        now: Instant,
    ) -> Result<ProbeResult, WallError> {
        let dropped = self.engine.shared().probes_dropped();
        let ids: Vec<&str> = match (module, modules) {
            (None, []) => {
                let (probes, left_out) = self.probes_now(now);
                return Ok(ProbeResult {
                    probes,
                    left_out,
                    missing: Vec::new(),
                    dropped,
                });
            }
            (Some(id), []) => {
                return Ok(ProbeResult {
                    probes: vec![self.probe_of(self.measuring(id)?, now)],
                    left_out: Vec::new(),
                    missing: Vec::new(),
                    dropped,
                });
            }
            (None, ids) if ids.len() <= MAX_PROBES => ids.iter().map(String::as_str).collect(),
            (None, ids) => {
                return Err(error(
                    ErrorCode::BadRequest,
                    format!(
                        "probe reads at most {MAX_PROBES} modules at once; {} were asked for",
                        ids.len()
                    ),
                ));
            }
            (Some(_), _) => {
                return Err(error(
                    ErrorCode::BadRequest,
                    "probe takes module or modules, not both",
                ));
            }
        };
        // Several at once (a console's screens in view): one taken away by
        // another seat meanwhile is passed over and named, never fails the
        // rest.
        let mut probes = Vec::with_capacity(ids.len());
        let mut missing = Vec::new();
        for id in ids {
            match self
                .patch
                .module(id)
                .filter(|module| probe::measures(module.kind))
            {
                Some(found) => probes.push(self.probe_of(found, now)),
                None => missing.push(id.to_string()),
            }
        }
        Ok(ProbeResult {
            probes,
            left_out: Vec::new(),
            missing,
            dropped,
        })
    }

    /// Module `id`, if it is a scope or a readout.
    fn measuring(&self, id: &str) -> Result<&Module, WallError> {
        let found = self.patch.find(id)?;
        if !probe::measures(found.kind) {
            let probes: Vec<&str> = self
                .patch
                .modules()
                .iter()
                .filter(|module| probe::measures(module.kind))
                .map(|module| module.id.as_str())
                .collect();
            let which = if probes.is_empty() {
                "there are none on the wall: add a scope or a readout".to_string()
            } else {
                format!("probes: {}", probes.join(", "))
            };
            return Err(error(
                ErrorCode::BadRequest,
                format!("{id} is a {} and measures nothing; {which}", found.kind),
            ));
        }
        Ok(found)
    }

    /// Take in what the scopes and readouts handed on since the last
    /// tick, keeping each one's latest, and let go of what modules that
    /// have gone measured.
    fn take_readings(&mut self, now: Instant) {
        if let Some(ProbeRing(ring)) = self.probe_ring.as_mut() {
            while let Some(frame) = ring.try_pop() {
                let Some(id) = self.rack.id_of(frame.slot, frame.tag) else {
                    // The module has gone since (or a swap is under way).
                    continue;
                };
                let fresh = self.fresh_from.get(id).copied().unwrap_or([0; MAX_INPUTS]);
                if frame.cables < fresh[0] {
                    // Measured of what was plugged in before.
                    continue;
                }
                let mut capture = frame.capture;
                for (input, from) in fresh.iter().enumerate().skip(1) {
                    if frame.cables < *from {
                        let plugged = self
                            .wiring
                            .get(id)
                            .is_some_and(|wiring| wiring[input].is_some());
                        unmeasured(&mut capture, input, plugged);
                    }
                }
                match self.readings.get_mut(id) {
                    Some(latest) => *latest = (now, capture),
                    None => {
                        self.readings.insert(id.to_string(), (now, capture));
                    }
                }
            }
        }
        let patch = &self.patch;
        self.readings.retain(|id, _| patch.module(id).is_some());
    }

    /// Whether the change log is taking changes (see [`LogHealth`]).
    #[must_use]
    pub fn log_health(&self) -> LogHealth {
        LogHealth {
            owed: u32::try_from(self.owed.len()).unwrap_or(u32::MAX),
            error: self.log_error.clone(),
        }
    }

    fn timings(&self) -> Timings {
        let rate = self.engine.sample_rate();
        let mut modules = BTreeMap::new();
        let ids = self.timing.latency.keys().chain(self.timing.arrival.keys());
        for id in ids {
            let latency = self.timing.latency.get(id).copied().unwrap_or(0);
            modules.insert(
                id.clone(),
                ModuleTiming {
                    latency_frames: latency,
                    latency_ms: f64::from(latency) * 1_000.0 / f64::from(rate.max(1)),
                    arrival_frames: self.timing.arrival.get(id).copied().unwrap_or(0),
                },
            );
        }
        Timings {
            sample_rate: rate,
            modules,
            cables: self
                .timing
                .delays
                .iter()
                .map(|(id, delay)| (id.to_string(), u32::from(*delay)))
                .collect(),
            uncompensated: self.timing.uncompensated.clone(),
            unsteady: self.timing.unsteady.clone(),
        }
    }

    fn module_view(&self, module: &Module) -> ModuleView {
        let spec = module.kind.spec();
        // Mid-swap, the engine's slots still hold the old wall.
        let slot = self
            .rack
            .slot_of(&module.id)
            .filter(|_| self.engine.shared().unhushed() >= self.swaps_sent);
        let shared = self.engine.shared();
        let knobs = spec
            .knobs
            .iter()
            .zip(&module.knobs)
            .enumerate()
            .map(|(index, (knob, target))| {
                let value = slot
                    .and_then(|slot| shared.knob(slot, index))
                    .filter(|value| value.is_finite())
                    .map_or(*target, |value| knob.clamp(value));
                KnobView {
                    name: knob.name.to_string(),
                    value: widen(value),
                    target: widen(*target),
                    min: widen(knob.min),
                    max: widen(knob.max),
                    unit: knob.unit.name().to_string(),
                    stepped: knob.stepped,
                    display: format::knob_value(knob, value),
                    target_display: format::knob_value(knob, *target),
                }
            })
            .collect();
        ModuleView {
            id: module.id.clone(),
            kind: module.kind.name().to_string(),
            name: module.name.clone(),
            knobs,
            inputs: spec
                .inputs
                .iter()
                .map(|port| port.name.to_string())
                .collect(),
            outputs: spec
                .outputs
                .iter()
                .map(|port| port.name.to_string())
                .collect(),
        }
    }

    // -----------------------------------------------------------------
    // Housekeeping
    // -----------------------------------------------------------------

    /// A new listening arrived.
    pub fn heard(&mut self, listen: Listen) {
        self.listen = Some(listen);
    }

    /// Regular housekeeping: free what the engine retired, read the
    /// meters, report faults, keep a recording going, and save the patch
    /// if it is due. Returns the events to tell everyone: faults, and the
    /// changes the daemon made on its own.
    pub fn tick(&mut self, now: Instant) -> Vec<Event> {
        self.settle(now);
        // The saves not read yet since the start, one a tick, so the first
        // listing finds them read (see `Store::list_saves`).
        if let Some(store) = self.store.as_mut() {
            if let Err(err) = store.warm_saves(1) {
                eprintln!("kazoo-wall: the saves could not be read: {err}");
            }
        }
        if let Some(finished) = self.recorder.poll() {
            self.carry_on(&finished, "the recording ended", false);
        }
        let faults = self.engine.pump();
        // On the desk, the desk sets the tempo, and the saved tempo follows
        // it; off the desk only `tempo` changes it, which saves it itself
        // (the engine may not have heard that change yet).
        if self.desk.as_ref().is_some_and(HubLink::is_connected) {
            let bpm = clamp_bpm(self.engine.shared().bpm());
            if (bpm - self.patch.tempo).abs() > 1e-9 {
                self.patch.tempo = bpm;
                self.dirty = true;
            }
        }
        self.take_readings(now);
        let (left, right) = self.engine.shared().take_peaks();
        self.levels = (
            left.max(self.levels.0 * METER_DECAY),
            right.max(self.levels.1 * METER_DECAY),
        );
        let mut events = std::mem::take(&mut self.told);
        for fault in faults {
            let Some(id) = self.rack.id_of(fault.slot, fault.tag).map(str::to_string) else {
                // The module has gone since: nothing to tell.
                continue;
            };
            *self.faults.unreported.entry(id).or_insert(0) += 1;
        }
        let (engine_total, _) = self.engine.shared().faults();
        self.faults.count += engine_total.saturating_sub(self.faults.engine_seen);
        self.faults.engine_seen = engine_total;
        let due: Vec<String> =
            self.faults
                .unreported
                .keys()
                .filter(|id| {
                    self.faults.last_report.get(*id).is_none_or(|at| {
                        now.saturating_duration_since(*at) >= FAULT_REPORT_INTERVAL
                    })
                })
                .cloned()
                .collect();
        for id in due {
            let times = self.faults.unreported.remove(&id).unwrap_or(1);
            let summary = if times == 1 {
                format!("{id} produced NaN; reset")
            } else {
                format!("{id} produced NaN {times} times; reset each time")
            };
            eprintln!("kazoo-wall: {summary}");
            self.faults.last_report.insert(id, now);
            self.faults.recent.push_back(summary.clone());
            while self.faults.recent.len() > RECENT_FAULTS {
                self.faults.recent.pop_front();
            }
            events.push(Event::Fault {
                summary,
                seq: Some(self.revision()),
            });
        }
        let due = self
            .last_save
            .is_none_or(|at| now.saturating_duration_since(at) >= SAVE_INTERVAL);
        if self.dirty && due {
            self.save(now);
        }
        // Owed changes paid by the save are told with the rest.
        events.append(&mut self.told);
        // The log stopping, starting again, or owing more or less.
        let health = self.log_health();
        if health != self.log_told {
            self.log_told = health.clone();
            events.push(Event::Log { log: health });
        }
        events
    }

    /// Save the patch now, after the log: `patch.json` is never ahead of
    /// the changes that made it. So nothing is written while the daemon's
    /// own changes are owed to the log, or the log cannot be synced: the
    /// patch stays unsaved, and is tried again at the next save. (This is
    /// housekeeping, asked for by nobody: what fails is told in the
    /// daemon's log.)
    pub fn save(&mut self, now: Instant) {
        self.last_save = Some(now);
        if self.store.is_none() {
            self.dirty = false;
            return;
        }
        if let Err(err) = self.pay_owed() {
            eprintln!(
                "kazoo-wall: the patch was not saved, as the log is behind it: {}",
                err.message
            );
            return;
        }
        let Some(store) = self.store.as_mut() else {
            return;
        };
        // The log flushed first, within the save (see `Store::save_patch`).
        match store.save_patch(&self.patch) {
            Ok(()) => {
                self.dirty = false;
                self.log_error = None;
                if self.too_large {
                    self.too_large = false;
                    let summary = "the wall is small enough to keep again: changes are kept, \
                                   and nothing is refused for size"
                        .to_string();
                    eprintln!("kazoo-wall: {summary}");
                    self.told.push(Event::Fault {
                        summary,
                        seq: Some(self.revision()),
                    });
                }
            }
            // Never written, as it could not be read back: said once, to
            // everyone, until the wall is small enough again.
            Err(err) if err.kind() == std::io::ErrorKind::FileTooLarge => {
                self.say_too_large(&err.to_string());
            }
            Err(err) if store.log_failed() => {
                eprintln!(
                    "kazoo-wall: the change log could not be synced ({err}), so the patch was \
                     not saved"
                );
                self.log_error = Some(err.to_string());
            }
            Err(err) => {
                // The log is whole; only patch.json waits for the next save.
                self.log_error = None;
                eprintln!("kazoo-wall: the patch could not be saved: {err}");
            }
        }
    }
}

/// How long into a glide from `from` to `to` a knob of `spec` arrives, as
/// a share of the glide: a stepped knob lands on its new step half a step
/// before the end (the glide is even along the knob's travel); any other
/// knob only at the end.
fn landing(spec: &KnobSpec, from: f64, to: f64) -> f64 {
    let distance = (to - from).abs();
    match spec.curve {
        Curve::Linear if spec.stepped && distance >= 0.5 => 1.0 - 0.5 / distance,
        Curve::Linear if spec.stepped => 0.0,
        Curve::Linear | Curve::Log => 1.0,
    }
}

/// How many frames a module of `kind` with `knobs` holds its sound back at
/// `sample_rate`: an effect's own latency with those knobs applied, asked
/// of a prepared probe of its kind in `probes`; nothing for everything
/// else.
fn latency_of(
    probes: &mut HashMap<Kind, (u32, Box<dyn Effect>)>,
    kind: Kind,
    knobs: &[f32],
    sample_rate: u32,
) -> u32 {
    let Builder::Effect(effect) = kind.spec().build else {
        return 0;
    };
    // Rates are at most a few hundred kHz: exact in f32.
    let (prepared_for, probe) = probes.entry(kind).or_insert_with(|| {
        let mut probe = (effect.build)();
        probe.prepare(sample_rate as f32);
        (sample_rate, probe)
    });
    if *prepared_for != sample_rate {
        probe.prepare(sample_rate as f32);
        *prepared_for = sample_rate;
    }
    // An effect's knobs are its parameters, in order; every one is set, so
    // nothing of the last module asked about lingers.
    for (index, value) in knobs.iter().enumerate().take(effect.params.len()) {
        probe.set_param(index, *value);
    }
    u32::try_from(probe.latency()).unwrap_or(u32::MAX)
}

/// Every module kind, for `catalogue`.
#[must_use]
pub fn catalogue() -> CatalogueResult {
    CatalogueResult {
        kinds: Kind::all()
            .map(|kind| {
                let spec = kind.spec();
                KindInfo {
                    kind: spec.name.to_string(),
                    family: spec.family.to_string(),
                    about: spec.about.clone(),
                    knobs: spec
                        .knobs
                        .iter()
                        .map(|knob| KnobInfo {
                            name: knob.name.to_string(),
                            min: widen(knob.min),
                            max: widen(knob.max),
                            default: widen(knob.default),
                            unit: knob.unit.name().to_string(),
                            curve: knob.curve,
                            stepped: knob.stepped,
                            labels: knob
                                .labels
                                .iter()
                                .map(|label| (*label).to_string())
                                .collect(),
                            jack: knob.jack,
                        })
                        .collect(),
                    inputs: spec
                        .inputs
                        .iter()
                        .map(|port| PortInfo {
                            name: port.name.to_string(),
                            signal: port.signal,
                            about: port.about.to_string(),
                            pitch: port.pitch,
                        })
                        .collect(),
                    outputs: spec
                        .outputs
                        .iter()
                        .map(|port| PortInfo {
                            name: port.name.to_string(),
                            signal: port.signal,
                            about: port.about.to_string(),
                            pitch: port.pitch,
                        })
                        .collect(),
                }
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests;
