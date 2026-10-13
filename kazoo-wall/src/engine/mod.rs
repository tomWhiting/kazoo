//! The realtime engine.
//!
//! [`engine`] returns two halves. The [`Engine`] moves into the audio
//! callback (or the headless render loop): it owns the module slots and
//! their port buffers (its [`Room`]), a delay line per cable, the cable and
//! order tables, the master chain and the wall's clock, and renders in
//! sub-blocks of [`SUB_BLOCK`] frames. It never allocates, frees, locks or
//! does I/O.
//!
//! The [`EngineControl`] stays on the control side, the only place that
//! allocates: it sends [`Command`]s over a lock-free ring, frees what the
//! engine hands back over the retire ring, and collects fault reports. The
//! engine publishes its clock, meters, render load and current knob values
//! through atomics ([`Shared`], and the room's knob cells), and what the
//! scopes and readouts measured as [`ProbeFrame`]s over a ring of its own.
//!
//! There is no fixed number of slots or lines. The engine starts with room
//! for [`INITIAL_SLOTS`] modules and [`INITIAL_LINES`] cables; when a
//! command needs more, [`EngineControl::send`] first builds a larger room
//! or line table on the control side (twice the size, or what is needed if
//! more) and sends it ahead of the command. Slots and lines live in pages
//! ([`room::SLOTS_PER_PAGE`], [`lines::LINES_PER_PAGE`]); the engine moves
//! its pages into the new table, one pointer each, and retires the old
//! table, so growing costs the audio thread a few hundred pointer moves at
//! most, and never a copy of anything a slot or line holds.
//!
//! The engine tolerates any order of commands: a cable or order entry naming
//! an empty slot reads silence or is skipped, so the control side never needs
//! a transaction. To swap the whole wall for another without a click, it
//! brackets the swap with [`Command::Hush`] and [`Command::Unhush`]: the
//! master fades out, the swap is applied in silence, and it fades back in.

pub mod lines;
pub mod master;
pub mod room;
pub mod tables;

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use kazoo_core::ipc::client::HubMessage;
use kazoo_core::ipc::follow::{TransportChange, TransportFollower};
use kazoo_core::ipc::link::HubLinkAudio;
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};

use crate::catalogue::{Kind, KnobSpec};
use crate::dsp::measure::Capture;
use crate::dsp::{Block, Io, Module, Sweep, Tick};
use crate::{MAX_INPUTS, MAX_KNOBS, MAX_OUTPUTS, SUB_BLOCK};
pub use lines::MoreLines;
pub use master::CEILING;
pub use room::{KnobCells, Room};
pub use tables::{CableTable, MAX_CABLE_DELAY, Order, Route, processing_order};

/// Module slots a new engine has room for before it first grows.
pub const INITIAL_SLOTS: usize = 64;

/// Delay lines (one per cable) a new engine has before it first grows.
pub const INITIAL_LINES: usize = 256;

/// Seconds over which the render load is averaged (see
/// [`Shared::render_load`]): long enough that one slow block (a room
/// growing, the machine busy for a moment) does not count as the wall
/// being full.
pub const LOAD_SECONDS: f32 = 1.0;

/// Commands waiting for the engine.
const COMMAND_BACKLOG: usize = 512;

/// Items waiting to be freed: every command retires at most one (see
/// [`EngineControl::flush`]).
const RETIRE_BACKLOG: usize = COMMAND_BACKLOG * 2;

/// Fault reports waiting for the control side.
const FAULT_BACKLOG: usize = 64;

/// Captures from scopes and readouts waiting for the control side.
///
/// A scope hands on at most 60 a second and a readout 10, and the control
/// side takes them every 20 ms tick (and on every `probe`): 64 scopes
/// fill it in about 130 ms, room for a busy moment of six missed ticks.
/// Beyond it captures are dropped (and counted): the next one is as good.
pub const PROBE_BACKLOG: usize = 512;

/// Seconds of stereo the record ring holds: how far the recording's writer
/// may fall behind (a slow disk, a busy machine) before samples are lost.
pub const RECORD_SECONDS: usize = 4;

/// Largest block handed to the desk at once, in frames.
pub const MAX_CHUNK_FRAMES: usize = 4096;

/// Slowest and fastest tempo the wall keeps.
pub const MIN_BPM: f64 = 20.0;
/// See [`MIN_BPM`].
pub const MAX_BPM: f64 = 300.0;

/// Something for the engine to do.
#[derive(Debug)]
pub enum Command {
    /// Put `module` in `slot`, with its knobs at `knobs`. A module already
    /// there is retired.
    Insert {
        /// Slot. [`EngineControl::send`] makes room for it first.
        slot: usize,
        /// The control side's name for this placement, echoed in faults.
        tag: u32,
        /// The module's kind.
        kind: Kind,
        /// The processor.
        module: Box<dyn Module>,
        /// Knob values, in catalogue order.
        knobs: [f32; MAX_KNOBS],
    },
    /// Take the module out of `slot`; it is retired.
    Remove {
        /// Slot.
        slot: usize,
    },
    /// Swap in a new cable table; the old one is retired.
    /// [`EngineControl::send`] makes sure there is a line for every cable
    /// in it first.
    Cables(Box<CableTable>),
    /// Swap in a new render order; the old one is retired.
    Order(Box<Order>),
    /// Move every slot into a larger room; the old room is retired, empty.
    /// Sent by [`EngineControl`] as it needs more slots.
    Room(Box<Room>),
    /// Move every delay line into a larger table; the old table is
    /// retired, empty. Sent by [`EngineControl`] as it needs more lines.
    Lines(Box<MoreLines>),
    /// Glide a knob to `target` over `glide_frames` frames.
    Knob {
        /// Slot.
        slot: usize,
        /// Knob index, below [`MAX_KNOBS`].
        knob: usize,
        /// Where it ends up.
        target: f32,
        /// How long it takes; 0 is at once.
        glide_frames: u32,
    },
    /// Set the wall's own tempo. While following a playing desk, the desk's
    /// next change overrides it.
    Tempo(f64),
    /// Fade the master out over [`SWAP_FADE_SECONDS`], and hold every
    /// command after this one until it is silent: the commands between a
    /// `Hush` and its [`Command::Unhush`] (a whole wall taken out and
    /// another put in) are all heard at once, in silence, never half done.
    Hush,
    /// Fade the master back in over [`SWAP_FADE_SECONDS`].
    Unhush,
}

