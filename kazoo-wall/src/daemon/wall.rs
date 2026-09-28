//! The daemon's state: the patch, the change log, the engine it drives, and
//! everything a request can ask about.
//!
//! Sockets live elsewhere ([`super::control`]); this is plain state with
//! methods, so it can be tested without any.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{Duration, Instant};

use kazoo_core::ipc::link::{HubLink, RequestError};
use kazoo_fx::Effect;

use super::record::{Ending, Finished, Recorder};
use super::speech::Speech;
use super::timing::{Timing, timing};
use crate::adapters::{self, Adapter};
use crate::catalogue::{Builder, Curve, Jack, Kind, KnobSpec, Signal};
use crate::change::{summarise_with, utc_now};
use crate::dsp;
use crate::engine::{
    CableTable, Command, EngineControl, Order, Route, clamp_bpm, processing_order,
};
use crate::fingerprints::Fingerprints;
use crate::format;
use crate::listen::dbfs;
use crate::patch::{Cable, Module, Patch, Words};
use crate::protocol::{
    ArrangeResult, CatalogueResult, Change, ChangeResult, ClockSource, ErrorCode, Event, Faults,
    KindInfo, KnobInfo, KnobView, Levels, Listen, ListenResult, LogPage, ModuleTiming, ModuleView,
    Place, PortInfo, RecordResult, Recording, Request, Snapshot, TempoResult, Timings, WallError,
    What, widen,
};
use crate::store::Store;
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

/// A seat's allowance of changes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bucket {
    tokens: f64,
    last: Instant,
}

impl Bucket {
    /// A full bucket at `now`.
    #[must_use]
    pub const fn new(now: Instant) -> Self {
        Self {
            tokens: FLOOD_BURST,
            last: now,
        }
    }

