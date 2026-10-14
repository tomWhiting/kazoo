//! The daemon's state: the patch, the change log, the engine it drives, and
//! everything a request can ask about.
//!
//! Sockets live elsewhere ([`super::control`]); this is plain state with
//! methods, so it can be tested without any.

mod batch;
mod chains;
mod cost;
mod delay_lines;
mod loops;
mod moments;
mod storage;
pub(super) mod unheard;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use kazoo_core::ipc::link::{HubLink, RequestError};
use kazoo_fx::Effect;
use ringbuf::HeapCons;
use ringbuf::traits::{Consumer, Observer};

use super::loops::Loops;
use super::record::{Ending, Finished, Recorder};
use super::speech::Speech;
use super::timing::{Timing, timing};
use crate::adapters::{self, Adapter};
use crate::catalogue::{Builder, Curve, Jack, Kind, KnobSpec, Signal};
use crate::change::{summarise_with, utc, utc_now};
use crate::dsp;
use crate::dsp::measure::{Capture, Reading};
use crate::engine::{
    CableTable, Command, EngineControl, LOAD_SECONDS, Order, ProbeFrame, Route, clamp_bpm,
};
use crate::fingerprints::Fingerprints;
use crate::format;
use crate::listen::dbfs;
use crate::patch::{
    Cable, FILE_VERSION, Module, Patch, Words, hash_bytes, never_undone, printable,
};
use crate::probe;
use crate::protocol::{
    ArrangeResult, BrokenSave, CatalogueResult, Change, ChangeResult, ClockSource, ClockStamp,
    DeleteSaveResult, EngineHealth, ErrorCode, Event, Faults, GroupsResult, KindInfo, KnobInfo,
    KnobView, Levels, Listen, ListenResult, LoadResult, LogHealth, LogPage, MAX_PROBES,
    MAX_SAVE_NAME, ModuleTiming, ModuleView, Place, PortInfo, Probe, ProbeResult, RecordResult,
    Recording, Request, SaveInfo, SaveMark, SaveResult, SavesResult, Snapshot, TempoResult,
    Timings, WallError, What, is_auto_save, valid_save_name, widen,
};
use crate::store::{LoadWriteError, Need, SaveError, Store};
use crate::{MAX_INPUTS, MAX_KNOBS};

/// The seat the daemon's own changes (bringing the patch up to date,
/// carrying a recording on or stopping one) are logged under.
pub const LOADER_SEAT: &str = "kazoo-wall";

/// Changes kept in memory for `log` and `undo`.
pub const MEMORY_CHANGES: usize = 10_000;

/// Changes a seat may make in a burst...
pub const FLOOD_BURST: f64 = 30.0;

/// ...refilled at this many per second (30 per 10 s).
pub const FLOOD_REFILL_PER_SECOND: f64 = 3.0;

/// The render load (see [`crate::engine::Shared::render_load`]) past which
/// the wall takes no more modules or cables, answering `full`.
///
/// The wall has no fixed number of modules or cables: it holds as many as
/// the machine can render. Heard modules render every sub-block, and their
/// cables carry signals through delay lines. The load is shared over heard
/// modules and the cables into them; an unheard module costs nothing until
/// a change makes it heard, and that change is costed at what it wakes
/// (see [`Wall::wake_room`]).
/// Past this share of the time it has, a busy moment elsewhere on the machine would leave the device waiting and the
/// wall would drop out for everyone listening, so a change that would add
/// more work is refused until something is taken away (or a heavy module
/// swapped for a light one). Changes that add nothing (turning a knob,
/// moving a cable without waking modules, taking things away) are never
/// refused for it.
///
/// The load is an average over about a second, so on its own it would let
/// a burst through before it moved. Three things stop one:
/// - Every addition is costed before it is taken: the load, shared over
///   the modules and cables it was measured on, times what the wall will
///   hold with the addition and everything added in the last second (which
///   the average has not caught up with yet). Past the guard, `full`.
/// - Hysteresis: once refusing, the wall takes nothing more until the load
///   is below [`RESUME_SHARE`] of the guard.
/// - Settling: above [`SETTLE_SHARE`] of the guard, an addition waits
///   (`slow_down`) until the engine has rendered [`ABSORBED_WINDOWS`]
///   averaging windows ([`crate::engine::LOAD_SECONDS`]) since the last
///   one, so the next is judged on a load that includes it.
///
/// A load (or a load undone) is costed at what its heard modules cost by
/// the bench (see [`Wall::load_room`]); a new, empty wall never is. Tom's
/// console can put a saved wall back past the guard, with a warning. Once it
/// is up (and when the engine starts), the wall takes no more (`slow_down`)
/// until its load has been measured for [`ABSORBED_WINDOWS`] windows, then
/// nothing more while past the guard.
///
/// The load is the audio thread's CPU time, not the time that passes: a
/// render the machine preempts is the machine's load, not the wall's, and
/// is not counted.
pub const LOAD_GUARD: f32 = 0.8;

/// Once refusing, the wall takes more only below this share of the guard
/// (0.7 of the time it has, at the default guard).
pub const RESUME_SHARE: f32 = 0.875;

/// Above this share of the guard (0.6 of the time it has, at the default
/// guard), additions come [`ABSORBED_WINDOWS`] averaging windows apart.
pub const SETTLE_SHARE: f32 = 0.75;

/// Averaging windows ([`LOAD_SECONDS`]) after which the render load has
/// taken in a change of the wall.
///
/// The average moves `1 - e^-n` of the way to a new level in `n` windows,
/// so 95% in three. A change is counted as not yet measured for this long,
/// and a wall just put up (or an engine just started) is measured for this
/// long before it takes more.
pub const ABSORBED_WINDOWS: u64 = 3;

/// The load above which additions to a small wall come apart.
///
/// A wall too small to be costed by share (see [`COSTED_FROM`]) takes an
/// addition every [`ABSORBED_WINDOWS`] windows above it, so a few heavy
/// modules added in a burst cannot pass the guard before the average
/// moves.
pub const SMALL_WALL_SETTLE: f32 = 0.3;

/// Modules and cables the load must have been measured on before an
/// addition is costed by its share.
///
/// On a smaller wall the master's own work, which every wall has, is most
/// of the load, and sharing it out would refuse a few modules on an idle
/// machine.
pub const COSTED_FROM: usize = 16;

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
/// wall (about 100 KB for a hundred modules) written to disk; a seat keeping its work
/// saves now and then, never in a loop.
pub const SAVE_REFILL_PER_SECOND: f64 = 0.05;

/// The most saves made by hand a seat that is not a console may bring the
/// saves to.
///
/// Beyond it, only replacing one (Tom's console) or deleting some makes
/// room. At about 100 KB for a wall of a hundred modules, about 20 MB.
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

/// Which engine slot holds which module. There are as many slots as the
/// most modules the wall has held at once; a freed slot is used again
/// before a new one is made.
#[derive(Debug, Clone, Default)]
struct Rack {
    slots: Vec<Option<(String, u32)>>,
    /// Each placed module's slot, by id.
    index: HashMap<String, usize>,
    /// The free slots below `slots.len()`, lowest first.
    free: BTreeSet<usize>,
    next_tag: u32,
}

impl Rack {
    fn slot_of(&self, id: &str) -> Option<usize> {
        self.index.get(id).copied()
    }

    /// The slot the next module placed goes in: the lowest free one, or a
    /// new one past the end.
    fn next_free(&self) -> usize {
        self.free.first().copied().unwrap_or(self.slots.len())
    }

    /// Slots needed to place `more` modules beside those placed now.
    fn slots_for(&self, more: usize) -> usize {
        self.slots.len() + more.saturating_sub(self.free.len())
    }

    /// Give `id` a free slot and a fresh tag.
    fn place(&mut self, id: &str) -> (usize, u32) {
        let slot = self.next_free();
        self.next_tag = self.next_tag.wrapping_add(1);
        let placed = Some((id.to_string(), self.next_tag));
        if self.free.remove(&slot) {
            self.slots[slot] = placed;
        } else {
            self.slots.push(placed);
        }
        self.index.insert(id.to_string(), slot);
        (slot, self.next_tag)
    }