/// Something the engine hands back to be freed off the audio thread.
#[derive(Debug)]
pub enum Retired {
    /// A module taken out or replaced.
    Module(Box<dyn Module>),
    /// A replaced cable table.
    Cables(Box<CableTable>),
    /// A replaced render order.
    Order(Box<Order>),
    /// A room outgrown (its modules moved on), or one too small to use.
    Room(Box<Room>),
    /// A line table outgrown (its lines moved on), or one too small to
    /// use.
    Lines(Box<MoreLines>),
}

/// What a scope or readout measured, from engine slot `slot`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProbeFrame {
    /// The module's slot.
    pub slot: usize,
    /// The tag it was inserted with.
    pub tag: u32,
    /// How many cable tables the engine had taken in when it was handed
    /// on: everything in it was measured with that table's cables (a
    /// module whose inputs a new table moves starts afresh, see
    /// [`Module::rewired`]).
    pub cables: u64,
    /// What it measured.
    pub capture: Capture,
}

/// A module produced a non-finite sample: it was silenced and reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fault {
    /// The module's slot.
    pub slot: usize,
    /// The tag it was inserted with.
    pub tag: u32,
}

/// What the engine publishes for the control side.
#[derive(Debug)]
pub struct Shared {
    bpm: AtomicU64,
    beat: AtomicU64,
    following_desk: AtomicBool,
    to_desk: AtomicBool,
    audible: AtomicBool,
    peak_left: AtomicU32,
    peak_right: AtomicU32,
    /// Slots in the engine's room, stored once the room is in place.
    slots: AtomicUsize,
    /// Delay lines the engine holds, stored once they are in place.
    lines: AtomicUsize,
    /// The render load, as f32 bits.
    load: AtomicU32,
    frames: AtomicU64,
    faults: AtomicU64,
    faults_unreported: AtomicU64,
    leaked: AtomicU64,
    listen_dropped: AtomicU64,
    probes_dropped: AtomicU64,
    recording: AtomicBool,
    record_dropped: AtomicU64,
    syncs_lost: AtomicU64,
    ignored: AtomicU64,
    unhushed: AtomicU64,
}

impl Shared {
    const fn new(bpm: f64, beat: f64, audible: bool, slots: usize, lines: usize) -> Self {
        Self {
            bpm: AtomicU64::new(bpm.to_bits()),
            beat: AtomicU64::new(beat.to_bits()),
            following_desk: AtomicBool::new(false),
            to_desk: AtomicBool::new(false),
            audible: AtomicBool::new(audible),
            peak_left: AtomicU32::new(0),
            peak_right: AtomicU32::new(0),
            slots: AtomicUsize::new(slots),
            lines: AtomicUsize::new(lines),
            load: AtomicU32::new(0.0_f32.to_bits()),
            frames: AtomicU64::new(0),
            faults: AtomicU64::new(0),
            faults_unreported: AtomicU64::new(0),
            leaked: AtomicU64::new(0),
            listen_dropped: AtomicU64::new(0),
            probes_dropped: AtomicU64::new(0),
            recording: AtomicBool::new(false),
            record_dropped: AtomicU64::new(0),
            syncs_lost: AtomicU64::new(0),
            ignored: AtomicU64::new(0),
            unhushed: AtomicU64::new(0),
        }
    }

    /// The tempo, in beats per minute.
    #[must_use]
    pub fn bpm(&self) -> f64 {
        f64::from_bits(self.bpm.load(Ordering::Relaxed))
    }

    /// The song position, in beats.
    #[must_use]
    pub fn beat(&self) -> f64 {
        f64::from_bits(self.beat.load(Ordering::Relaxed))
    }

    /// Whether the wall is following a playing desk's beat.
    #[must_use]
    pub fn following_desk(&self) -> bool {
        self.following_desk.load(Ordering::Relaxed)
    }

    /// Whether the last block went to the desk (the device got silence).
    #[must_use]
    pub fn to_desk(&self) -> bool {
        self.to_desk.load(Ordering::Relaxed)
    }

    /// Whether the wall's output is heard: when not, it goes on playing
    /// (and listening, and feeding the meters) but sends silence to the
    /// device and the desk.
    #[must_use]
    pub fn audible(&self) -> bool {
        self.audible.load(Ordering::Relaxed)
    }

    /// Make the output heard or not; the engine fades over
    /// [`MONITOR_FADE_SECONDS`] rather than clicking.
    pub fn set_audible(&self, audible: bool) {
        self.audible.store(audible, Ordering::Relaxed);
    }

    /// The loudest left and right master samples since the last call.
    #[must_use]
    pub fn take_peaks(&self) -> (f32, f32) {
        (
            f32::from_bits(self.peak_left.swap(0, Ordering::Relaxed)),
            f32::from_bits(self.peak_right.swap(0, Ordering::Relaxed)),
        )
    }

    /// Module slots the engine has room for now.
    #[must_use]
    pub fn slots(&self) -> usize {
        self.slots.load(Ordering::Acquire)
    }

    /// Delay lines (one per cable) the engine holds now.
    #[must_use]
    pub fn lines(&self) -> usize {
        self.lines.load(Ordering::Acquire)
    }

    /// How much of the time it has the engine spends rendering, averaged
    /// over about [`LOAD_SECONDS`]: 0 when idle, 1 when rendering takes as
    /// long as the audio lasts (and the device would run dry).
    #[must_use]
    pub fn render_load(&self) -> f32 {
        f32::from_bits(self.load.load(Ordering::Relaxed))
    }

    /// Set the render load, as a test would find it.
    #[cfg(test)]
    pub(crate) fn set_render_load(&self, load: f32) {
        self.load.store(load.to_bits(), Ordering::Relaxed);
    }

    /// Set the frames rendered, as a test would find them.
    #[cfg(test)]
    pub(crate) fn set_frames(&self, frames: u64) {
        self.frames.store(frames, Ordering::Release);
    }

    /// Frames rendered. Each sub-block stores it after its master went to
    /// the listen and record rings, so a reader that sees it move on sees
    /// what those sub-blocks pushed.
    #[must_use]
    pub fn frames(&self) -> u64 {
        self.frames.load(Ordering::SeqCst)
    }

    /// Faults: every one, and those that could not be reported in detail.
    #[must_use]
    pub fn faults(&self) -> (u64, u64) {
        (
            self.faults.load(Ordering::Relaxed),
            self.faults_unreported.load(Ordering::Relaxed),
        )
    }

    /// Retired items leaked rather than freed on the audio thread because
    /// the retire ring was full.
    #[must_use]
    pub fn leaked(&self) -> u64 {
        self.leaked.load(Ordering::Relaxed)
    }

    /// Listen samples dropped because the analysis fell behind.
    #[must_use]
    pub fn listen_dropped(&self) -> u64 {
        self.listen_dropped.load(Ordering::Relaxed)
    }