    /// Take one change's worth at `now`, if there is one.
    pub fn take(&mut self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.tokens = elapsed
            .mul_add(FLOOD_REFILL_PER_SECOND, self.tokens)
            .min(FLOOD_BURST);
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
    store: Option<Store>,
    dirty: bool,
    last_save: Option<Instant>,
    faults: FaultLog,
    levels: (f32, f32),
    listen: Option<Listen>,
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
    seats: BTreeMap<String, usize>,
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
            store,
            dirty: false,
            last_save: None,
            faults: FaultLog::default(),
            levels: (0.0, 0.0),
            listen: None,
            buckets: HashMap::new(),
            seats: BTreeMap::new(),
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
        self.engine = engine;
        self.desk = desk;
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

    fn load_engine(&mut self) {
        self.rack = Rack::new();
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
                let slot = self.rack.free(&module.id);
                self.send_tables();
                if let Some(slot) = slot {
                    self.engine.send(Command::Remove { slot });
                }
            }
            // Applied before the engine was loaded, (new words) already
            // handed to the player, or (a recording) started or stopped by
            // the recorder itself: nothing to mirror.
            What::Migrate { .. } | What::Speak { .. } | What::Record { .. } | What::Unknown => {}
            What::Tempo { to, desk, .. } => {
                if !desk {
                    self.engine.send(Command::Tempo(*to));
                }
            }
        }
    }

    /// Log `what` as `seat`'s change, mirror it into the engine, and move
    /// the fingerprints. Returns the change and, if the fingerprints moved,
    /// the event saying how.
    fn commit(&mut self, seat: &str, what: What, undoes: Option<u64>) -> (Change, Option<Event>) {
        self.mirror(&what);
        let mut dye = std::mem::take(&mut self.patch.dye);
        dye.touch(seat, &what, &self.patch);
        self.patch.dye = dye;
        let prints = self.patch.dye.flow(&self.patch);
        let moved = prints.changed_since(&self.fingerprints);
        self.fingerprints = prints;
        let moved = (!moved.is_empty()).then_some(Event::Fingerprints {
            seq: self.next_seq,
            modules: moved.modules,
            cables: moved.cables,
        });
        let change = Change {
            seq: self.next_seq,
            at: utc_now(),
            seat: seat.to_string(),
            summary: summarise_with(seat, &what, undoes, |id| {
                self.patch.module(id).map(|m| m.kind)
            }),
            what,
            undoes,
        };
        self.next_seq += 1;
        self.patch.last_seq = change.seq;
        self.dirty = true;
        if let Some(store) = self.store.as_mut() {
            if let Err(err) = store.append(&change) {
                eprintln!(
                    "kazoo-wall: change {} could not be logged: {err}",
                    change.seq
                );
            }
        }
        self.log.push_back(change.clone());
        while self.log.len() > MEMORY_CHANGES {
            self.log.pop_front();
        }
        (change, moved)
    }

    /// Log what the daemon itself did to the patch as it loaded it (see
    /// [`crate::migrate`]), as a change by the seat `kazoo-wall`.
    pub fn record_load(&mut self, from_version: u32, notes: Vec<String>) -> Change {
        let what = What::Migrate {
            from_version,
            to_version: crate::patch::FILE_VERSION,
            notes,
        };
        self.commit(LOADER_SEAT, what, None).0
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

    /// The latest change's sequence number.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.next_seq - 1
    }

    // -----------------------------------------------------------------
    // Requests
    // -----------------------------------------------------------------

    /// Handle a request from `seat` (after hello; connection requests are
    /// the caller's). Change requests count against the seat's flood
    /// guard.
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
        let outcome = self.answer(seat, request);
        for (module, knob, settles) in std::mem::take(&mut self.turned) {
            let due = now
                .checked_add(settles + RETIME_AFTER)
                .unwrap_or(now + RETIME_AFTER);
            // The latest turn of a knob is the one that counts.
            self.settling.insert((module, knob), due);
        }
        outcome
    }

    /// Answer `request` from `seat`, once it has passed the flood check.
    fn answer(&mut self, seat: &str, request: &Request) -> Result<Outcome, WallError> {
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
            Request::Undo { change } => self.undo(seat, *change),
            Request::Tempo { bpm } => self.tempo(seat, *bpm, None),
            Request::Speak {
                module,
                text,
                voice,
            } => self.speak(seat, module, text, voice.as_deref()),
            Request::Record { on: true } => self.start_recording(seat),
            Request::Record { on: false } => self.stop_recording(seat),
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
        let (change, fingerprints) = self.commit(seat, what, undoes);
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

    fn undo(&mut self, seat: &str, seq: u64) -> Result<Outcome, WallError> {
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
        let from = self.engine.shared().bpm();
        let linked = self.desk.as_ref().is_some_and(HubLink::is_connected);
        let desk = if linked {
            // Tempos are 20-300 BPM: exact enough in f32.
            match self
                .desk
                .as_ref()
                .map(|link| link.request_tempo(bpm as f32))
            {
                Some(Ok(())) => true,
                Some(Err(RequestError::NotConnected)) | None => false,
                Some(Err(RequestError::Full)) => {
                    return Err(error(
                        ErrorCode::SlowDown,
                        "the desk has too many requests waiting; try again",
                    ));
                }
                Some(Err(RequestError::Invalid)) => {
                    return Err(error(ErrorCode::BadRequest, "the desk refused that tempo"));
                }
            }
        } else {
            false
        };
        if !desk {
            self.patch.tempo = bpm;
        }
        let (change, fingerprints) = self.commit(
            seat,
            What::Tempo {
                from,
                to: bpm,
                desk,
            },
            undoes,
        );
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
    /// [`Outcome::pending`] gave. A seat's words that reached their module
    /// are logged as its change; saved words put back are not.
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
            let answer = match spoken.result {
                Ok(seconds) => {
                    let words = u32::try_from(spoken.words.text.split_whitespace().count())
                        .unwrap_or(u32::MAX);
                    self.patch
                        .speech
                        .insert(spoken.module.clone(), spoken.words);
                    let what = What::Speak {
                        module: spoken.module.clone(),
                        words,
                        seconds,
                    };
                    self.changed(&seat, what, None, Some(spoken.module), None)
                }
                Err(err) => Err(err),
            };
            answers.push((spoken.ticket, answer));
        }
        answers
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
        let (change, fingerprints) = self.commit(seat, what, None);
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
        let (change, fingerprints) = self.log_stop(seat, &finished, reason);
        let result = encode(&RecordResult {
            on: false,
            path: Some(finished.path.to_string_lossy().into_owned()),
            seconds: finished.seconds,
            dropped: finished.dropped,
            change: Some(change.clone()),
        })?;
        Ok(Outcome {
            result,
            change: Some(change),
            event: fingerprints,
            pending: None,
        })
    }

    /// Log `finished` as stopped by `seat`, for `reason` when the wall
    /// stopped it on its own, and say so in the daemon's log.
    fn log_stop(
        &mut self,
        seat: &str,
        finished: &Finished,
        reason: Option<String>,
    ) -> (Change, Option<Event>) {
        let what = What::Record {
            on: false,
            path: finished.path.to_string_lossy().into_owned(),
            seconds: Some(finished.seconds),
            dropped: finished.dropped,
            continues: None,
            reason,
        };
        let logged = self.commit(seat, what, None);
        eprintln!("kazoo-wall: {}", logged.0.summary);
        logged
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
        let (change, _) = self.log_stop(LOADER_SEAT, finished, Some(reason.clone()));
        self.told.push(Event::Change {
            change: Box::new(change),
        });
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
                let (change, _) = self.commit(LOADER_SEAT, what, None);
                eprintln!("kazoo-wall: {}", change.summary);
                self.told.push(Event::Change {
                    change: Box::new(change),
                });
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
    /// a recording.
    pub fn end_recording(&mut self) -> Option<Event> {
        let finished = self.recorder.stop()?;
        let reason = match &finished.ending {
            Ending::Failed(why) => why.clone(),
            Ending::Asked | Ending::Full => "the wall stopped".to_string(),
        };
        let (change, _) = self.log_stop(LOADER_SEAT, &finished, Some(reason));
        Some(Event::Change {
            change: Box::new(change),
        })
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
        let slot = self.rack.slot_of(&module.id);
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
        events
    }

    /// Save the patch now (and sync the log).
    pub fn save(&mut self, now: Instant) {
        self.last_save = Some(now);
        let Some(store) = self.store.as_ref() else {
            self.dirty = false;
            return;
        };
        match store.save_patch(&self.patch) {
            Ok(()) => self.dirty = false,
            Err(err) => eprintln!("kazoo-wall: the patch could not be saved: {err}"),
        }
        if let Err(err) = store.sync_log() {
            eprintln!("kazoo-wall: the change log could not be synced: {err}");
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
                        })
                        .collect(),
                    outputs: spec
                        .outputs
                        .iter()
                        .map(|port| PortInfo {
                            name: port.name.to_string(),
                            signal: port.signal,
                            about: port.about.to_string(),
                        })
                        .collect(),
                }
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests;