    fn free(&mut self, id: &str) -> Option<usize> {
        let slot = self.index.remove(id)?;
        if let Some(placed) = self.slots.get_mut(slot) {
            *placed = None;
            self.free.insert(slot);
        }
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
        self.index.clear();
        self.free = (0..self.slots.len()).collect();
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

/// Whether the wall refuses more at render load `load`, having been
/// refusing (`was`) or not: past [`LOAD_GUARD`] it refuses, and once
/// refusing it goes on until the load is below [`RESUME_SHARE`] of it.
fn refusing_at(was: bool, load: f32) -> bool {
    load > LOAD_GUARD || (was && load >= LOAD_GUARD * RESUME_SHARE)
}

/// How the render load stands against [`LOAD_GUARD`] (see
/// [`Wall::render_room`]). Frames are the engine's: while an engine renders
/// nothing (a device lost, until the audio is rebuilt with a new engine
/// that measures afresh), a wall just put up waits on it.
#[derive(Debug)]
struct LoadWatch {
    /// Whether more work is refused while the render load is past the
    /// guard (see [`Wall::set_load_guard`]).
    on: bool,
    /// Whether the wall is refusing more work, until the load falls below
    /// [`RESUME_SHARE`] of the guard.
    refusing: bool,
    /// Changes the load has not yet taken in (see [`ABSORBED_WINDOWS`]):
    /// the frames the engine had rendered when each was made, and how many
    /// heard units it added (negative for removals), and the share it woke.
    recent: VecDeque<(u64, i64, f32)>,
    waking: f32,
    /// Last addition, including unheard work, for the unchanged settling rule.
    added: Option<u64>,
    /// The frames the engine will have rendered when its load has taken in
    /// the whole wall, after the wall was put up or the engine started.
    measured_from: u64,
}

impl LoadWatch {
    /// Off, with nothing measured yet: the daemon turns it on (see
    /// [`Wall::set_load_guard`]).
    const fn new() -> Self {
        Self {
            on: false,
            refusing: false,
            recent: VecDeque::new(),
            waking: 0.0,
            added: None,
            measured_from: 0,
        }
    }

    /// Start measuring afresh: the wall was put up, or the engine started,
    /// when it had rendered `rendered` frames, `window` frames to a window.
    fn restart(&mut self, rendered: u64, window: u64) {
        self.refusing = false;
        self.recent.clear();
        self.waking = 0.0;
        self.added = None;
        self.measured_from = rendered.saturating_add(window.saturating_mul(ABSORBED_WINDOWS));
    }
}

/// What a change adds to the wall: modules, cables, and speakers' words
/// (see [`Wall::room_to_grow`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Growth {
    modules: usize,
    cables: usize,
    words: usize,
}

impl Growth {
    /// Nothing added.
    const NONE: Self = Self {
        modules: 0,
        cables: 0,
        words: 0,
    };

    /// The most it adds to the wall's file.
    const fn bytes(self) -> usize {
        self.modules
            .saturating_mul(crate::store::WORST_MODULE_BYTES)
            .saturating_add(self.cables.saturating_mul(crate::store::WORST_CABLE_BYTES))
            .saturating_add(self.words.saturating_mul(crate::store::WORST_WORDS_BYTES))
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
    /// result is empty until then. Tree requests (mark, go, tree and undo
    /// of a go) return their tickets from [`Wall::tree_answers`].
    pub pending: Option<u64>,
}

/// The clock line every change is stamped on (see [`ClockStamp`]).
#[derive(Debug)]
struct ClockLine {
    /// This daemon run's start, drawn once: `<start>` in every line.
    start: u64,
    /// The count of breaks this run: `<n>` in every line.
    n: u64,
    /// The last stamp's beat and whether the desk had the clock then.
    last: Option<(f64, bool)>,
    /// Whether the daemon log has been told a change went without a stamp.
    invalid_logged: bool,
}

/// Where a [`ClockLine`] stood before a break, for [`ClockLine::rewind`].
#[derive(Debug, Clone, Copy)]
struct ClockMark {
    n: u64,
    last: Option<(f64, bool)>,
}

impl ClockLine {
    /// A run's first line, `<start>.0`, with no stamp on it yet.
    const fn new(start: u64) -> Self {
        Self {
            start,
            n: 0,
            last: None,
            invalid_logged: false,
        }
    }

    /// Start the next line: the beats after this are not on the last one's.
    const fn break_line(&mut self) {
        self.n += 1;
        self.last = None;
    }

    /// Where the line stands now.
    const fn mark(&self) -> ClockMark {
        ClockMark {
            n: self.n,
            last: self.last,
        }
    }

    /// Go back to `mark`, taken before a break for a replacement that then
    /// did not happen and was written nowhere: the music played on, so its
    /// line goes on as if the break had never been taken.
    const fn rewind(&mut self, mark: ClockMark) {
        self.n = mark.n;
        self.last = mark.last;
    }

    /// Break the line if the desk took or let go of the clock, or the beat
    /// went back, since the last reading; then remember this one. A beat
    /// that is not finite breaks the line and is never remembered.
    fn check(&mut self, beat: f64, following: bool) {
        if !beat.is_finite() {
            self.break_line();
            return;
        }
        if self
            .last
            .is_some_and(|(last, was_following)| following != was_following || beat < last - 1e-9)
        {
            self.break_line();
        }
        self.last = Some((beat, following));
    }

    /// [`Self::check`], then the stamp for a change made now; `None`, said
    /// once in the daemon log, when `beat` or `tempo` is not finite.
    fn stamp(&mut self, beat: f64, tempo: f64, following: bool) -> Option<ClockStamp> {
        self.check(beat, following);
        if !beat.is_finite() || !tempo.is_finite() {
            if !self.invalid_logged {
                eprintln!(
                    "kazoo-wall: a non-finite beat or tempo left a change without a clock stamp"
                );
                self.invalid_logged = true;
            }
            return None;
        }
        Some(ClockStamp {
            beat,
            tempo,
            line: format!("{:016x}.{}", self.start, self.n),
        })
    }
}

/// The daemon's state.
#[derive(Debug)]
pub struct Wall {
    tree: Option<crate::tree::thread::TreeHandle>,
    tree_mirror: crate::tree::Files,
    tree_error: Option<String>,
    moved_to: Option<crate::protocol::TreePlace>,
    pub tree_answered: Vec<crate::tree::thread::TreeAnswer>,
    tree_waiting: BTreeMap<u64, moments::Waiting>,
    tree_kept: Option<(Option<String>, Print)>,
    next_tree_ticket: u64,
    mark_buckets: HashMap<String, Bucket>,
    tree_buckets: HashMap<String, Bucket>,
    chain_batch: Option<Vec<What>>,
    patch: Patch,
    rack: Rack,
    engine: EngineControl,
    clock_line: ClockLine,
    desk: Option<HubLink>,
    desk_tempo: tempo_sync::DeskTempo,
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
    loops: Loops,
    loop_work: loops::Work,
    recorder: Recorder,
    /// Changes the daemon made on its own (to a recording), for the next
    /// [`Self::tick`] to tell everyone.
    told: Vec<Event>,
    /// Latency and delays as the tables were last sent.
    timing: Timing,
    /// Ids with no cable path to an out, scope, readout or loop, in added
    /// order.
    /// They are not rendered: knob glides freeze, with look reporting the
    /// target and frozen value, and resume when heard again. State (tails,
    /// envelope stages and sequencer steps) is held, never reset. Outputs
    /// keep the last block; no heard module can read an unheard source, and
    /// cables into a module heard again start with empty delay lines (see
    /// `engine::tables`). A scope, readout or loop makes its sources heard. An unheard speaker's
    /// phrase feed waits for rendering; speech synthesis completion answers
    /// the request without waiting for playback.
    unheard: Vec<String>,
    /// Heard modules and cables into them, sharing the measured render load.
    heard_units: usize,
    /// The tag of every cable that has needed delay.
    lined: BTreeMap<u32, u32>,
    /// Destinations recorded as lines are placed, until they are retired.
    line_places: BTreeMap<u32, (usize, usize)>,
    /// Destinations of cables in the last table sent.
    sent_cables: BTreeMap<u32, (usize, usize)>,
    #[cfg(test)]
    line_allocator: fn(f32, u32, bool) -> Option<crate::engine::Line>,
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
    /// How the render load stands against [`LOAD_GUARD`].
    load_watch: LoadWatch,
    /// The wall's file as last measured, in bytes, if it has been since the
    /// wall last changed hands or was saved (see [`Self::file_room`]).
    file_bytes: Option<usize>,
    /// The most the changes let through since then can have added to it.
    file_grown: usize,
    /// The revision the file was last measured at.
    file_measured_at: u64,
    /// The most bytes the wall's file may hold: [`crate::store::MAX_WALL_FILE`]
    /// (smaller in tests, to reach it).
    file_limit: usize,
    /// The cable table the next [`Self::send_tables`] fills, made (by
    /// [`Self::room_to_grow`]) before a change that needs a larger one.
    spare_table: Option<CableTable>,
    /// The engine's leaked and ignored counts as last looked at, so a move
    /// is reported once.
    engine_counts: (u64, u64),
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

/// Most outputs one readout's trail follows: a bound on the work one
/// readout's answer takes on a wall of any size. A trail longer than this
/// shows nothing.
const TRAIL_STEPS: usize = 4_096;

/// Deepest a readout's trail goes back through modules that pass their
/// input on: each is a call deeper, so this keeps the control thread's
/// stack safe on a wall of any size. Further back than this, it shows
/// nothing.
const TRAIL_DEPTH: usize = 256;

/// What [`Wall::source_show`] follows the cables with, for one answer:
/// every cable by the input it goes into; each output's answer once known
/// (so a selector of selectors is followed once each, not once per way to
/// it; only answers that did not cut a loop short, which depend on where
/// the loop was entered); the outputs on the way (so a loop ends); and
/// the outputs followed for this readout.
#[derive(Debug, Default)]
struct Trail<'a> {
    into: HashMap<(&'a str, usize), &'a Cable>,
    known: HashMap<(&'a str, usize), Option<probe::Show>>,
    on_the_way: HashSet<(&'a str, usize)>,
    steps: usize,
}

/// What a trail found at an output: how it is shown, and whether that
/// was found by cutting a loop short.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Found {
    show: Option<probe::Show>,
    cut: bool,
}

impl Found {
    /// Nothing plugged in.
    const NOTHING: Self = Self {
        show: None,
        cut: false,
    };

    /// Round a loop.
    const LOOP: Self = Self {
        show: None,
        cut: true,
    };
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
    Moment {
        to: crate::protocol::TreePlace,
        from: crate::protocol::TreePlace,
    },
    Back {
        go: u64,
        kept: Kept,
        to: crate::protocol::TreePlace,
        from: crate::protocol::TreePlace,
    },
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
            Self::Undo { .. } | Self::Back { .. } => "undo",
            Self::Moment { .. } => "go",
        }
    }