    /// Captures from scopes and readouts dropped because the probe ring
    /// was full.
    #[must_use]
    pub fn probes_dropped(&self) -> u64 {
        self.probes_dropped.load(Ordering::Relaxed)
    }

    /// Whether the master goes to the record ring.
    #[must_use]
    pub fn recording(&self) -> bool {
        self.recording.load(Ordering::SeqCst)
    }

    /// Start or stop sending the master to the record ring. Once it is
    /// cleared, the sub-block under way may still push; any sub-block that
    /// starts after [`Self::frames`] has moved on from where it read when
    /// this was cleared pushes nothing.
    pub fn set_recording(&self, recording: bool) {
        self.recording.store(recording, Ordering::SeqCst);
    }

    /// Record samples dropped because the recording's writer fell behind
    /// (whole sub-blocks: the ring never holds half a frame).
    #[must_use]
    pub fn record_dropped(&self) -> u64 {
        self.record_dropped.load(Ordering::Relaxed)
    }

    /// Desk transport changes that could not be followed.
    #[must_use]
    pub fn syncs_lost(&self) -> u64 {
        self.syncs_lost.load(Ordering::Relaxed)
    }

    /// Commands that named a slot or knob out of range (a slot there was
    /// no memory to make room for), or brought a room or line table no
    /// larger than the one in place.
    #[must_use]
    pub fn ignored(&self) -> u64 {
        self.ignored.load(Ordering::Relaxed)
    }

    /// Swaps finished: [`Command::Unhush`]es taken in, so every command
    /// sent before each has been too. Until a swap is in, the knobs read
    /// here are the old wall's.
    #[must_use]
    pub fn unhushed(&self) -> u64 {
        self.unhushed.load(Ordering::Acquire)
    }
}

/// How an engine starts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EngineConfig {
    /// Frames per second.
    pub sample_rate: u32,
    /// Starting tempo.
    pub bpm: f64,
    /// Starting song position, so a rebuilt engine carries on the beat.
    pub beat: f64,
    /// Mono samples the listen ring holds.
    pub listen_frames: usize,
    /// Stereo frames the record ring holds.
    pub record_frames: usize,
    /// Whether the output is heard from the first frame (see
    /// [`Shared::set_audible`]).
    pub audible: bool,
}

impl EngineConfig {
    /// An engine at `sample_rate`, `bpm` and `beat`, with a second of
    /// listening and [`RECORD_SECONDS`] of recording, heard from its first
    /// frame.
    #[must_use]
    pub fn new(sample_rate: u32, bpm: f64, beat: f64) -> Self {
        Self {
            sample_rate,
            bpm,
            beat,
            listen_frames: sample_rate.max(1) as usize,
            record_frames: sample_rate.max(1) as usize * RECORD_SECONDS,
            audible: true,
        }
    }
}

/// How long the output takes to fade in or out when it is made heard or
/// silent.
pub const MONITOR_FADE_SECONDS: f32 = 0.05;

/// How long the master takes to fade out, and back in, around a whole wall
/// being swapped for another ([`Command::Hush`]): long enough that neither
/// edge clicks, short enough to read as a cut in the music.
pub const SWAP_FADE_SECONDS: f32 = 0.06;

/// The shortest a knob takes to move, even when told to move at once: a
/// jump in a level or a cutoff would click.
const MIN_GLIDE_SECONDS: f32 = 0.001;

/// A knob moving towards its target.
///
/// It moves in knob position (0 to 1 along its travel, see
/// [`KnobSpec::normalise`]), frame by frame: the modules read where it is at
/// each frame through [`Io::knob_at`], and a glide on a logarithmic knob
/// such as a cutoff moves by even musical steps rather than spending most
/// of its time in the top octaves.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Glide {
    value: f32,
    position: f32,
    target: f32,
    step: f32,
    remaining: u32,
}

impl Glide {
    const fn at(value: f32, position: f32) -> Self {
        Self {
            value,
            position,
            target: value,
            step: 0.0,
            remaining: 0,
        }
    }

    /// A knob resting on `value`, placed along `spec`'s travel.
    fn resting(value: f32, spec: Option<&KnobSpec>) -> Self {
        Self::at(value, spec.map_or(0.0, |spec| spec.normalise(value)))
    }

    /// Move one sub-block along, returning how the knob moves through it.
    fn advance(&mut self, spec: Option<&KnobSpec>) -> Sweep {
        let Some(spec) = spec.filter(|_| self.remaining > 0) else {
            return Sweep::REST;
        };
        // At most SUB_BLOCK (32): exact in f32.
        let frames = self.remaining.min(SUB_BLOCK as u32);
        let sweep = Sweep {
            from: self.position,
            step: self.step,
            frames,
        };
        self.remaining -= frames;
        if self.remaining == 0 {
            self.value = self.target;
            self.position = spec.normalise(self.target);
        } else {
            self.position = self.step.mul_add(frames as f32, self.position);
            self.value = spec.denormalise(self.position);
        }
        sweep
    }
}

/// The audio-callback half of the engine.
pub struct Engine {
    sample_rate: f32,
    /// Every slot's module, knobs and outputs.
    room: Room,
    /// How the module being rendered has its knobs move this sub-block.
    sweeps: [Sweep; MAX_KNOBS],
    /// [`MIN_GLIDE_SECONDS`] in frames.
    min_glide: u32,
    inputs: [Block; MAX_INPUTS],
    knob_cv: [Block; MAX_KNOBS],
    filled: u32,
    cables: Box<CableTable>,
    order: Box<Order>,
    commands: HeapCons<Command>,
    /// Cable tables taken in (see [`ProbeFrame::cables`]).
    tables: u64,
    retired: HeapProd<Retired>,
    faults: HeapProd<Fault>,
    listen: HeapProd<f32>,
    /// The master, stereo interleaved, while [`Shared::recording`].
    record: HeapProd<f32>,
    /// What scopes and readouts measured.
    probes: HeapProd<ProbeFrame>,
    shared: Arc<Shared>,
    hub: Option<HubLinkAudio>,
    follower: TransportFollower,
    bpm: f64,
    beat: f64,
    following_desk: bool,
    rendered: u64,
    master: master::Master,
    lines: lines::Lines,
    carry: [Block; 2],
    carry_used: usize,
    stereo: Box<[f32]>,
    /// Gain on what leaves the wall, fading towards 1 while it is heard and
    /// 0 while it is not; `None` until the first render, which starts it
    /// where it should be rather than fading from somewhere else.
    monitor_gain: Option<f32>,
    /// How far `monitor_gain` moves per frame.
    monitor_step: f32,
    /// Gain on the master around a swap of the whole wall: 1 but for the
    /// fade out and in that [`Command::Hush`] and [`Command::Unhush`] ask
    /// for.
    hush_gain: f32,
    /// Whether the master is fading out for a swap, or silent for it.
    hushed: bool,
    /// How far `hush_gain` moves per frame.
    hush_step: f32,
    /// The render load, averaged (see [`Shared::render_load`]).
    load: f32,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("sample_rate", &self.sample_rate)
            .field("bpm", &self.bpm)
            .field("beat", &self.beat)
            .field("following_desk", &self.following_desk)
            .field("rendered", &self.rendered)
            .field("slots", &self.room.len())
            .field("lines", &self.lines.len())
            .finish_non_exhaustive()
    }
}

