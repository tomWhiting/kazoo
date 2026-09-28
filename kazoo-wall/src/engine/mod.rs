//! The realtime engine.
//!
//! [`engine`] returns two halves. The [`Engine`] moves into the audio
//! callback (or the headless render loop): it owns a fixed table of
//! [`MAX_MODULES`] module slots, the cable and order tables, pre-sized port
//! buffers, the master chain and the wall's clock, and renders in sub-blocks
//! of [`SUB_BLOCK`] frames. It never allocates, frees, locks or does I/O.
//!
//! The [`EngineControl`] stays on the control side, the only place that
//! allocates: it sends [`Command`]s over a lock-free ring, frees what the
//! engine hands back over the retire ring, and collects fault reports. The
//! engine publishes its clock, meters and current knob values through
//! [`Shared`] atomics.
//!
//! The engine tolerates any order of commands: a cable or order entry naming
//! an empty slot reads silence or is skipped, so the control side never needs
//! a transaction.

pub mod lines;
pub mod master;
pub mod tables;

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use kazoo_core::ipc::client::HubMessage;
use kazoo_core::ipc::follow::{TransportChange, TransportFollower};
use kazoo_core::ipc::link::HubLinkAudio;
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};

use crate::catalogue::{Kind, KnobSpec};
use crate::dsp::{Block, Io, Module, Sweep, Tick};
use crate::{MAX_INPUTS, MAX_KNOBS, MAX_MODULES, MAX_OUTPUTS, SUB_BLOCK};
pub use master::CEILING;
pub use tables::{CableTable, MAX_CABLE_DELAY, Order, Route, processing_order};

/// Commands waiting for the engine.
const COMMAND_BACKLOG: usize = 512;

/// Items waiting to be freed: every command retires at most one.
const RETIRE_BACKLOG: usize = COMMAND_BACKLOG * 2;

/// Fault reports waiting for the control side.
const FAULT_BACKLOG: usize = 64;

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
        /// Slot, below [`MAX_MODULES`].
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
        /// Slot, below [`MAX_MODULES`].
        slot: usize,
    },
    /// Swap in a new cable table; the old one is retired.
    Cables(Box<CableTable>),
    /// Swap in a new render order; the old one is retired.
    Order(Box<Order>),
    /// Glide a knob to `target` over `glide_frames` frames.
    Knob {
        /// Slot, below [`MAX_MODULES`].
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
    knobs: Box<[[AtomicU32; MAX_KNOBS]]>,
    frames: AtomicU64,
    faults: AtomicU64,
    faults_unreported: AtomicU64,
    leaked: AtomicU64,
    listen_dropped: AtomicU64,
    recording: AtomicBool,
    record_dropped: AtomicU64,
    syncs_lost: AtomicU64,
    ignored: AtomicU64,
}