    /// The save loaded, if one was.
    fn save(&self) -> Option<&str> {
        match self {
            Self::Save(kept) => Some(&kept.name),
            Self::Empty | Self::Undo { .. } | Self::Moment { .. } | Self::Back { .. } => None,
        }
    }

    /// The load undone, if one was.
    const fn undoes(&self) -> Option<u64> {
        match self {
            Self::Undo { load, .. } => Some(*load),
            Self::Back { go, .. } => Some(*go),
            Self::Save(_) | Self::Empty | Self::Moment { .. } => None,
        }
    }

    /// The save that holds exactly the wall put up, if one does.
    fn kept(&self) -> Option<&Kept> {
        match self {
            Self::Back { kept, .. } if kept.name.starts_with("tree:") => None,
            Self::Save(kept) | Self::Undo { kept, .. } | Self::Back { kept, .. } => Some(kept),
            Self::Empty | Self::Moment { .. } => None,
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
        What::Load { .. } | What::Go { .. } => !abandoned.contains(&change.seq),
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
            What::Load { .. } | What::Go { .. } if !abandoned.contains(&change.seq) => {
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
                    What::Load { .. } | What::Go { .. } => !abandoned.contains(&change.seq),
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
            matches!(change.what, What::Load { .. } | What::Go { .. })
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
            clock: None,
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
    pub(super) fn new_with_reels(
        mut patch: Patch,
        mut engine: EngineControl,
        desk: Option<HubLink>,
        history: Vec<Change>,
        store: Option<Store>,
        reels_dir: Option<std::path::PathBuf>,
    ) -> Self {
        // A seed is built by plain adds, never numbered as changes: its
        // modules are found here, as a file's are as it loads. Every module
        // on a wall has a birth from here on.
        let mut birth_notes = Vec::new();
        patch.find_births(&mut birth_notes);
        for note in birth_notes {
            eprintln!("kazoo-wall: {note}");
        }
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
        let loops = reels_dir.map_or_else(Loops::new, Loops::with_reels_dir);
        loops.observe_takes(history.iter(), &patch);
        let mut wall = Self {
            tree: None,
            tree_mirror: crate::tree::Files::new(),
            tree_error: None,
            moved_to: None,
            tree_answered: Vec::new(),
            tree_waiting: BTreeMap::new(),
            tree_kept: None,
            next_tree_ticket: moments::TREE_TICKETS,
            mark_buckets: HashMap::new(),
            tree_buckets: HashMap::new(),
            chain_batch: None,
            speech: Speech::new(speech_cache),
            loops,
            loop_work: loops::Work::default(),
            recorder,
            told: Vec::new(),
            timing: Timing::default(),
            unheard: Vec::new(),
            heard_units: 0,
            lined: BTreeMap::new(),
            line_places: BTreeMap::new(),
            sent_cables: BTreeMap::new(),
            #[cfg(test)]
            line_allocator: delay_lines::make_line,
            line_tags: 0,
            probes: HashMap::new(),
            settled: HashMap::new(),
            turned: Vec::new(),
            settling: HashMap::new(),
            fingerprints: patch.dye.flow(&patch),
            patch,
            rack: Rack::default(),
            engine,
            clock_line: ClockLine::new(crate::chance::fresh_wall_seed()),
            desk_tempo: tempo_sync::DeskTempo::default(),
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
            load_watch: LoadWatch::new(),
            file_bytes: None,
            file_grown: 0,
            file_measured_at: 0,
            // 8 MiB: fits any usize the wall runs on.
            file_limit: usize::try_from(crate::store::MAX_WALL_FILE).unwrap_or(usize::MAX),
            spare_table: None,
            engine_counts: (0, 0),
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
        self.note_loop_restart();
        let before = self.recorder.sample_rate();
        let stopped = self.recorder.attach(&mut engine);
        self.probe_ring = engine.take_probes().map(ProbeRing);
        self.engine = engine;
        // A new engine counts its frames, leaks and refusals from nothing,
        // and has measured no load yet.
        self.engine_counts = (0, 0);
        self.spare_table = None;
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

    /// Refuse more work while rendering takes more than [`LOAD_GUARD`] of
    /// the time the wall has (`true`, every wall that plays for people), or
    /// never for the load (`false`: a test daemon in a debug build, which
    /// renders far slower than real time).
    pub const fn set_load_guard(&mut self, guard: bool) {
        self.load_watch.on = guard;
        self.load_watch.refusing = false;
    }

    /// Frames in one averaging window of the render load.
    fn load_window(&self) -> u64 {
        // The engine's rate is at most a few hundred kHz: a window is a few
        // hundred thousand frames, exact in f32 and u64.
        (LOAD_SECONDS * self.engine.sample_rate() as f32) as u64
    }

    /// Measure the render load afresh, from now: a wall was put up, or the
    /// engine started.
    fn restart_load_watch(&mut self) {
        let rendered = self.engine.shared().frames();
        let window = self.load_window();
        self.load_watch.restart(rendered, window);
    }

    /// Heard modules and the cables into them: what the render load is shared
    /// over (an unheard module renders nothing, see `unheard`).
    const fn held(&self) -> usize {
        self.heard_units
    }

    /// The desk link, while the wall has one.
    #[must_use]
    pub const fn desk_link(&self) -> Option<&HubLink> {
        self.desk.as_ref()
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
        self.loops.replace_wall();
        self.restart_load_watch();
        self.rack = Rack::default();
        self.lined.clear();
        self.line_places.clear();
        self.sent_cables.clear();
        // What the old engine's modules measured is gone with them.
        self.readings.clear();
        // Every knob goes in at its target, with nothing gliding.
        self.settled.clear();
        self.settling.clear();
        let modules = self.patch.modules().to_vec();
        self.reserve_for(modules.len());
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

    /// Size the engine for `modules` modules before anything goes in.
    fn reserve_for(&mut self, modules: usize) {
        let slots = self.rack.slots_for(modules);
        if !self.engine.reserve_slots(slots) {
            eprintln!("kazoo-wall: no memory for {modules} modules in the engine");
        }
    }

    /// Swap the wall the engine plays for the patch, as it plays: the
    /// master fades out, every module comes out and the patch's go in,
    /// knobs at their targets and nothing gliding, with fresh lines at
    /// delayed jacks, and the master fades back in (see
    /// [`Command::Hush`]). The engine's clock, the desk link and a
    /// recording under way carry on through it; the wall's own tempo
    /// becomes the patch's, unless the desk keeps the tempo.
    fn swap_engine(&mut self) {
        self.engine.begin_swap();
        self.engine.send(Command::Hush);
        self.rack.empty();
        for slot in 0..self.engine.slots() {
            self.engine.send(Command::Remove { slot });
        }
        self.speech.replace_wall();
        self.loops.replace_wall();
        self.lined.clear();
        self.line_places.clear();
        self.sent_cables.clear();
        self.settled.clear();
        self.settling.clear();
        // Faults not yet told, and readings, belong to modules that have
        // gone.
        self.faults.unreported.clear();
        self.faults.last_report.clear();
        self.readings.clear();
        // Another wall: measured afresh, and judged afresh for the load.
        self.file_bytes = None;
        self.file_grown = 0;
        self.restart_load_watch();
        let modules = self.patch.modules().to_vec();
        self.reserve_for(modules.len());
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
        self.engine.end_swap();
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
        if !self.engine.reserve_slots(self.rack.next_free() + 1) {
            // Requests that add a module make room before they change the
            // patch (see `room_to_grow`): only a patch loaded on a machine
            // with no memory for it gets here.
            eprintln!(
                "kazoo-wall: no memory for an engine slot; {} is not played",
                module.id
            );
            return false;
        }
        let (slot, tag) = self.rack.place(&module.id);
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
        } else if module.kind == Kind::LOOP {
            let (built, feed) = dsp::looper::build_loop(rate);
            self.loops.attach(&module.id, feed, rate);
            if let Some(tape) = self
                .patch
                .loop_record(&module.id)
                .and_then(|record| record.tape.clone())
            {
                self.loops.put_back(&module.id, tape);
            }
            (built, false)
        } else {
            (
                dsp::build(
                    module.kind,
                    rate,
                    crate::chance::module_seed(self.patch.seed, &module.id),
                ),
                false,
            )
        };
        self.engine.forget_cost(slot);
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
        let heard_before: BTreeSet<usize> = self
            .rack
            .index
            .iter()
            .filter(|(id, _)| !self.unheard.contains(id))
            .map(|(_, slot)| *slot)
            .collect();
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
        let outs: Vec<usize> = self
            .patch
            .modules()
            .iter()
            .filter(|module| module.kind == Kind::OUT)
            .filter_map(|module| self.rack.slot_of(&module.id))
            .collect();
        let order: Order = self.heard_order(&slots, &edges, &outs);
        let rendered: Vec<String> = order
            .slots()
            .iter()
            // u32 into usize: lossless.
            .filter_map(|slot| self.rack.id_at(*slot as usize).map(str::to_string))
            .collect();
        let latencies = self.latencies();
        self.timing = timing(&self.patch, &rendered, |id| {
            latencies.get(id).copied().unwrap_or(0)
        });
        self.timing.unsteady = self.unsteady();
        // The table room_to_grow made sure of, or one made now for the rack
        // as it stands (a re-timing, or a change that added nothing).
        let slots = self.rack.slots.len();
        let table = self
            .spare_table
            .take()
            .filter(|table| table.slots() >= slots)
            .or_else(|| CableTable::with_slots(slots));
        let Some(mut table) = table else {
            eprintln!(
                "kazoo-wall: no memory for a cable table of {slots} slots; the engine keeps \
                 the cables it has"
            );
            return;
        };
        let kinds: HashMap<&str, Kind> = self
            .patch
            .modules()
            .iter()
            .map(|module| (module.id.as_str(), module.kind))
            .collect();
        let mut routed = BTreeMap::new();
        for cable in self.patch.cables() {
            let (Some(from), Some(to)) = (
                self.rack.slot_of(&cable.from_module),
                self.rack.slot_of(&cable.to_module),
            ) else {
                continue;
            };
            let jack = match cable.to_jack {
                Jack::Input(index) => index,
                Jack::Knob(index) => MAX_INPUTS + index,
            };
            // A slot past u32 would need more modules than any machine can
            // render, and ports are below MAX_OUTPUTS (4): both always fit.
            let (Ok(from), Ok(port)) = (u32::try_from(from), u8::try_from(cable.from_port)) else {
                eprintln!("kazoo-wall: cable {} is out of any range", cable.id);
                continue;
            };
            let tag = self.lined.get(&cable.id).copied().unwrap_or(0);
            let route = Route {
                delay: self.timing.delays.get(&cable.id).copied().unwrap_or(0),
                gate: carries_gate(&kinds, cable),
                ..Route::new(from, port, cable.amount, tag)
            };
            if table.set(to, jack, Some(route)) {
                routed.insert(cable.id, (to, jack));
            } else {
                eprintln!(
                    "kazoo-wall: cable {} could not go in the engine's table and is not played",
                    cable.id
                );
            }
        }
        self.place_lines(&routed, &heard_before);
        self.apply_line_delays(&mut table, &routed);
        self.engine.send(Command::Cables(Box::new(table)));
        self.engine.send(Command::Order(Box::new(order)));
        self.sent_cables = routed;
        self.tables_sent = self.tables_sent.wrapping_add(1);
        self.note_rewiring();
    }

    /// Note each scope and readout whose inputs the table just sent moves
    /// (see [`Self::fresh_from`]), letting go of what it measured of what
    /// was plugged in before.
    fn note_rewiring(&mut self) {
        // What each scope's and readout's inputs come from, in one pass over
        // the cables.
        let measuring: HashSet<&str> = self
            .patch
            .modules()
            .iter()
            .filter(|module| probe::measures(module.kind))
            .map(|module| module.id.as_str())
            .collect();
        let mut wirings: HashMap<&str, Wiring> = measuring
            .iter()
            .map(|id| (*id, [None; MAX_INPUTS]))
            .collect();
        for cable in self.patch.cables() {
            if let (Some(wiring), Jack::Input(input), Some(from)) = (
                wirings.get_mut(cable.to_module.as_str()),
                cable.to_jack,
                self.rack.slot_of(&cable.from_module),
            ) {
                if let Some(entry) = wiring.get_mut(input) {
                    *entry = Some((from, cable.from_port, cable.amount.to_bits()));
                }
            }
        }
        let mut rewired = Vec::new();
        for (id, wiring) in wirings {
            let before = self.wiring.get(id);
            let moved: [bool; MAX_INPUTS] =
                std::array::from_fn(|input| before.is_none_or(|old| old[input] != wiring[input]));
            if moved.contains(&true) {
                rewired.push((id.to_string(), wiring, moved));
            }
        }
        let measuring: HashSet<String> = measuring.into_iter().map(str::to_string).collect();
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
        self.wiring.retain(|id, _| measuring.contains(id));
        self.fresh_from.retain(|id, _| measuring.contains(id));
    }

    /// Every module's latency in frames, with its knobs as they last
    /// settled (modules with none are left out). Probes of kinds no longer
    /// on the wall, and settled knobs of modules no longer on it, are let
    /// go.
    fn latencies(&mut self) -> BTreeMap<String, u32> {
        let rate = self.engine.sample_rate();
        let modules = self.patch.modules();
        let unheard: HashSet<&str> = self.unheard.iter().map(String::as_str).collect();
        let kinds: HashSet<Kind> = modules
            .iter()
            .filter(|module| !unheard.contains(module.id.as_str()))
            .map(|module| module.kind)
            .collect();
        let ids: HashSet<&str> = modules.iter().map(|module| module.id.as_str()).collect();
        self.probes.retain(|kind, _| kinds.contains(kind));
        self.settled.retain(|id, _| ids.contains(id.as_str()));
        let mut latencies = BTreeMap::new();
        for module in modules
            .iter()
            .filter(|module| !unheard.contains(module.id.as_str()))
        {
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
        // The knobs with a cable in them, by module: one pass over the
        // cables.
        let mut patched_knobs: HashMap<&str, Vec<usize>> = HashMap::new();
        for cable in self.patch.cables() {
            if let Jack::Knob(index) = cable.to_jack {
                patched_knobs
                    .entry(cable.to_module.as_str())
                    .or_default()
                    .push(index);
            }
        }
        let mut unsteady = Vec::new();
        let unheard: HashSet<&str> = self.unheard.iter().map(String::as_str).collect();
        for module in self
            .patch
            .modules()
            .iter()
            .filter(|module| !unheard.contains(module.id.as_str()))
        {
            let spec = module.kind.spec();
            let (Some(knobs), Some(patched)) = (
                self.settled.get(&module.id),
                patched_knobs.get(module.id.as_str()),
            ) else {
                continue;
            };
            let mut moves = false;
            for &index in patched {
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
            What::Batch { changes, .. } => self.mirror_batch(changes),
            What::Chain { changes, .. } => self.mirror_chain(changes),
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
                self.loops.forget(&module.id);
                // What it measured goes with it: brought back, it measures
                // afresh.
                self.readings.remove(&module.id);
                let slot = self.rack.free(&module.id);
                self.send_tables();
                if let Some(slot) = slot {
                    self.engine.send(Command::Remove { slot });
                }
            }
            // The loop takes what the patch now says it holds, over what
            // it recorded.
            What::Take { module, .. } | What::Erase { module, .. } => {
                let tape = self
                    .patch
                    .loop_record(module)
                    .and_then(|record| record.tape.clone());
                self.loops.replace(module, tape, false);
            }
            // Applied before the engine was loaded, (new words) already
            // handed to the player, (a recording) started or stopped by
            // the recorder itself, or (a loop's desk strip) asked of the
            // desk by KZ-L09: nothing to mirror.
            What::LoopFrom { .. }
            | What::Migrate { .. }
            | What::Speak { .. }
            | What::Record { .. }
            | What::Abandoned { .. }
            | What::Resumed { .. }
            | What::SetAside { .. }
            | What::Unknown => {}
            What::Tempo { to, desk, .. } => {
                self.patch.tempo = *to;
                if !desk {
                    self.engine.send(Command::Tempo(*to));
                }
            }
            // The patch is the new wall already.
            What::Load { .. } | What::Go { .. } => self.swap_engine(),
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
        let mut change = self.next_change(seat, what, undoes)?;
        if crate::tree::has_add(&change.what) {
            let print = self.patch.fingerprint_word();
            let salt = crate::chance::fresh_wall_seed();
            for (id, birth) in
                crate::tree::stamp_births(&mut change.what, change.seq, &change.at, print, salt)
            {
                let removed = batch::stamp_removed_birth(&mut change.what, &id, &birth);
                if !self.patch.mark_birth(&id, birth) && !removed {
                    self.unmade = true;
                    return Err(WallError {
                        step: None,
                        code: ErrorCode::Internal,
                        message: format!("newborn module '{id}' is missing"),
                    });
                }
            }
        }
        self.record(&change)?;
        if let Err(err) = self.ask_batch_tempo(&change.what) {
            self.not_made(change.seq);
            return Err(err);
        }
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
        while let Some((seat, what)) = self.owed.front().cloned() {
            let change = self.new_change(&seat, what, None);
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
    fn new_change(&mut self, seat: &str, what: What, undoes: Option<u64>) -> Change {
        let shared = self.engine.shared();
        let clock = self
            .clock_line
            .stamp(shared.beat(), shared.bpm(), shared.following_desk());
        Change {
            seq: self.next_seq,
            at: utc_now(),
            seat: seat.to_string(),
            summary: if undoes.is_none() {
                self.named_take_summary(&what)
            } else {
                None
            }
            .unwrap_or_else(|| {
                summarise_with(seat, &what, undoes, |id| {
                    self.patch.module(id).map(|m| m.kind)
                })
            }),
            what,
            undoes,
            clock,
        }
    }

    /// Make `change` (logged already, and made to the patch): mirror it
    /// into the engine, move the fingerprints, and remember it. Returns
    /// the change and, if the fingerprints moved, the event saying how.
    fn apply(&mut self, change: Change) -> (Change, Option<Event>) {
        self.accept_take(&change);
        if change.undoes.is_some() || !matches!(change.what, What::Take { .. }) {
            self.mirror(&change.what);
        }
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
                What::Load { .. } | What::Go { .. } if !abandoned() => {
                    self.memory_opens_on = gone.seq;
                }
                _ => {}
            }
        }
        self.feed_tree(&change);
        (change, moved)
    }

    /// Attach the tree in `dir` when it is a planted bare repository; the
    /// daemon never plants one. A failure is kept for the snapshot and
    /// said in the daemon's log; the wall plays on.
    pub fn find_tree(&mut self, dir: &std::path::Path) {
        match crate::tree::git::is_bare(dir) {
            Ok(true) => match crate::tree::thread::TreeHandle::attach(dir) {
                Ok(handle) => self.attach_tree(handle),
                Err(err) => self.tree_error = Some(err.to_string()),
            },
            Ok(false) => {}
            Err(err) => self.tree_error = Some(err.to_string()),
        }
        if let Some(err) = &self.tree_error {
            eprintln!("kazoo-wall: the tree could not attach: {err}");
        }
    }

    /// Take `handle` as the wall's tree: the mirror becomes the wall's
    /// files and the start commit is offered.
    pub fn attach_tree(&mut self, handle: crate::tree::thread::TreeHandle) {
        self.tree = Some(handle);
        self.whole_tree(crate::tree::thread::WholeKind::Start);
        self.tree_kept = Some((None, Print::of(&self.patch)));
    }

    fn whole_tree(&mut self, kind: crate::tree::thread::WholeKind) {
        // The changes the commit may name come after what the tree holds:
        // the changes kept in memory (the last `MEMORY_CHANGES`) have them
        // all unless the tree is further behind than that, and only then
        // is the log read from disk (every go asks for this commit).
        let first_owed = self
            .tree
            .as_ref()
            .and_then(|tree| tree.through)
            .map_or(1, |through| through.saturating_add(1));
        let in_memory = self
            .log
            .front()
            .is_none_or(|change| change.seq <= first_owed);
        let from_disk = self.store.as_ref().filter(|_| !in_memory);
        let history = match from_disk.map(Store::load_log).transpose() {
            Ok(Some(log)) => log.changes,
            Ok(None) => self.log.iter().cloned().collect(),
            Err(err) => {
                self.tree_failed(&crate::tree::TreeError::Git(format!(
                    "read change log for tree gap: {err}"
                )));
                return;
            }
        };
        let abandoned = abandoned_changes(history.iter());
        let changes = history
            .iter()
            .filter(|change| {
                change.seq <= self.made
                    && !matches!(change.what, What::Go { .. })
                    && !abandoned.contains(&change.seq)
            })
            .map(|change| change.seq)
            .collect();
        let files = match crate::tree::to_files(&self.patch.to_file()) {
            Ok(files) => files,
            Err(err) => {
                self.tree_failed(&err);
                return;
            }
        };
        let shared = self.engine.shared();
        let clock = self
            .clock_line
            .stamp(shared.beat(), shared.bpm(), shared.following_desk());
        if let Some(tree) = self.tree.as_mut() {
            if let Some(to) = &self.moved_to {
                if !tree.offer(crate::tree::thread::TreeItem::Moved { to: to.clone() }) {
                    return;
                }
                self.moved_to = None;
            }
            self.tree_mirror = files.clone();
            let accepted = tree.offer(crate::tree::thread::TreeItem::Whole {
                kind,
                changes,
                seq: self.made,
                clock,
                files,
                generation: tree.generation,
            });
            if accepted {
                tree.resync = false;
            }
        }
    }

    /// The wall did not turn into files: nothing can be sent until it does,
    /// so the tree is behind from this change. Said once while it lasts
    /// (every tick tries again).
    fn tree_failed(&mut self, err: &crate::tree::TreeError) {
        let error = err.to_string();
        if let Some(tree) = self.tree.as_mut() {
            if tree.error.as_deref() != Some(error.as_str()) {
                eprintln!(
                    "kazoo-wall: the tree could not encode change {}: {error}",
                    self.made
                );
            }
            tree.error = Some(error);
            tree.resync = true;
            tree.behind_from.get_or_insert(self.made);
        }
    }

    /// Stopping: send what the tree still lacks (trying again while its
    /// queue is full) and finish it, waiting at most
    /// [`crate::tree::thread::FINISH_WAIT`] in all.
    fn finish_tree(&mut self) {
        let deadline = Instant::now() + crate::tree::thread::FINISH_WAIT;
        loop {
            self.tick_tree();
            let Some(tree) = self.tree.as_mut() else {
                return;
            };
            if !tree.resync || Instant::now() >= deadline {
                break;
            }
            tree.wait_news(Duration::from_millis(20));
        }
        if let Some(tree) = self.tree.as_mut() {
            match tree.finish_by(deadline) {
                Ok(()) => eprintln!("kazoo-wall: the tree finished"),
                Err(err) => eprintln!("kazoo-wall: the tree did not finish: {err}"),
            }
        }
    }

    fn feed_tree(&mut self, change: &Change) {
        let go = match &change.what {
            What::Go { to, .. } => Some(to.clone()),
            _ => None,
        };
        if self.tree.as_ref().is_none_or(|tree| tree.resync) {
            if self.tree.is_some() && go.is_some() {
                self.moved_to = go;
            }
            return;
        }
        let diff = if go.is_some() {
            Vec::new()
        } else {
            match crate::tree::thread::files_diff(&self.patch.to_file(), &mut self.tree_mirror) {
                Ok(diff) => diff,
                Err(err) => {
                    self.tree_failed(&err);
                    return;
                }
            }
        };
        if let Some(tree) = self.tree.as_mut() {
            let accepted = tree.offer(crate::tree::thread::TreeItem::Change {
                change: Box::new(change.clone()),
                files: diff,
                generation: tree.generation,
            });
            if go.is_some() {
                if !accepted {
                    self.moved_to = go;
                }
                tree.generation = tree.generation.saturating_add(1);
            }
        }
        if matches!(change.what, What::Go { .. }) {
            self.whole_tree(crate::tree::thread::WholeKind::Gap);
        }
    }

    fn tick_tree(&mut self) {
        if let Some(tree) = self.tree.as_mut() {
            tree.poll();
            self.tree_answered.append(&mut tree.answered);
            if !tree.resync {
                return;
            }
            tree.generation = tree.generation.saturating_add(1);
        } else {
            return;
        }
        self.whole_tree(crate::tree::thread::WholeKind::Gap);
    }

    #[cfg(test)]
    pub fn wait_for_tree(&mut self) {
        self.tick_tree();
        match self.tree.as_mut() {
            Some(tree) => tree.wait_event(),
            None => panic!("no tree attached"),
        }
        self.tick_tree();
    }

    #[cfg(test)]
    pub(crate) fn tree_test_wait(&mut self) {
        match self.tree.as_mut() {
            Some(tree) => tree.wait_event(),
            None => panic!("no tree attached"),
        }
        self.tick_tree();
    }

    #[cfg(test)]
    pub(crate) fn tree_test_apply(&mut self, file: &crate::patch::PatchFile, change: Change) {
        self.patch = Patch::from_file(file).unwrap();
        self.apply(change);
    }

    #[cfg(test)]
    pub(crate) fn tree_test_ask(&mut self, item: crate::tree::thread::TreeItem) -> bool {
        self.tree.as_mut().is_some_and(|tree| tree.ask(item))
    }

    #[cfg(test)]
    pub(crate) fn tree_test_file(&self) -> crate::patch::PatchFile {
        self.patch.to_file()
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
        if matches!(request, Request::Tree { .. }) {
            self.tree_allow(seat, now)?;
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
        let (held, swaps) = (self.held(), self.swaps_sent);
        let outcome = self.answer((seat, console), request, now);
        if std::mem::take(&mut self.unmade) {
            self.put_back(before);
            self.load_watch.waking = 0.0;
        }
        // A wall put up is measured afresh (see `swap_engine`); any other
        // change counts towards the load not yet taken in (one the log
        // refused was put back, and counts nothing).
        if self.swaps_sent == swaps {
            self.note_held(held);
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
            | Request::Remove { .. }
            | Request::Batch { .. }
            | Request::ChainAdd { .. }
            | Request::Loop { .. } => Before::Whole(Box::new(self.patch.clone())),
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
                            | What::Restore { .. }
                            | What::Batch { .. }
                            | What::Chain { .. }
                            | What::Take { .. }
                            | What::Erase { .. }
                            | What::LoopFrom { .. },
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
            Request::Add { .. }
            | Request::Patch { .. }
            | Request::Speak { .. }
            | Request::ChainAdd { .. } => true,
            Request::Batch { changes, .. } => changes.iter().any(|change| self.grows(change)),
            Request::Group { join, .. } => *join,
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
            Request::Load { .. } | Request::Go { .. } | Request::New => true,
            Request::Undo { change } => self.log.iter().any(|logged| {
                logged.seq == *change && matches!(logged.what, What::Load { .. } | What::Go { .. })
            }),
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
        let read = unchanged;
        match request {
            Request::Look => encode(&self.snapshot()).map(read),
            Request::Catalogue => encode(&catalogue()).map(read),
            Request::Listen => encode(&ListenResult {
                listen: self.listen.clone(),
            })
            .map(read),
            Request::Probe { module, modules } => {
                // Captures handed on since the last tick, so a console
                // reading its scopes sixty times a second sees each.
                self.take_readings(now);
                self.probe(module.as_deref(), modules, now)
                    .and_then(|result| encode(&result))
                    .map(read)
            }
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
            Request::Patch { from, to, amount } => self.plug(seat, from, to, *amount),
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
            Request::Add { .. } => self.batch_add(seat, request),
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
            Request::Group {
                group,
                modules,
                join,
            } => self.group(group, modules, *join),
            Request::ChainSave {
                name,
                modules,
                overwrite,
            } => self.chain_save(name, modules, console && *overwrite),
            Request::Batch { note, changes } => self.batch(seat, note, changes, now),
            Request::ChainAdd { name } => self.chain_add(seat, name, now),
            Request::Chains => self.chains(),
            Request::Undo { change } => self.undo((seat, console), *change),
            Request::Tempo { bpm } => self.tempo(seat, *bpm, None),
            Request::Loop { module, op } => self.loop_change(seat, module, op),
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
            Request::Mark { name } => self.mark((seat, console), name, now),
            Request::Go { moment } => self.go((seat, console), moment, now),
            Request::Tree { recent } => self.tree(seat, *recent, now),
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

    /// Plug output `from` into jack `to` for `seat`. The request is checked
    /// first, then the room for it, and only then is the patch changed. A
    /// cable into a jack already in use replaces it: no more work than
    /// before.
    fn plug(
        &mut self,
        seat: &str,
        from: &str,
        to: &str,
        amount: Option<f64>,
    ) -> Result<Outcome, WallError> {
        let growth = Growth {
            cables: usize::from(self.patch.check_plug(from, to, amount)?),
            ..Growth::NONE
        };
        self.room_to_grow(growth)?;
        let what = self.patch.plug(from, to, amount, None)?;
        let cable = match &what {
            What::Patch { cable, .. } => Some(cable.id),
            _ => None,
        };
        self.changed(seat, what, None, None, cable)
    }

    /// Add a module of `kind` for `seat`: checked, then the room for it,
    /// then added.
    fn add(
        &mut self,
        seat: &str,
        kind: &str,
        name: Option<&str>,
        place: Option<&Place>,
    ) -> Result<Outcome, WallError> {
        let checked_kind = Patch::check_add(kind, name)?;
        if checked_kind == Kind::LOOP {
            self.patch.loop_room()?;
            self.file_room(crate::store::WORST_LOOP_BYTES)?;
        }
        let growth = Growth {
            modules: 1,
            ..Growth::NONE
        };
        self.room_to_grow(growth)?;
        let what = self.patch.add(kind, name, place)?;
        let module = match &what {
            What::Add { module } => Some(module.id.clone()),
            _ => None,
        };
        self.changed(seat, what, None, module, None)
    }

    /// Make sure the wall can take `growth`: its file stays within
    /// [`crate::store::MAX_WALL_FILE`] (see [`Self::file_room`]), the
    /// machine is rendering it with time to spare (see [`LOAD_GUARD`] and
    /// [`Self::render_room`]), and the engine has, or has been sent, room
    /// for it. A change that adds nothing always can.
    ///
    /// # Errors
    ///
    /// `full` when the file would be too large, while rendering takes
    /// most of the time the wall has (unless the guard is off, see
    /// [`Self::set_load_guard`]), or when there is no memory for the room;
    /// `slow_down` while the wall settles after the last addition.
    fn room_to_grow(&mut self, growth: Growth) -> Result<(), WallError> {
        // Chain capacity is reserved for the whole batch before its first add.
        if growth == Growth::NONE || self.chain_batch.is_some() {
            return Ok(());
        }
        self.file_room(growth.bytes())?;
        self.render_room(growth.modules + growth.cables, None)?;
        let slots = self.rack.slots_for(growth.modules);
        if !self.engine.reserve_slots(slots) {
            return Err(error(
                ErrorCode::Full,
                "the machine has no memory for another module",
            ));
        }
        // The cable table the change will send, made now so that a machine
        // with no memory for it refuses the change rather than playing on
        // without it.
        if self
            .spare_table
            .as_ref()
            .is_none_or(|table| table.slots() < slots)
        {
            self.spare_table = CableTable::with_slots(slots);
            if self.spare_table.is_none() {
                return Err(error(
                    ErrorCode::Full,
                    "the machine has no memory for the wall's cables",
                ));
            }
        }
        Ok(())
    }

    /// Make sure the wall's file has room for `bytes` more, at worst.
    ///
    /// The file is measured (the patch encoded as it would be saved) only
    /// when what was measured, and the most every change let through
    /// since can have added, would leave no room: a wall well inside the
    /// bound is never encoded for it. What is let through counts at its
    /// worst, and taking something away does not count down, so the sum
    /// only errs high; it is measured afresh at every save of the patch.
    /// (A change the bound does not see, the dye on a module filling as
    /// seats touch it, is a few bytes; the save catches it, see
    /// [`Self::say_too_large`].)
    ///
    /// # Errors
    ///
    /// `full` when the file would be larger than
    /// [`crate::store::MAX_WALL_FILE`]; `internal` when the patch cannot
    /// be encoded.
    fn file_room(&mut self, bytes: usize) -> Result<(), WallError> {
        let limit = self.file_limit;
        let room = |measured: usize, grown: usize| {
            measured.saturating_add(grown).saturating_add(bytes) <= limit
        };
        if self
            .file_bytes
            .is_some_and(|measured| room(measured, self.file_grown))
        {
            self.file_grown = self.file_grown.saturating_add(bytes);
            return Ok(());
        }
        // Measured at this revision, with nothing let through since: what
        // was measured is the file as it is (a seat asking again and again
        // near the bound costs no encoding).
        let fresh = self
            .file_bytes
            .filter(|_| self.file_grown == 0 && self.file_measured_at == self.revision());
        let measured = match fresh {
            Some(measured) => measured,
            None => crate::store::encode(&self.patch)
                .map_err(|err| {
                    error(
                        ErrorCode::Internal,
                        format!("the wall could not be measured: {err}"),
                    )
                })?
                .len(),
        };
        self.file_bytes = Some(measured);
        self.file_measured_at = self.revision();
        self.file_grown = 0;
        if !room(measured, 0) {
            return Err(error(
                ErrorCode::Full,
                format!(
                    "the wall's file is {} KB, and this could take it past the {} KB a wall's \
                     file may hold; take something away first",
                    measured / 1024,
                    limit / 1024
                ),
            ));
        }
        self.file_grown = bytes;
        Ok(())
    }

    /// Make sure the machine has time to render `units` more modules and
    /// cables, and the modules a staged change would wake, costed once by
    /// [`Self::wake_room`] (see [`LOAD_GUARD`]).
    ///
    /// # Errors
    ///
    /// `full` while rendering takes more than [`LOAD_GUARD`] of the time
    /// the wall has (until it is back below [`RESUME_SHARE`] of it), or
    /// when the addition, costed at the load each module and cable is
    /// taking now, would take it past; `slow_down` while the load of a
    /// wall just put up is still being measured, and above
    /// [`SETTLE_SHARE`] of the guard until the last addition has been
    /// taken in (see [`ABSORBED_WINDOWS`]).
    fn render_room(
        &mut self,
        units: usize,
        waking: Option<&cost::Waking>,
    ) -> Result<(), WallError> {
        let woken = waking.map_or(0.0, cost::Waking::share);
        if !self.load_watch.on || (units == 0 && woken <= 0.0) {
            return Ok(());
        }
        let shared = self.engine.shared();
        let load = shared.render_load();
        let rendered = shared.frames();
        let absorbed = self.load_window().saturating_mul(ABSORBED_WINDOWS);
        while self
            .load_watch
            .recent
            .front()
            .is_some_and(|(at, _, _)| rendered.saturating_sub(*at) >= absorbed)
        {
            self.load_watch.recent.pop_front();
        }
        self.load_watch.refusing = refusing_at(self.load_watch.refusing, load);
        if self.load_watch.refusing {
            return Err(error(
                ErrorCode::Full,
                format!(
                    "the wall is spending {:.0}% of its time rendering, so there is no room for \
                     more until it is below {:.0}%; take something away first",
                    load * 100.0,
                    LOAD_GUARD * RESUME_SHARE * 100.0
                ),
            ));
        }
        if rendered < self.load_watch.measured_from {
            return Err(error(
                ErrorCode::SlowDown,
                "the wall has just been put up (or its sound started), and how hard it is \
                 working is still being measured; wait a few seconds",
            ));
        }
        // The load was measured on the wall as it was before the changes
        // it has not taken in yet: share it over the modules and cables it
        // had then, and cost the wall with this addition too.
        let held = self.held();
        let lately: i64 = self
            .load_watch
            .recent
            .iter()
            .map(|(_, units, _)| units)
            .sum();
        // Counts of modules and cables: far inside i64.
        let measured_on = usize::try_from(i64::try_from(held).unwrap_or(i64::MAX) - lately)
            .unwrap_or(0)
            .max(1);
        // Counts far below 2^24: close enough in f32.
        let base = if measured_on >= COSTED_FROM {
            load / measured_on as f32 * held.saturating_add(units) as f32
        } else {
            load
        };
        let recent: f32 = self
            .load_watch
            .recent
            .iter()
            .map(|(_, _, share)| share)
            .sum();
        let projected = base + recent + woken;
        if projected > LOAD_GUARD {
            if let Some(waking) = waking.filter(|_| woken > 0.0) {
                return Err(waking.refusal(load, projected));
            }
            return Err(error(
                ErrorCode::Full,
                format!(
                    "the wall is spending {:.0}% of its time rendering, and this would take \
                         it to about {:.0}%, past the {:.0}% it keeps to; take something away \
                         first",
                    load * 100.0,
                    projected * 100.0,
                    LOAD_GUARD * 100.0
                ),
            ));
        }
        let adding_lately = self
            .load_watch
            .added
            .is_some_and(|at| rendered.saturating_sub(at) < absorbed)
            || self
                .load_watch
                .recent
                .iter()
                .any(|(_, units, share)| *units > 0 || *share > 0.0);
        // A wall too small to cost by its share can still be heavy: its
        // additions come apart from a lower load.
        let settle_from = if measured_on >= COSTED_FROM {
            LOAD_GUARD * SETTLE_SHARE
        } else {
            SMALL_WALL_SETTLE
        };
        if load > settle_from && adding_lately {
            return Err(error(
                ErrorCode::SlowDown,
                format!(
                    "the wall is spending {:.0}% of its time rendering, so it takes an addition \
                     every few seconds, to measure each; wait a moment",
                    load * 100.0
                ),
            ));
        }
        Ok(())
    }

    /// Count a change that took the wall from `before` modules and cables
    /// to as many as it holds now, towards the render load the averaging
    /// has not taken in yet (see [`Self::render_room`]).
    fn note_held(&mut self, before: usize) {
        let delta = i64::try_from(self.held()).unwrap_or(i64::MAX)
            - i64::try_from(before).unwrap_or(i64::MAX);
        let waking = std::mem::take(&mut self.load_watch.waking);
        if delta != 0 || waking > 0.0 {
            let rendered = self.engine.shared().frames();
            self.load_watch.recent.push_back((rendered, delta, waking));
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
        if self.chain_batch.is_some() {
            return self.collect_chain_change(what, module.as_deref(), cable);
        }
        let module = module.or_else(|| match &what {
            What::Restore { module, .. } => Some(module.id.clone()),
            _ => None,
        });
        let cable = cable.or(match &what {
            What::Patch { cable, .. } => Some(cable.id),
            _ => None,
        });
        if cost::can_wake(&what) {
            self.wake_room(&what)?;
        }
        let (change, fingerprints) = self.commit(seat, what, undoes)?;
        self.note_addition(&change.what);
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

    /// Put modules in a group or take them out: not a change (like a
    /// move), but saved with the patch and told to every subscriber.
    fn group(&mut self, name: &str, modules: &[String], join: bool) -> Result<Outcome, WallError> {
        if self.patch.group(name, modules, join)? {
            self.dirty = true;
        }
        let groups = self.patch.groups().clone();
        let result = encode(&GroupsResult {
            groups: groups.clone(),
        })?;
        Ok(Outcome {
            result,
            change: None,
            event: Some(Event::Groups { groups }),
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
        if let What::Go {
            backup,
            fingerprint,
            from,
            ..
        } = &what
        {
            return self.undo_go(
                (seat, console),
                seq,
                backup.clone(),
                fingerprint.clone(),
                from.clone(),
            );
        }
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
        if matches!(what, What::Batch { .. }) {
            return self.undo_batch_change(seat, seq, &what);
        }
        // Undoing a removal brings a module, its cables and its words back,
        // and undoing an unplugging brings a cable back: more work, like
        // any other addition. Every other undo adds nothing.
        let growth = match &what {
            What::Chain { changes, .. } => chains::undo_growth(changes),
            What::Remove { cables, .. } => Growth {
                modules: 1,
                cables: cables.len(),
                words: 1,
            },
            What::Unpatch { .. } => Growth {
                cables: 1,
                ..Growth::NONE
            },
            _ => Growth::NONE,
        };
        self.room_to_grow(growth)?;
        self.check_take_undo(&what)?;
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
        if self.chain_batch.is_some() {
            let from = self.batch_tempo_from(desk);
            self.patch.tempo = bpm;
            return self.changed(
                seat,
                What::Tempo {
                    from,
                    to: bpm,
                    desk,
                },
                undoes,
                None,
                None,
            );
        }
        if desk {
            // A desk move not yet heard by a tick is logged before the ask,
            // so the tick after the ask hears nothing new until the desk
            // answers it.
            self.sync_desk_tempo(from);
        }
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
                    RequestError::Unsupported => error(
                        ErrorCode::BadRequest,
                        "the desk is too old to take a tempo from the wall",
                    ),
                });
            }
            self.desk_tempo.asked(bpm);
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
                What::Load { .. } | What::Go { .. } => !abandoned.contains(&change.seq),
                What::Resumed { .. } => true,
                _ => false,
            })
            .map_or_else(String::new, |up| {
                if matches!(up.what, What::Load { .. } | What::Go { .. }) {
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
        if self.store.is_none() {
            return Err(no_saves());
        }
        if let Some(pending) = self.begin_loop_save(seat, name, overwrite, now) {
            return Ok(pending);
        }
        self.write_loop_save(seat, name, overwrite)
    }

    fn write_loop_save(
        &mut self,
        seat: &str,
        name: &str,
        overwrite: bool,
    ) -> Result<Outcome, WallError> {
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
        let mut notes = unsynced_note(written.unsynced.as_ref(), "in place", "take it back");
        self.save_tree_mark(&stored, &save.saved_at, &mut notes);
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
        self.refresh_loop_refs(true);
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
        let mut empty = Patch::empty(self.patch.tempo);
        empty.seed = crate::chance::fresh_wall_seed();
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
        let read = self.read_backup(seq, backup, fingerprint)?;
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
            }
            | What::Go {
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
        self.refresh_loop_refs(true);
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
        let warning = self.load_room_as(console, &incoming, source, &mut notes)?;
        let how = source.how();
        // The daemon's own changes owed to the log go first (they take
        // their numbers before this one), or nothing is replaced.
        self.pay_owed()?;
        let (backup, backup_bytes) = self
            .keeping_before(source)
            .map_err(|err| not_kept(seat, how, &err))?;
        notes.extend(self.loops.lost_notes(
            self.engine.shared().frames(),
            self.engine.sample_rate(),
            &backup.name,
        ));
        let playing = self.playing_after(&incoming, &mut notes);
        let incoming_print = Print::of(&incoming);
        let what = source.replacement(&backup, &incoming, playing, console);
        // The wall put up starts a clock line with this change. Should the
        // change be written nowhere, nothing was replaced and the line is
        // put back; once it may be in the log, its line is never reused.
        let unbroken = self.clock_line.mark();
        self.clock_line.break_line();
        let change = self.new_change(seat, what, source.undoes());
        // The wall put up is named by the change that puts it up, and goes
        // to patch.json as of that change.
        incoming.last_seq = change.seq;
        incoming.wall = change.seq;
        let Some(store) = self.store.as_mut() else {
            self.clock_line.rewind(unbroken);
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
            Err(other) => {
                self.clock_line.rewind(unbroken);
                return Err(not_written(seat, how, &other));
            }
        };
        // In the log for good: the load happens.
        self.patch = incoming;
        let (change, _) = self.apply(change);
        eprintln!("kazoo-wall: {}", change.summary);
        if let Some(warning) = warning {
            eprintln!("kazoo-wall: {warning}");
        }
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
        self.remember_tree_moment(source, incoming_print.clone());
        self.kept_in = source.kept().map(|kept| (kept.clone(), incoming_print));
        self.prune();
        notes.push(format!(
            "lands on bar {:.0}",
            self.engine.landing_beat() / 4.0 + 1.0
        ));
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
        self.refresh_loop_refs(true);
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
        // Words make the wall's file larger: the room for them is judged as
        // they are logged.
        self.file_room(crate::store::WORST_WORDS_BYTES)?;
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
            return Err(self.unheard_refusal(&module, err));
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
        self.finish_tree();
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
        self.loops.observe_takes(self.log.iter(), &self.patch);
        let modules: Vec<_> = self
            .patch
            .modules()
            .iter()
            .map(|module| self.module_view(module))
            .collect();
        Snapshot {
            loops_meet: super::loops::meet::loops_meet(
                modules.iter().filter_map(|module| module.tape.as_ref()),
                shared.bpm(),
                shared.beat(),
            ),
            checkpoints_paused: self.loops.checkpoints_paused,
            revision: self.revision(),
            wall: self.patch.wall,
            seed: self.patch.seed,
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
                recent: self
                    .faults
                    .recent
                    .iter()
                    .cloned()
                    .chain(self.loops.unassigned_notes(&self.patch).map(str::to_owned))
                    .collect(),
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
            groups: self.patch.groups().clone(),
            save: self.save_mark.as_ref().map(|(mark, print)| SaveMark {
                changed: !self.still(print),
                ..mark.clone()
            }),
            log: self.log_health(),
            tree: Some(self.tree.as_ref().map_or_else(
                || crate::protocol::TreeHealth {
                    error: self.tree_error.clone(),
                    ..crate::protocol::TreeHealth::default()
                },
                crate::tree::thread::TreeHandle::health,
            )),
            probes: self.probes_now(Instant::now()).0,
            engine: Some(EngineHealth {
                load: f64::from(shared.render_load()),
                load_guard: self.load_watch.on.then_some(f64::from(LOAD_GUARD)),
                // As an addition would find it now, not as the last one did.
                refusing: self.load_watch.on
                    && refusing_at(self.load_watch.refusing, shared.render_load()),
                measuring: self.load_watch.on && shared.frames() < self.load_watch.measured_from,
                leaked: shared.leaked(),
                ignored: shared.ignored(),
            }),
        }
    }

    /// What the scopes and readouts measured, as of `now`: the first
    /// [`MAX_PROBES`] of them in the wall's order, and the ids of the
    /// rest.
    fn probes_now(&self, now: Instant) -> (Vec<Probe>, Vec<String>) {
        let mut trail = self.trail();
        let mut shown = Vec::new();
        let mut left_out = Vec::new();
        for module in self.patch.modules() {
            if !probe::measures(module.kind) {
                continue;
            }
            if shown.len() < MAX_PROBES {
                shown.push(self.probe_of(module, now, &mut trail));
            } else {
                left_out.push(module.id.clone());
            }
        }
        (shown, left_out)
    }

    /// What `module` (a scope or readout) measured last, as of `now`;
    /// `trail` follows readouts' cables (see [`Self::source_show`]).
    fn probe_of<'a>(&'a self, module: &Module, now: Instant, trail: &mut Trail<'a>) -> Probe {
        let latest = self
            .readings
            .get(&module.id)
            .map(|(at, capture)| (capture, now.saturating_duration_since(*at)));
        // Only a readout shows by what is plugged in.
        let source = (module.kind == Kind::READOUT)
            .then(|| self.source_show(&module.id, trail))
            .flatten();
        probe::probe(
            &module.id,
            module.kind,
            latest,
            source,
            self.engine.shared().bpm(),
        )
    }

    /// A trail to follow the wall's cables with, for one answer: every
    /// cable by the input it goes into, made once.
    fn trail(&self) -> Trail<'_> {
        Trail {
            into: self
                .patch
                .cables()
                .iter()
                .filter_map(|cable| match cable.to_jack {
                    Jack::Input(input) => Some(((cable.to_module.as_str(), input), cable)),
                    Jack::Knob(_) => None,
                })
                .collect(),
            ..Trail::default()
        }
    }

    /// How what is plugged into readout `id`'s input is shown at `auto`
    /// (see [`probe::Show`]): by the output its cable comes from, followed
    /// back through what passes its input on (see [`Self::show_out`]).
    /// None when nothing is plugged in, or the trail only goes round in a
    /// loop.
    fn source_show<'a>(&'a self, id: &str, trail: &mut Trail<'a>) -> Option<probe::Show> {
        trail.steps = 0;
        self.show_into(id, 0, trail).show
    }

    /// How what arrives at `module`'s input `input` is shown.
    fn show_into<'a>(&'a self, module: &str, input: usize, trail: &mut Trail<'a>) -> Found {
        let cable = trail.into.get(&(module, input)).copied();
        cable.map_or(Found::NOTHING, |cable| {
            self.show_out(&cable.from_module, cable.from_port, trail)
        })
    }

    /// How what output `port` of `module` carries is shown.
    ///
    /// Most outputs say so themselves (see
    /// [`crate::catalogue::PortSpec::pitch`]). Some pass on what comes in,
    /// and are followed back to it: a scope's output a is its input a, b
    /// is b; a readout's out is its in; a slew limiter's and a sample and
    /// hold's out is their in, slewed or held; a switch's outputs are its
    /// in, sent on; a selector's out is whichever input is chosen, so it
    /// is followed when every input plugged in is shown alike; and the
    /// sums (`mix`'s out, `math`'s `sum`, `min` and `max`) of a pitch and
    /// anything (a transposition, a vibrato) are still pitch (`math`'s
    /// `abs` is a plain CV). One of those with nothing plugged in shows
    /// what its output is said to carry (a sample and hold of its own noise
    /// is a plain CV, a switch with no input sends a steady 1.0), but a
    /// scope or readout, which passes on silence, shows nothing. An input
    /// that only leads round a loop says nothing either way.
    fn show_out<'a>(&'a self, module: &str, port: usize, trail: &mut Trail<'a>) -> Found {
        let Some(from) = self.patch.module(module) else {
            return Found::NOTHING;
        };
        let key = (from.id.as_str(), port);
        if let Some(known) = trail.known.get(&key) {
            return Found {
                show: *known,
                cut: false,
            };
        }
        trail.steps += 1;
        // The outputs on the way are the calls deep this one is.
        let too_far = trail.steps > TRAIL_STEPS || trail.on_the_way.len() >= TRAIL_DEPTH;
        if too_far || !trail.on_the_way.insert(key) {
            // Round a loop (or a trail too long to follow): nothing here,
            // and not kept, as it depends on where the trail came from.
            return Found::LOOP;
        }
        let found = self.shown_out(from, port, trail);
        trail.on_the_way.remove(&key);
        // What was found by cutting a loop short depends on where the loop
        // was entered: only what was not is kept for the next readout.
        if !found.cut {
            trail.known.insert(key, found.show);
        }
        found
    }

    /// [`Self::show_out`] of `from`, once the trail has been checked.
    fn shown_out<'a>(&'a self, from: &'a Module, port: usize, trail: &mut Trail<'a>) -> Found {
        let own = Found {
            show: from.kind.spec().outputs.get(port).map(probe::Show::of),
            cut: false,
        };
        let kind = from.kind;
        // A scope passes input `port` to output `port`.
        let same = [port];
        let inputs: &[usize] = if probe::measures(kind) {
            &same
        } else if kind == Kind::SLEW || kind == Kind::SH || kind == Kind::SWITCH {
            &[0]
        } else if kind == Kind::SELECT || kind == Kind::MIX {
            &[0, 1, 2, 3]
        } else if kind == Kind::MATH && port <= 2 {
            &[0, 1]
        } else {
            return own;
        };
        let plugged: Vec<usize> = inputs
            .iter()
            .copied()
            .filter(|&input| trail.into.contains_key(&(from.id.as_str(), input)))
            .collect();
        if plugged.is_empty() {
            return Found {
                show: own.show.filter(|_| !probe::measures(kind)),
                cut: false,
            };
        }
        let mut cut = false;
        let mut shown = Vec::with_capacity(plugged.len());
        for input in plugged {
            let found = self.show_into(&from.id, input, trail);
            cut |= found.cut;
            // An input that only leads round a loop says nothing.
            if !(found.cut && found.show.is_none()) {
                shown.push(found.show);
            }
        }
        let show = if shown.is_empty() {
            None
        } else if kind == Kind::MATH || kind == Kind::MIX {
            if shown.contains(&Some(probe::Show::Pitch)) {
                Some(probe::Show::Pitch)
            } else {
                own.show
            }
        } else if shown.iter().all(|show| *show == shown[0]) {
            shown[0]
        } else {
            own.show
        };
        Found { show, cut }
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
                    probes: vec![self.probe_of(self.measuring(id)?, now, &mut self.trail())],
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
        let mut trail = self.trail();
        for id in ids {
            match self
                .patch
                .module(id)
                .filter(|module| probe::measures(module.kind))
            {
                Some(found) => probes.push(self.probe_of(found, now, &mut trail)),
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
            unheard: self.unheard.clone(),
        }
    }

    fn module_view(&self, module: &Module) -> ModuleView {
        let spec = module.kind.spec();
        // Mid-swap, the engine's slots still hold the old wall.
        let slot = self
            .rack
            .slot_of(&module.id)
            .filter(|_| self.engine.shared().unhushed() >= self.swaps_sent);
        let knobs = spec
            .knobs
            .iter()
            .zip(&module.knobs)
            .enumerate()
            .map(|(index, (knob, target))| {
                let value = slot
                    .and_then(|slot| self.engine.knob(slot, index))
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
            tape: self
                .loops
                .view(&module.id, &self.patch, self.engine.shared().origin()),
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
        self.tick_tree();
        let shared = self.engine.shared();
        self.clock_line
            .check(shared.beat(), shared.following_desk());
        self.tick_loops(now);
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
            let bpm = self.engine.shared().bpm();
            if bpm.is_finite() {
                self.sync_desk_tempo(clamp_bpm(bpm));
            }
        } else {
            self.desk_tempo.disconnected();
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
        if let Some(summary) = self.engine_trouble() {
            eprintln!("kazoo-wall: {summary}");
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

    /// What the engine could not do since last asked, in a sentence: items
    /// it had to leak rather than free (the retire ring was full), and
    /// commands it refused (a room there was no memory for). Never
    /// expected; each is said once.
    fn engine_trouble(&mut self) -> Option<String> {
        let shared = self.engine.shared();
        let counts = (shared.leaked(), shared.ignored());
        let (leaked, ignored) = (
            counts.0.saturating_sub(self.engine_counts.0),
            counts.1.saturating_sub(self.engine_counts.1),
        );
        self.engine_counts = counts;
        let mut said = Vec::new();
        if leaked > 0 {
            said.push(format!(
                "the engine kept {leaked} retired items it could not hand back to be freed"
            ));
        }
        if ignored > 0 {
            said.push(format!(
                "the engine refused {ignored} commands (no memory for the room they needed, \
                 or a slot, knob or value out of range): some of the wall may not be playing"
            ));
        }
        (!said.is_empty()).then(|| said.join("; "))
    }

    /// Save the patch now, after the log: `patch.json` is never ahead of
    /// the changes that made it. So nothing is written while the daemon's
    /// own changes are owed to the log, or the log cannot be synced: the
    /// patch stays unsaved, and is tried again at the next save. (This is
    /// housekeeping, asked for by nobody: what fails is told in the
    /// daemon's log.)
    pub fn save(&mut self, now: Instant) {
        self.last_save = Some(now);
        // The file is measured afresh at the next change that adds to it.
        self.file_bytes = None;
        self.file_grown = 0;
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

/// Whether `cable` carries a gate (it comes from a gate output, or goes
/// into a gate input), which must switch delay between pulses rather than
/// fade. `kinds` is every module's kind, by id.
fn carries_gate(kinds: &HashMap<&str, Kind>, cable: &Cable) -> bool {
    let from_gate = kinds.get(cable.from_module.as_str()).is_some_and(|kind| {
        kind.spec()
            .outputs
            .get(cable.from_port)
            .map(|port| port.signal)
            == Some(Signal::Gate)
    });
    from_gate
        || match cable.to_jack {
            Jack::Input(index) => kinds.get(cable.to_module.as_str()).is_some_and(|kind| {
                kind.spec().inputs.get(index).map(|port| port.signal) == Some(Signal::Gate)
            }),
            Jack::Knob(_) => false,
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

#[cfg(test)]
mod clock_tests;

mod tempo_sync;