/// The control-side half of the engine.
pub struct EngineControl {
    sample_rate: u32,
    commands: HeapProd<Command>,
    backlog: VecDeque<Command>,
    retired: HeapCons<Retired>,
    faults: HeapCons<Fault>,
    listen: Option<HeapCons<f32>>,
    record: Option<HeapCons<f32>>,
    probes: Option<HeapCons<ProbeFrame>>,
    shared: Arc<Shared>,
    /// Slots in the largest room sent to the engine (or its first).
    slots: usize,
    /// Lines in the largest line table sent to the engine (or its first).
    lines: usize,
    /// The knob cells of every page sent to the engine, by page number.
    /// A page's cells never move, so they read the same before and after
    /// the engine puts the page in place (NaN until it publishes to them).
    knobs: Vec<Arc<KnobCells>>,
}

impl std::fmt::Debug for EngineControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineControl")
            .field("sample_rate", &self.sample_rate)
            .field("slots", &self.slots)
            .field("lines", &self.lines)
            .field("backlog", &self.backlog.len())
            .field("listen_taken", &self.listen.is_none())
            .field("record_taken", &self.record.is_none())
            .field("probes_taken", &self.probes.is_none())
            .finish_non_exhaustive()
    }
}

/// Build an engine and its control half. `hub` is the desk link's audio
/// half, when the wall is joining the desk.
#[must_use]
pub fn engine(config: EngineConfig, hub: Option<HubLinkAudio>) -> (Engine, EngineControl) {
    let bpm = clamp_bpm(config.bpm);
    let beat = if config.beat.is_finite() {
        config.beat
    } else {
        0.0
    };
    let sample_rate = config.sample_rate.max(8_000);
    let (command_tx, command_rx) = HeapRb::<Command>::new(COMMAND_BACKLOG).split();
    let (retired_tx, retired_rx) = HeapRb::<Retired>::new(RETIRE_BACKLOG).split();
    let (fault_tx, fault_rx) = HeapRb::<Fault>::new(FAULT_BACKLOG).split();
    let (listen_tx, listen_rx) = HeapRb::<f32>::new(config.listen_frames.max(SUB_BLOCK)).split();
    // Whole sub-blocks of stereo go in at a time, so the ring holds at
    // least one.
    let (record_tx, record_rx) =
        HeapRb::<f32>::new(config.record_frames.max(SUB_BLOCK).saturating_mul(2)).split();
    let (probe_tx, probe_rx) = HeapRb::<ProbeFrame>::new(PROBE_BACKLOG).split();
    // A machine with no memory for a modest wall starts with none, and
    // grows as the wall asks.
    let room = Room::new(0, INITIAL_SLOTS).unwrap_or_default();
    let knobs = room
        .published()
        .into_iter()
        .map(|(_, cells)| cells)
        .collect();
    // 8 kHz to a few hundred kHz: exact in f32.
    let rate = sample_rate as f32;
    let lines = lines::Lines::new(rate, INITIAL_LINES);
    let shared = Arc::new(Shared::new(
        bpm,
        beat,
        config.audible,
        room.len(),
        lines.len(),
    ));
    let engine = Engine {
        sample_rate: rate,
        sweeps: [Sweep::REST; MAX_KNOBS],
        // A few hundred frames at most: exact.
        min_glide: ((rate * MIN_GLIDE_SECONDS).round() as u32).max(1),
        inputs: [[0.0; SUB_BLOCK]; MAX_INPUTS],
        knob_cv: [[0.0; SUB_BLOCK]; MAX_KNOBS],
        filled: 0,
        cables: Box::new(CableTable::new()),
        order: Box::new(Order::default()),
        commands: command_rx,
        tables: 0,
        retired: retired_tx,
        faults: fault_tx,
        listen: listen_tx,
        record: record_tx,
        probes: probe_tx,
        shared: Arc::clone(&shared),
        hub,
        follower: TransportFollower::new(sample_rate),
        bpm,
        beat,
        following_desk: false,
        rendered: 0,
        master: master::Master::new(rate),
        carry: [[0.0; SUB_BLOCK]; 2],
        carry_used: SUB_BLOCK,
        stereo: vec![0.0; MAX_CHUNK_FRAMES * 2].into_boxed_slice(),
        monitor_gain: None,
        monitor_step: 1.0 / (rate * MONITOR_FADE_SECONDS),
        hush_gain: 1.0,
        hushed: false,
        hush_step: 1.0 / (rate * SWAP_FADE_SECONDS),
        load: 0.0,
        room,
        lines,
    };
    let control = EngineControl {
        sample_rate,
        commands: command_tx,
        backlog: VecDeque::new(),
        retired: retired_rx,
        faults: fault_rx,
        listen: Some(listen_rx),
        record: Some(record_rx),
        probes: Some(probe_rx),
        slots: shared.slots(),
        lines: shared.lines(),
        shared,
        knobs,
    };
    (engine, control)
}

/// `len` items made by `make`, or `None` when there is no memory for them.
/// It allocates: control side only.
fn slice_of<T>(len: usize, make: impl FnMut() -> T) -> Option<Box<[T]>> {
    let mut items = Vec::new();
    if items.try_reserve_exact(len).is_err() {
        return None;
    }
    items.resize_with(len, make);
    Some(items.into_boxed_slice())
}

/// How large to grow to from `have` when `need` is asked for: twice as
/// large, so growth is amortised, or `need` if that is more.
const fn doubled(have: usize, need: usize) -> usize {
    let twice = have.saturating_mul(2);
    if twice > need { twice } else { need }
}

/// The CPU time the calling thread has used.
fn thread_cpu() -> Duration {
    let time = rustix::time::clock_gettime(rustix::time::ClockId::ThreadCPUTime);
    Duration::new(
        u64::try_from(time.tv_sec).unwrap_or(0),
        u32::try_from(time.tv_nsec).unwrap_or(0),
    )
}