impl Shared {
    fn new(bpm: f64, beat: f64, audible: bool) -> Self {
        Self {
            bpm: AtomicU64::new(bpm.to_bits()),
            beat: AtomicU64::new(beat.to_bits()),
            following_desk: AtomicBool::new(false),
            to_desk: AtomicBool::new(false),
            audible: AtomicBool::new(audible),
            peak_left: AtomicU32::new(0),
            peak_right: AtomicU32::new(0),
            knobs: (0..MAX_MODULES)
                .map(|_| std::array::from_fn(|_| AtomicU32::new(0)))
                .collect(),
            frames: AtomicU64::new(0),
            faults: AtomicU64::new(0),
            faults_unreported: AtomicU64::new(0),
            leaked: AtomicU64::new(0),
            listen_dropped: AtomicU64::new(0),
            recording: AtomicBool::new(false),
            record_dropped: AtomicU64::new(0),
            syncs_lost: AtomicU64::new(0),
            ignored: AtomicU64::new(0),
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

    /// The current (glided) value of `slot`'s knob `knob`.
    #[must_use]
    pub fn knob(&self, slot: usize, knob: usize) -> Option<f32> {
        self.knobs
            .get(slot)
            .and_then(|knobs| knobs.get(knob))
            .map(|value| f32::from_bits(value.load(Ordering::Relaxed)))
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

    /// Commands that named a slot or knob out of range.
    #[must_use]
    pub fn ignored(&self) -> u64 {
        self.ignored.load(Ordering::Relaxed)
    }

    fn store_knob(&self, slot: usize, knob: usize, value: f32) {
        if let Some(cell) = self.knobs.get(slot).and_then(|knobs| knobs.get(knob)) {
            cell.store(value.to_bits(), Ordering::Relaxed);
        }
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
    slots: [Option<Box<dyn Module>>; MAX_MODULES],
    tags: [u32; MAX_MODULES],
    kinds: [Option<Kind>; MAX_MODULES],
    /// Per slot, [`MAX_MODULES`] long.
    glides: Box<[[Glide; MAX_KNOBS]]>,
    /// How the module being rendered has its knobs move this sub-block.
    sweeps: [Sweep; MAX_KNOBS],
    /// [`MIN_GLIDE_SECONDS`] in frames.
    min_glide: u32,
    /// Per slot, [`MAX_MODULES`] long.
    knobs: Box<[[f32; MAX_KNOBS]]>,
    /// Per slot, [`MAX_MODULES`] long.
    outputs: Box<[[Block; MAX_OUTPUTS]]>,
    inputs: [Block; MAX_INPUTS],
    knob_cv: [Block; MAX_KNOBS],
    cables: Box<CableTable>,
    order: Box<Order>,
    commands: HeapCons<Command>,
    retired: HeapProd<Retired>,
    faults: HeapProd<Fault>,
    listen: HeapProd<f32>,
    /// The master, stereo interleaved, while [`Shared::recording`].
    record: HeapProd<f32>,
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
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("sample_rate", &self.sample_rate)
            .field("bpm", &self.bpm)
            .field("beat", &self.beat)
            .field("following_desk", &self.following_desk)
            .field("rendered", &self.rendered)
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
    shared: Arc<Shared>,
}

impl std::fmt::Debug for EngineControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineControl")
            .field("sample_rate", &self.sample_rate)
            .field("backlog", &self.backlog.len())
            .field("listen_taken", &self.listen.is_none())
            .field("record_taken", &self.record.is_none())
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
    let shared = Arc::new(Shared::new(bpm, beat, config.audible));
    // 8 kHz to a few hundred kHz: exact in f32.
    let rate = sample_rate as f32;
    let engine = Engine {
        sample_rate: rate,
        slots: std::array::from_fn(|_| None),
        tags: [0; MAX_MODULES],
        kinds: [None; MAX_MODULES],
        glides: vec![[Glide::at(0.0, 0.0); MAX_KNOBS]; MAX_MODULES].into_boxed_slice(),
        sweeps: [Sweep::REST; MAX_KNOBS],
        // A few hundred frames at most: exact.
        min_glide: ((rate * MIN_GLIDE_SECONDS).round() as u32).max(1),
        knobs: vec![[0.0; MAX_KNOBS]; MAX_MODULES].into_boxed_slice(),
        outputs: vec![[[0.0; SUB_BLOCK]; MAX_OUTPUTS]; MAX_MODULES].into_boxed_slice(),
        inputs: [[0.0; SUB_BLOCK]; MAX_INPUTS],
        knob_cv: [[0.0; SUB_BLOCK]; MAX_KNOBS],
        cables: Box::new(CableTable::new()),
        order: Box::new(Order::default()),
        commands: command_rx,
        retired: retired_tx,
        faults: fault_tx,
        listen: listen_tx,
        record: record_tx,
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
        lines: lines::Lines::new(rate),
        monitor_gain: None,
        monitor_step: 1.0 / (rate * MONITOR_FADE_SECONDS),
    };
    let control = EngineControl {
        sample_rate,
        commands: command_tx,
        backlog: VecDeque::new(),
        retired: retired_rx,
        faults: fault_rx,
        listen: Some(listen_rx),
        record: Some(record_rx),
        shared,
    };
    (engine, control)
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
        self.drain_commands();
        self.drain_hub();
        let channels = channels.max(1);
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

    fn drain_commands(&mut self) {
        while let Some(command) = self.commands.try_pop() {
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
                    self.retire(Retired::Cables(old));
                }
                Command::Order(order) => {
                    let old = std::mem::replace(&mut self.order, order);
                    self.retire(Retired::Order(old));
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
        if slot >= MAX_MODULES {
            self.shared.ignored.fetch_add(1, Ordering::Relaxed);
            self.retire(Retired::Module(module));
            return;
        }
        if let Some(old) = self.slots[slot].replace(module) {
            self.retire(Retired::Module(old));
        }
        self.tags[slot] = tag;
        self.kinds[slot] = Some(kind);
        self.outputs[slot] = [[0.0; SUB_BLOCK]; MAX_OUTPUTS];
        for (index, value) in knobs.iter().enumerate() {
            let value = kazoo_core::sanitize_sample(*value);
            self.glides[slot][index] = Glide::resting(value, kind.spec().knobs.get(index));
            self.knobs[slot][index] = value;
            self.shared.store_knob(slot, index, value);
        }
    }

    fn remove(&mut self, slot: usize) {
        if slot >= MAX_MODULES {
            self.shared.ignored.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if let Some(old) = self.slots[slot].take() {
            self.retire(Retired::Module(old));
        }
        self.kinds[slot] = None;
        self.outputs[slot] = [[0.0; SUB_BLOCK]; MAX_OUTPUTS];
    }

    fn knob(&mut self, slot: usize, knob: usize, target: f32, glide_frames: u32) {
        if slot >= MAX_MODULES || knob >= MAX_KNOBS || !target.is_finite() {
            self.shared.ignored.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let spec = self.kinds[slot].and_then(|kind| kind.spec().knobs.get(knob));
        let glide = &mut self.glides[slot][knob];
        if let Some(spec) = spec {
            let frames = glide_frames.max(self.min_glide);
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
        self.knobs[slot][knob] = glide.value;
        self.shared.store_knob(slot, knob, glide.value);
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
        while let Some(message) = hub.try_recv() {
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
        self.follow_desk();
        let tick = Tick::new(self.sample_rate, self.bpm, self.beat);
        for index in 0..self.order.slots().len() {
            let slot = usize::from(self.order.slots()[index]);
            self.render_module(slot, &tick);
        }
        let mut left = [0.0; SUB_BLOCK];
        let mut right = [0.0; SUB_BLOCK];
        for slot in 0..MAX_MODULES {
            if self.kinds[slot] == Some(Kind::OUT) && self.slots[slot].is_some() {
                for frame in 0..SUB_BLOCK {
                    left[frame] += self.outputs[slot][crate::dsp::FEED_LEFT][frame];
                    right[frame] += self.outputs[slot][crate::dsp::FEED_RIGHT][frame];
                }
            }
        }
        self.master.process(&mut left, &mut right);
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
        let Some(kind) = self.kinds[slot] else {
            return;
        };
        for knob in 0..MAX_KNOBS {
            let glide = &mut self.glides[slot][knob];
            let sweep = glide.advance(kind.spec().knobs.get(knob));
            self.sweeps[knob] = sweep;
            if sweep.frames > 0 {
                let value = glide.value;
                self.knobs[slot][knob] = value;
                self.shared.store_knob(slot, knob, value);
            }
        }
        let mut connected = [false; MAX_INPUTS];
        for (input, plugged) in connected.iter_mut().enumerate() {
            *plugged = gather(
                &self.cables,
                &self.outputs,
                &mut self.lines,
                slot,
                input,
                &mut self.inputs[input],
            );
        }
        let mut knob_patched = [false; MAX_KNOBS];
        for (knob, plugged) in knob_patched.iter_mut().enumerate() {
            *plugged = gather(
                &self.cables,
                &self.outputs,
                &mut self.lines,
                slot,
                MAX_INPUTS + knob,
                &mut self.knob_cv[knob],
            );
        }
        let Some(module) = self.slots[slot].as_mut() else {
            return;
        };
        module.process(
            tick,
            Io {
                spec: kind.spec(),
                knobs: &self.knobs[slot],
                sweeps: &self.sweeps,
                knob_cv: &self.knob_cv,
                knob_patched,
                inputs: &self.inputs,
                connected,
                outputs: &mut self.outputs[slot],
            },
        );
        let healthy = self.outputs[slot]
            .iter()
            .all(|port| port.iter().all(|sample| sample.is_finite()));
        if !healthy {
            self.outputs[slot] = [[0.0; SUB_BLOCK]; MAX_OUTPUTS];
            module.reset();
            self.shared.faults.fetch_add(1, Ordering::Relaxed);
            let fault = Fault {
                slot,
                tag: self.tags[slot],
            };
            if self.faults.try_push(fault).is_err() {
                self.shared
                    .faults_unreported
                    .fetch_add(1, Ordering::Relaxed);
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
    outputs: &[[Block; MAX_OUTPUTS]],
    lines: &mut lines::Lines,
    slot: usize,
    jack: usize,
    buffer: &mut Block,
) -> bool {
    let source = cables.route(slot, jack).and_then(|route| {
        outputs
            .get(usize::from(route.slot))
            .and_then(|ports| ports.get(usize::from(route.port)))
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

    /// Send a command. If the ring is full it waits in order on this side
    /// and goes with the next [`Self::pump`].
    pub fn send(&mut self, command: Command) {
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
        while let Some(item) = self.retired.try_pop() {
            drop(item);
        }
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

    /// Items waiting in the retire ring.
    #[must_use]
    pub fn retired_waiting(&self) -> usize {
        self.retired.occupied_len()
    }

    fn flush(&mut self) {
        while let Some(command) = self.backlog.pop_front() {
            if let Err(command) = self.commands.try_push(command) {
                self.backlog.push_front(command);
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests;