/// A tempo held to the wall's range; not a number gives 120.
#[must_use]
pub const fn clamp_bpm(bpm: f64) -> f64 {
    if bpm.is_finite() {
        bpm.clamp(MIN_BPM, MAX_BPM)
    } else {
        120.0
    }
}

impl Engine {
    /// Render interleaved audio for a device with `channels` channels: the
    /// left and right master go to the first two (both averaged on a mono
    /// device), and any others get silence. While the desk is playing the
    /// wall, the whole buffer is silence. While the wall is not heard
    /// ([`Shared::set_audible`]), what leaves it, to the device and to the
    /// desk alike, fades to silence; the meters and the listening still
    /// hear it.
    pub fn render(&mut self, data: &mut [f32], channels: usize) {
        // The load is the CPU time this thread spends rendering, not the
        // time that passes: on a busy machine the scheduler sets the audio
        // thread aside now and then, and that waiting is the machine's
        // load, not the wall's. Reading the thread's CPU clock allocates
        // nothing and takes no lock.
        let started = thread_cpu();
        self.drain_commands();
        self.drain_hub();
        let channels = channels.max(1);
        self.render_frames(data, channels);
        self.measure(started, data.len() / channels);
    }

    /// Fold how long rendering `frames` frames took, since `started`, into
    /// the render load.
    fn measure(&mut self, started: Duration, frames: usize) {
        if frames == 0 {
            return;
        }
        // Blocks are at most a few seconds: close enough in f32.
        let lasted = frames as f32 / self.sample_rate;
        let load = thread_cpu().saturating_sub(started).as_secs_f32() / lasted;
        // An average over about LOAD_SECONDS, whatever the block size.
        let weight = 1.0 - (-lasted / LOAD_SECONDS).exp();
        let averaged = weight.mul_add(load - self.load, self.load);
        if averaged.is_finite() {
            self.load = averaged;
            self.shared
                .load
                .store(averaged.to_bits(), Ordering::Relaxed);
        }
    }

    /// Render `data` (see [`Self::render`]) once the commands are in.
    fn render_frames(&mut self, data: &mut [f32], channels: usize) {
        for chunk in data.chunks_mut(MAX_CHUNK_FRAMES * channels) {
            let frames = chunk.len() / channels;
            for frame in 0..frames {
                if self.carry_used == SUB_BLOCK {
                    self.render_sub_block();
                    self.carry_used = 0;
                }
                self.stereo[frame * 2] = self.carry[0][self.carry_used];
                self.stereo[frame * 2 + 1] = self.carry[1][self.carry_used];
                self.carry_used += 1;
            }
            self.apply_monitor(frames);
            // At most MAX_CHUNK_FRAMES: the cast is lossless.
            let to_desk = self
                .hub
                .as_mut()
                .is_some_and(|hub| hub.send_audio(frames as u32, &self.stereo[..frames * 2]));
            self.shared.to_desk.store(to_desk, Ordering::Relaxed);
            for (frame, out) in chunk.chunks_mut(channels).enumerate() {
                if to_desk || frame >= frames {
                    out.fill(0.0);
                    continue;
                }
                let left = self.stereo[frame * 2];
                let right = self.stereo[frame * 2 + 1];
                if let [mono] = out {
                    *mono = 0.5 * (left + right);
                } else {
                    for (channel, sample) in out.iter_mut().enumerate() {
                        *sample = match channel {
                            0 => left,
                            1 => right,
                            _ => 0.0,
                        };
                    }
                }
            }
        }
    }

    /// Frames owed to a desk that paces the wall at `now` (see
    /// [`HubLinkAudio::desk_owes`]); `None` when no desk is pacing it.
    #[must_use]
    pub fn desk_owes(&self, now: std::time::Instant) -> Option<kazoo_core::ipc::link::DeskOwes> {
        self.hub.as_ref().and_then(|hub| hub.desk_owes(now))
    }

    /// Fade the first `frames` of `stereo` by the monitor gain, moving it
    /// towards whether the wall is heard.
    fn apply_monitor(&mut self, frames: usize) {
        let target = if self.shared.audible() { 1.0 } else { 0.0 };
        let mut gain = *self.monitor_gain.get_or_insert(target);
        let step = self.monitor_step;
        for pair in self.stereo[..frames * 2].chunks_exact_mut(2) {
            gain = if gain < target {
                (gain + step).min(target)
            } else {
                (gain - step).max(target)
            };
            pair[0] *= gain;
            pair[1] *= gain;
        }
        self.monitor_gain = Some(gain);
    }

    /// Whether commands are held: a hush is fading the master out and it is
    /// not silent yet.
    fn holding(&self) -> bool {
        self.hushed && self.hush_gain > 0.0
    }

    fn drain_commands(&mut self) {
        while !self.holding() {
            let Some(command) = self.commands.try_pop() else {
                return;
            };
            match command {
                Command::Insert {
                    slot,
                    tag,
                    kind,
                    module,
                    knobs,
                } => self.insert(slot, tag, kind, module, knobs),
                Command::Remove { slot } => self.remove(slot),
                Command::Cables(table) => {
                    let old = std::mem::replace(&mut self.cables, table);
                    self.rewire(&old);
                    self.tables = self.tables.wrapping_add(1);
                    self.retire(Retired::Cables(old));
                }
                Command::Order(order) => {
                    let old = std::mem::replace(&mut self.order, order);
                    self.retire(Retired::Order(old));
                }
                Command::Room(room) => self.grow_room(room),
                Command::Lines(mut more) => {
                    if self.lines.grow(&mut more) {
                        self.shared.lines.store(self.lines.len(), Ordering::Release);
                    } else {
                        self.shared.ignored.fetch_add(1, Ordering::Relaxed);
                    }
                    self.retire(Retired::Lines(more));
                }
                Command::Knob {
                    slot,
                    knob,
                    target,
                    glide_frames,
                } => self.knob(slot, knob, target, glide_frames),
                Command::Tempo(bpm) => {
                    self.bpm = clamp_bpm(bpm);
                    self.shared.bpm.store(self.bpm.to_bits(), Ordering::Relaxed);
                }
                Command::Hush => self.hushed = true,
                Command::Unhush => {
                    self.hushed = false;
                    self.shared.unhushed.fetch_add(1, Ordering::Release);
                }
            }
        }
    }

    /// Fade the master by the hush gain, moving it towards silence while a
    /// swap is hushed and back to 1 after.
    fn apply_hush(&mut self, left: &mut Block, right: &mut Block) {
        if !self.hushed && self.hush_gain >= 1.0 {
            return;
        }
        let step = self.hush_step;
        let mut gain = self.hush_gain;
        for (left, right) in left.iter_mut().zip(right.iter_mut()) {
            gain = if self.hushed {
                (gain - step).max(0.0)
            } else {
                (gain + step).min(1.0)
            };
            *left *= gain;
            *right *= gain;
        }
        self.hush_gain = gain;
    }

    /// Tell every module whose inputs come from elsewhere under the new
    /// cable table than under `old` (another source or port, another
    /// amount, or a cable plugged or unplugged), and which.
    fn rewire(&mut self, old: &CableTable) {
        let source = |route: Option<Route>| {
            route.map(|route| (route.slot, route.port, route.amount.to_bits()))
        };
        // A slot past both tables has nothing plugged in under either.
        for slot in 0..old.slots().max(self.cables.slots()) {
            let moved: [bool; MAX_INPUTS] = std::array::from_fn(|input| {
                source(old.route(slot, input)) != source(self.cables.route(slot, input))
            });
            if moved.contains(&true) {
                let module = self
                    .room
                    .page_mut(slot)
                    .and_then(|(page, at)| page.modules[at].as_mut());
                if let Some(module) = module {
                    module.rewired(moved);
                }
            }
        }
    }

    fn insert(
        &mut self,
        slot: usize,
        tag: u32,
        kind: Kind,
        module: Box<dyn Module>,
        knobs: [f32; MAX_KNOBS],
    ) {
        // Every slice in a page is SLOTS_PER_PAGE long, so a slot's place
        // in its page is in range in each.
        let Some((page, at)) = self.room.page_mut(slot) else {
            // The control side makes room first; there was no memory for
            // it.
            self.shared.ignored.fetch_add(1, Ordering::Relaxed);
            self.retire(Retired::Module(module));
            return;
        };
        let old = page.modules[at].replace(module);
        page.tags[at] = tag;
        page.kinds[at] = Some(kind);
        page.outputs[at] = [[0.0; SUB_BLOCK]; MAX_OUTPUTS];
        page.gliding[at] = 0;
        for (index, value) in knobs.iter().enumerate() {
            let value = kazoo_core::sanitize_sample(*value);
            page.glides[at][index] = Glide::resting(value, kind.spec().knobs.get(index));
            page.knobs[at][index] = value;
            page.publish(at, index, value);
        }
        if let Some(old) = old {
            self.retire(Retired::Module(old));
        }
    }

    fn remove(&mut self, slot: usize) {
        let Some((page, at)) = self.room.page_mut(slot) else {
            self.shared.ignored.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let old = page.modules[at].take();
        page.kinds[at] = None;
        page.outputs[at] = [[0.0; SUB_BLOCK]; MAX_OUTPUTS];
        if let Some(old) = old {
            self.retire(Retired::Module(old));
        }
    }

    /// Move into a larger `room`, and retire the one outgrown (or `room`,
    /// if it is no larger).
    fn grow_room(&mut self, mut room: Box<Room>) {
        if room.len() > self.room.len() {
            room.take_from(&mut self.room);
            std::mem::swap(&mut self.room, &mut *room);
            self.shared.slots.store(self.room.len(), Ordering::Release);
        } else {
            self.shared.ignored.fetch_add(1, Ordering::Relaxed);
        }
        self.retire(Retired::Room(room));
    }

    fn knob(&mut self, slot: usize, knob: usize, target: f32, glide_frames: u32) {
        let min_glide = self.min_glide;
        let Some((page, at)) = self
            .room
            .page_mut(slot)
            .filter(|_| knob < MAX_KNOBS && target.is_finite())
        else {
            self.shared.ignored.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let spec = page.kinds[at].and_then(|kind| kind.spec().knobs.get(knob));
        let glide = &mut page.glides[at][knob];
        if let Some(spec) = spec {
            let frames = glide_frames.max(min_glide);
            // Frame counts are far below 2^24 per sub-step: close enough in
            // f32, and the last step lands exactly on the target.
            glide.step = (spec.normalise(target) - glide.position) / frames as f32;
            glide.target = target;
            glide.remaining = frames;
        } else {
            // A knob the kind does not have (or an empty slot): nothing
            // reads it, so it just takes the value.
            *glide = Glide::at(target, 0.0);
        }
        let value = glide.value;
        page.gliding[at] =
            (page.gliding[at] & !(1 << knob)) | (u32::from(glide.remaining > 0) << knob);
        page.knobs[at][knob] = value;
        page.publish(at, knob, value);
    }

    fn retire(&mut self, item: Retired) {
        if let Err(item) = self.retired.try_push(item) {
            // Never expected: the ring holds more than the command ring can
            // retire. Leaked rather than freed on the audio thread, and
            // counted.
            self.shared.leaked.fetch_add(1, Ordering::Relaxed);
            std::mem::forget(item);
        }
    }

    fn drain_hub(&mut self) {
        let Some(hub) = self.hub.as_mut() else {
            return;
        };
        for message in kazoo_core::host::link::take_events(&mut hub.events) {
            match message {
                HubMessage::TransportSync(sync) => {
                    if self.follower.schedule(&sync).is_err() {
                        self.shared.syncs_lost.fetch_add(1, Ordering::Relaxed);
                    }
                }
                // The wall takes no notes or parameters from the desk, and
                // the link handles the desk closing.
                HubMessage::NoteEvent(_)
                | HubMessage::ParameterChange(_)
                | HubMessage::StreamReady { .. }
                | HubMessage::Shutdown => {}
            }
        }
    }

    /// Follow the desk: a playing desk sets the tempo and the beat; a
    /// stopped or missing desk leaves the wall on its own beat.
    fn follow_desk(&mut self) {
        let connected = self.hub.as_ref().is_some_and(HubLinkAudio::is_connected);
        let change = if self.hub.is_some() {
            self.follower.due(self.rendered)
        } else {
            None
        };
        let (bpm, beat, following) =
            follow(self.bpm, self.beat, self.following_desk, change, connected);
        self.bpm = bpm;
        self.beat = beat;
        self.following_desk = following;
    }

    fn render_sub_block(&mut self) {
        if self.hushed {
            // The commands a hush held go as soon as it is silent, not at
            // the next callback.
            self.drain_commands();
        }
        self.follow_desk();
        let tick = Tick::new(self.sample_rate, self.bpm, self.beat).at_frame(self.rendered);
        for index in 0..self.order.slots().len() {
            // u32 into usize: lossless.
            let slot = self.order.slots()[index] as usize;
            self.render_module(slot, &tick);
        }
        let mut left = [0.0; SUB_BLOCK];
        let mut right = [0.0; SUB_BLOCK];
        for &slot in self.order.outs() {
            if let Some((page, at)) = self.room.page(slot as usize) {
                if page.kinds[at] == Some(Kind::OUT) && page.modules[at].is_some() {
                    let outputs = &page.outputs[at];
                    for frame in 0..SUB_BLOCK {
                        left[frame] += outputs[crate::dsp::FEED_LEFT][frame];
                        right[frame] += outputs[crate::dsp::FEED_RIGHT][frame];
                    }
                }
            }
        }
        self.master.process(&mut left, &mut right);
        // After the master, so the fade is what the meters, the listening
        // and a recording hear too.
        self.apply_hush(&mut left, &mut right);
        self.publish(&left, &right);
        self.carry = [left, right];
        self.beat = tick.beat_at(SUB_BLOCK);
        // SUB_BLOCK is 32: lossless.
        self.rendered = self.rendered.wrapping_add(SUB_BLOCK as u64);
        self.shared
            .beat
            .store(self.beat.to_bits(), Ordering::Relaxed);
        self.shared.bpm.store(self.bpm.to_bits(), Ordering::Relaxed);
        self.shared
            .following_desk
            .store(self.following_desk, Ordering::Relaxed);
        self.shared.frames.store(self.rendered, Ordering::SeqCst);
    }

    fn render_module(&mut self, slot: usize, tick: &Tick) {
        // An order entry past the room (never sent: the room grows first)
        // or naming an empty slot is skipped. Every slice in a page is
        // SLOTS_PER_PAGE long, so a slot's place in its page is in range.
        let Some((page, at)) = self.room.page_mut(slot) else {
            return;
        };
        let Some(kind) = page.kinds[at].filter(|_| page.modules[at].is_some()) else {
            return;
        };
        let gliding = page.advance_glides(at, kind, &mut self.sweeps);
        let mask = self.cables.plugged(slot);
        tables::clear_jacks(self.filled & !mask, &mut self.inputs, &mut self.knob_cv);
        self.filled = mask;
        let mut pending = mask;
        while pending != 0 {
            let jack = pending.trailing_zeros() as usize;
            let buffer = if jack < MAX_INPUTS {
                &mut self.inputs[jack]
            } else {
                &mut self.knob_cv[jack - MAX_INPUTS]
            };
            gather(
                &self.cables,
                &self.room,
                &mut self.lines,
                slot,
                jack,
                buffer,
            );
            pending &= pending - 1;
        }
        let connected = std::array::from_fn(|input| mask & (1 << input) != 0);
        let knob_patched = std::array::from_fn(|knob| mask & (1 << (MAX_INPUTS + knob)) != 0);
        let fed = std::array::from_fn(|port| self.cables.feeds(slot, port));
        let Some((page, at)) = self.room.page_mut(slot) else {
            return;
        };
        let (Some(module), knobs, outputs, tag) = (
            page.modules[at].as_mut(),
            &page.knobs[at],
            &mut page.outputs[at],
            page.tags[at],
        ) else {
            return;
        };
        module.process(
            tick,
            Io {
                spec: kind.spec(),
                knobs,
                sweeps: &self.sweeps,
                knob_cv: &self.knob_cv,
                knob_patched,
                inputs: &self.inputs,
                connected,
                fed,
                outputs: &mut *outputs,
            },
        );
        room::rest_sweeps(gliding, &mut self.sweeps);
        let healthy = outputs.iter().enumerate().all(|(port, samples)| {
            !(fed[port]
                || kind == Kind::OUT
                    && (port == crate::dsp::FEED_LEFT || port == crate::dsp::FEED_RIGHT))
                || samples.iter().all(|sample| sample.is_finite())
        });
        if !healthy {
            *outputs = [[0.0; SUB_BLOCK]; MAX_OUTPUTS];
            module.reset();
            self.shared.faults.fetch_add(1, Ordering::Relaxed);
            let fault = Fault { slot, tag };
            if self.faults.try_push(fault).is_err() {
                self.shared
                    .faults_unreported
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        if let Some(capture) = module.capture() {
            let frame = ProbeFrame {
                slot,
                tag,
                cables: self.tables,
                capture: *capture,
            };
            if self.probes.try_push(frame).is_err() {
                self.shared.probes_dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn publish(&mut self, left: &Block, right: &Block) {
        let mut peak_left = 0.0_f32;
        let mut peak_right = 0.0_f32;
        let mut mono = [0.0; SUB_BLOCK];
        for frame in 0..SUB_BLOCK {
            peak_left = peak_left.max(left[frame].abs());
            peak_right = peak_right.max(right[frame].abs());
            mono[frame] = 0.5 * (left[frame] + right[frame]);
        }
        // Non-negative finite floats order the same as their bit patterns.
        self.shared
            .peak_left
            .fetch_max(peak_left.to_bits(), Ordering::Relaxed);
        self.shared
            .peak_right
            .fetch_max(peak_right.to_bits(), Ordering::Relaxed);
        let pushed = self.listen.push_slice(&mono);
        if pushed < SUB_BLOCK {
            // SUB_BLOCK is 32: lossless.
            self.shared
                .listen_dropped
                .fetch_add((SUB_BLOCK - pushed) as u64, Ordering::Relaxed);
        }
        if self.shared.recording() {
            self.push_record(left, right);
        }
    }

    /// Push the sub-block's master to the record ring, stereo interleaved.
    /// It goes whole or not at all, so the ring never holds half a frame
    /// and the channels never swap.
    fn push_record(&mut self, left: &Block, right: &Block) {
        let mut frames = [0.0; SUB_BLOCK * 2];
        for (frame, pair) in frames.chunks_exact_mut(2).enumerate() {
            pair[0] = left[frame];
            pair[1] = right[frame];
        }
        if self.record.vacant_len() < frames.len() {
            // SUB_BLOCK * 2 is 64: lossless.
            self.shared
                .record_dropped
                .fetch_add(frames.len() as u64, Ordering::Relaxed);
            return;
        }
        self.record.push_slice(&frames);
    }
}

/// The wall's clock after a desk transport `change` (if one fell due), given
/// whether the desk is still there: `(bpm, beat, following_desk)`.
///
/// A playing desk sets the tempo and the song position. A stopped desk sets
/// the tempo and leaves the wall on its own beat, carrying on from where it
/// was; so does a desk that has gone away.
#[must_use]
pub const fn follow(
    bpm: f64,
    beat: f64,
    following: bool,
    change: Option<TransportChange>,
    connected: bool,
) -> (f64, f64, bool) {
    let (bpm, beat, following) = match change {
        Some(TransportChange {
            bpm,
            beat: Some(beat),
        }) if beat.is_finite() => (clamp_bpm(bpm), beat, true),
        Some(TransportChange { bpm, .. }) => (clamp_bpm(bpm), beat, false),
        None => (bpm, beat, following),
    };
    (bpm, beat, following && connected)
}

/// Fill `buffer` with what arrives at `slot`'s `jack`: the cable's source,
/// through its delay line, scaled by its amount; silence when unplugged.
/// Returns whether a cable is in.
fn gather(
    cables: &CableTable,
    room: &Room,
    lines: &mut lines::Lines,
    slot: usize,
    jack: usize,
    buffer: &mut Block,
) -> bool {
    let source = cables.route(slot, jack).and_then(|route| {
        // u32 into usize: lossless.
        room.output(route.slot as usize, usize::from(route.port))
            .map(|source| (source, route))
    });
    if let Some((source, route)) = source {
        lines.carry(&route, source, buffer);
        true
    } else {
        *buffer = [0.0; SUB_BLOCK];
        false
    }
}

impl EngineControl {
    /// Frames per second the engine renders at.
    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// What the engine publishes.
    #[must_use]
    pub const fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    /// Module slots the engine has room for, or will have once the rooms
    /// sent so far are in place.
    #[must_use]
    pub const fn slots(&self) -> usize {
        self.slots
    }

    /// Delay lines the engine holds, or will once the tables sent so far
    /// are in place.
    #[must_use]
    pub const fn lines(&self) -> usize {
        self.lines
    }

    /// Make sure the engine will have room for `slots` module slots, by
    /// sending it a larger room when it needs one: twice its size, or
    /// `slots` if that is more (or just `slots`, when there is no memory
    /// for twice). Returns whether it will have the room; `false` when
    /// there is no memory for it.
    pub fn reserve_slots(&mut self, slots: usize) -> bool {
        if slots <= self.slots {
            return true;
        }
        let have = self.slots;
        let Some(room) = Room::new(have, doubled(have, slots)).or_else(|| Room::new(have, slots))
        else {
            return false;
        };
        for (number, cells) in room.published() {
            if number >= self.knobs.len() {
                self.knobs.push(cells);
            }
        }
        self.slots = room.len();
        self.push(Command::Room(Box::new(room)));
        true
    }

    /// Make sure the engine will have `lines` delay lines, by sending it a
    /// larger table when it needs one: twice its size, or `lines` if that
    /// is more (or just `lines`, when there is no memory for twice).
    /// Returns whether it will have them; `false` when there is no memory
    /// for them.
    pub fn reserve_lines(&mut self, lines: usize) -> bool {
        if lines <= self.lines {
            return true;
        }
        let have = self.lines;
        let Some(more) =
            MoreLines::new(have, doubled(have, lines)).or_else(|| MoreLines::new(have, lines))
        else {
            return false;
        };
        self.lines = more.len();
        self.push(Command::Lines(Box::new(more)));
        true
    }

    /// The current (glided) value of `slot`'s knob `knob`, as the engine
    /// last published it; `None` for a slot or knob out of range, or one
    /// the engine has published nothing to yet.
    #[must_use]
    pub fn knob(&self, slot: usize, knob: usize) -> Option<f32> {
        self.knobs
            .get(slot / room::SLOTS_PER_PAGE)
            .and_then(|cells| cells.get(slot % room::SLOTS_PER_PAGE))
            .and_then(|knobs| knobs.get(knob))
            .map(|value| f32::from_bits(value.load(Ordering::Relaxed)))
            .filter(|value| !value.is_nan())
    }

    /// Send a command, making room for it first: a module's slot, or a
    /// line for every cable in a table. With no memory for the room, the
    /// command goes anyway and the engine refuses it (counting it in
    /// [`Shared::ignored`]). If the ring is full it waits in order on this
    /// side and goes with the next [`Self::pump`].
    pub fn send(&mut self, command: Command) {
        let room = match &command {
            Command::Insert { slot, .. } => self.reserve_slots(slot.saturating_add(1)),
            Command::Cables(table) => self.reserve_lines(table.lines()),
            _ => true,
        };
        if !room {
            eprintln!("kazoo-wall: no memory to grow the engine; a change goes unplayed");
        }
        self.push(command);
    }

    /// Send a command as it is. If the ring is full it waits in order on
    /// this side and goes with the next [`Self::pump`].
    fn push(&mut self, command: Command) {
        self.flush();
        if !self.backlog.is_empty() {
            self.backlog.push_back(command);
            return;
        }
        if let Err(command) = self.commands.try_push(command) {
            self.backlog.push_back(command);
        }
    }

    /// Commands waiting on this side for room in the ring.
    #[must_use]
    pub fn backlog(&self) -> usize {
        self.backlog.len()
    }

    /// Move waiting commands into the ring, free what the engine retired,
    /// and return the faults it reported.
    pub fn pump(&mut self) -> Vec<Fault> {
        self.flush();
        let mut faults = Vec::new();
        while let Some(fault) = self.faults.try_pop() {
            faults.push(fault);
        }
        faults
    }

    /// Take the listen ring's reading end (once).
    pub const fn take_listen(&mut self) -> Option<HeapCons<f32>> {
        self.listen.take()
    }

    /// Take the record ring's reading end (once): the master, stereo
    /// interleaved, while [`Shared::recording`] is set.
    pub const fn take_record(&mut self) -> Option<HeapCons<f32>> {
        self.record.take()
    }

    /// Take the probe ring's reading end (once): what the scopes and
    /// readouts measured.
    pub const fn take_probes(&mut self) -> Option<HeapCons<ProbeFrame>> {
        self.probes.take()
    }

    /// Items waiting in the retire ring.
    #[must_use]
    pub fn retired_waiting(&self) -> usize {
        self.retired.occupied_len()
    }

    /// Free what the engine retired, and move waiting commands into the
    /// ring. Every send does this too, so a burst of changes between pumps
    /// keeps the retire ring from filling (the engine would have to leak
    /// what it cannot retire).
    ///
    /// What was retired is freed before each command goes in: then what
    /// waits in the retire ring was retired by commands that were in the
    /// command ring when it was last freed, at most [`COMMAND_BACKLOG`],
    /// and what the commands in the ring now will retire is at most as
    /// many again, so [`RETIRE_BACKLOG`] never fills, however long the
    /// backlog (a whole wall swapped out under a hush).
    fn flush(&mut self) {
        loop {
            while let Some(item) = self.retired.try_pop() {
                drop(item);
            }
            let Some(command) = self.backlog.pop_front() else {
                return;
            };
            if let Err(command) = self.commands.try_push(command) {
                self.backlog.push_front(command);
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests;
