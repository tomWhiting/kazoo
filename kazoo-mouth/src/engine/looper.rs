use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use kazoo_core::sanitize_sample;
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};

use super::Command;
use super::fx::Ramp;

/// A hand looper holds a sung phrase or two: sixteen bars at 120 BPM.
pub const LOOP_SECONDS: f64 = 32.0;
/// Thirty-two seconds at 96 kHz bounds memory even on faster devices.
pub const MAX_LOOP_FRAMES: usize = 3_072_000;
/// A 45 degree tape splice at 30 ips lasts 8.3 ms: short enough to preserve hits.
pub const SEAM_SECONDS: f32 = 0.008;
/// Four tracks retain three undoable layers and a recording buffer, using
/// 24.6 MB at 48 kHz and 49.2 MB at 96 kHz, including splice continuations.
pub const BUFFERS: usize = 4;
/// Sixty-four cells per frame merge a thirty-two second loop in half a second.
pub const MERGE_CELLS_PER_FRAME: usize = 64;
/// Sixty-four cells per frame copy a thirty-two second loop in half a second.
pub const BOUNCE_CELLS_PER_FRAME: usize = 64;
/// A close shorter than a quarter second is a slip of the finger.
pub const MIN_TAKE_SECONDS: f64 = 0.25;
pub const VOLUME_DEFAULT: f32 = 0.8;
pub const BAR_CHOICES: [u8; 5] = [1, 2, 4, 8, 16];
pub const BEATS_PER_BAR: f64 = 4.0;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LoopLength {
    #[default]
    Free,
    Bars(u8),
}

impl LoopLength {
    pub const fn bars(self) -> u8 {
        match self {
            Self::Free => 0,
            Self::Bars(bars) => bars,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    Blank,
    Armed,
    Taking,
    Playing,
    Dubbing,
    Waiting,
    Stopped,
    NoTape,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Note {
    None,
    NoTape,
    TooShort,
    DoesNotFit,
    ClearFirst,
    Merging,
    SaveCancelled,
}

#[derive(Debug, Default)]
pub struct LoopGauges {
    pub state: AtomicU8,
    pub n: AtomicUsize,
    pub capacity: AtomicUsize,
    pub head: AtomicUsize,
    pub taken: AtomicUsize,
    pub layers: AtomicU8,
    pub merging: AtomicBool,
    pub saving: AtomicBool,
    pub bars: AtomicU8,
    pub loop_bpm: AtomicU64,
    pub off_tempo: AtomicBool,
    pub arcs: [[AtomicU64; 2]; BUFFERS],
    pub beat_in_bar: AtomicU64,
    pub note: AtomicU8,
    pub note_seq: AtomicU32,
}

#[derive(Debug)]
pub enum LoopOut {
    Bounced {
        samples: Box<[f32]>,
        rate: u32,
        layers: u8,
    },
    Refused(Box<[f32]>),
}

pub struct LoopFeed {
    pub bounce_in: HeapProd<Box<[f32]>>,
    pub out: HeapCons<LoopOut>,
    pub gauges: Arc<LoopGauges>,
}

impl std::fmt::Debug for LoopFeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoopFeed")
            .field("gauges", &self.gauges)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Layer {
    buffer: u8,
    start: usize,
    written: usize,
}

#[derive(Debug)]
struct Bounce {
    samples: Box<[f32]>,
    copied: usize,
}

#[derive(Debug, Clone, Copy)]
struct Undo {
    buffer: u8,
    remaining: usize,
}

#[derive(Debug, Clone, Copy)]
struct Locate {
    old: usize,
    elapsed: usize,
}

pub struct Looper {
    buffers: [Box<[f32]>; BUFFERS],
    n: Option<usize>,
    stack: [Layer; BUFFERS],
    layers: usize,
    head: usize,
    state: State,
    length: LoopLength,
    anchor: Option<f64>,
    loop_bpm: f64,
    volume: Ramp,
    play: Ramp,
    env: f32,
    env_frames: usize,
    out_start: f32,
    punch_out: bool,
    fresh_seam: usize,
    merge: Option<usize>,
    bounce: Option<Bounce>,
    held: [Option<LoopOut>; 2],
    quarter_sine: [f32; 64],
    capacity: usize,
    tail: usize,
    rate: u32,
    previous_beat: f64,
    undo: Option<Undo>,
    clearing: bool,
    stopping: bool,
    locate: Option<Locate>,
    bounce_in: HeapCons<Box<[f32]>>,
    out: HeapProd<LoopOut>,
    gauges: Arc<LoopGauges>,
}

impl std::fmt::Debug for Looper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Looper")
            .field("state", &self.state)
            .field("n", &self.n)
            .field("layers", &self.layers)
            .finish_non_exhaustive()
    }
}

fn resident(cells: usize) -> Result<Box<[f32]>, std::collections::TryReserveError> {
    let mut memory = Vec::new();
    memory.try_reserve_exact(cells)?;
    memory.resize(cells, 0.0);
    Ok(memory.into_boxed_slice())
}

impl Looper {
    pub fn new(rate: u32, length: LoopLength, volume: f32) -> (Self, LoopFeed) {
        let capacity = ((LOOP_SECONDS * f64::from(rate)).round() as usize).min(MAX_LOOP_FRAMES);
        let (mut looper, feed) = Self::with_capacity(rate, capacity);
        looper.length = length;
        looper.volume.snap(if volume.is_finite() {
            volume.clamp(0.0, 1.0)
        } else {
            VOLUME_DEFAULT
        });
        looper.publish(120.0);
        (looper, feed)
    }

    pub fn with_capacity(rate: u32, capacity: usize) -> (Self, LoopFeed) {
        let capacity = capacity.min(MAX_LOOP_FRAMES);
        let tail = (SEAM_SECONDS * rate as f32).round().max(1.0) as usize;
        let mut buffers: [Box<[f32]>; BUFFERS] = std::array::from_fn(|_| Box::default());
        let mut ready = capacity > 0 && rate > 0;
        if ready {
            for buffer in &mut buffers {
                if let Ok(memory) = resident(capacity + tail) {
                    *buffer = memory;
                } else {
                    ready = false;
                    break;
                }
            }
        }
        if !ready {
            // Partial reservations are released before the callback owns this object.
            buffers = std::array::from_fn(|_| Box::default());
        }
        let (bounce_in, bounce_rx) = HeapRb::new(2).split();
        let (out_tx, out) = HeapRb::new(4).split();
        let gauges = Arc::new(LoopGauges::default());
        gauges.note.store(Note::None as u8, Ordering::Relaxed);
        let looper = Self {
            buffers,
            n: None,
            stack: [Layer::default(); BUFFERS],
            layers: 0,
            head: 0,
            state: if ready { State::Blank } else { State::NoTape },
            length: LoopLength::Free,
            anchor: None,
            loop_bpm: 0.0,
            volume: Ramp::new(VOLUME_DEFAULT, (rate as f32 * 0.01).round() as usize),
            play: Ramp::new(1.0, tail),
            env: 0.0,
            env_frames: 0,
            out_start: 0.0,
            punch_out: false,
            fresh_seam: 0,
            merge: None,
            bounce: None,
            held: [None, None],
            quarter_sine: std::array::from_fn(|i| {
                (i as f32 / 63.0 * std::f32::consts::FRAC_PI_2).sin()
            }),
            capacity: if ready { capacity } else { 0 },
            tail,
            rate,
            previous_beat: f64::NAN,
            undo: None,
            clearing: false,
            stopping: false,
            locate: None,
            bounce_in: bounce_rx,
            out: out_tx,
            gauges: Arc::clone(&gauges),
        };
        looper.publish(120.0);
        (
            looper,
            LoopFeed {
                bounce_in,
                out,
                gauges,
            },
        )
    }

    fn note(&self, note: Note) {
        self.gauges.note.store(note as u8, Ordering::Relaxed);
        self.gauges.note_seq.fetch_add(1, Ordering::Relaxed);
    }

    pub fn command(&mut self, command: Command, bpm: f64) {
        if self.state == State::NoTape {
            self.note(Note::NoTape);
            return;
        }
        match command {
            Command::LoopRecord => self.record(bpm),
            Command::LoopUndo => self.undo(),
            Command::LoopClear => self.clear(),
            Command::LoopLength(length) => {
                if self.state == State::Blank {
                    if matches!(length, LoopLength::Free) || BAR_CHOICES.contains(&length.bars()) {
                        self.length = length;
                    } else {
                        self.note(Note::DoesNotFit);
                    }
                } else {
                    self.note(Note::ClearFirst);
                }
            }
            Command::LoopPlay(on) => self.set_playing(on),
            Command::LoopVolume(volume) if !volume.is_nan() => {
                self.volume.aim(volume.clamp(0.0, 1.0));
            }
            _ => {}
        }
    }

    fn record(&mut self, bpm: f64) {
        if self.clearing || self.undo.is_some() {
            return;
        }
        match self.state {
            State::Blank => {
                self.cancel_bounce();
                self.stack[0] = Layer::default();
                self.layers = 1;
                self.head = 0;
                self.play.snap(1.0);
                match self.length {
                    LoopLength::Free => self.state = State::Taking,
                    LoopLength::Bars(bars) => {
                        let n = (f64::from(bars) * BEATS_PER_BAR * 60.0 / bpm
                            * f64::from(self.rate))
                        .round();
                        if !bpm.is_finite() || bpm <= 0.0 || n < 1.0 || n > self.capacity as f64 {
                            self.layers = 0;
                            self.note(Note::DoesNotFit);
                        } else {
                            self.n = Some(n as usize);
                            self.loop_bpm = bpm;
                            self.state = State::Armed;
                        }
                    }
                }
            }
            State::Taking => {
                if self.stack[0].written
                    < (MIN_TAKE_SECONDS * f64::from(self.rate)).round() as usize
                {
                    self.note(Note::TooShort);
                } else {
                    self.close_take();
                }
            }
            State::Playing | State::Stopped => {
                if self.state == State::Stopped {
                    self.set_playing(true);
                }
                self.punch_in();
            }
            State::Dubbing => self.punch_out(),
            State::Waiting => self.state = State::Playing,
            State::Armed => self.blank(),
            State::NoTape => {}
        }
    }

    const fn close_take(&mut self) {
        self.n = Some(self.stack[0].written);
        self.head = 0;
        self.fresh_seam = self.tail;
        self.state = State::Playing;
    }

    fn punch_in(&mut self) {
        let free = (0..BUFFERS).find(|buffer| {
            !self.stack[..self.layers]
                .iter()
                .any(|layer| usize::from(layer.buffer) == *buffer)
        });
        if let Some(buffer) = free {
            self.stack[self.layers] = Layer {
                buffer: buffer as u8,
                start: self.head,
                written: 0,
            };
            self.layers += 1;
            self.env = 0.0;
            self.env_frames = 0;
            self.punch_out = false;
            self.state = State::Dubbing;
            if self.layers == BUFFERS {
                self.merge = Some(0);
            }
        } else {
            self.state = State::Waiting;
        }
    }

    fn undo(&mut self) {
        self.cancel_bounce();
        if matches!(self.state, State::Taking | State::Armed) {
            self.blank();
            return;
        }
        if self.layers == 0 || self.undo.is_some() {
            return;
        }
        if self.merge.is_some() && self.layers <= 2 {
            self.note(Note::Merging);
            return;
        }
        if self.layers == 1 {
            self.clear();
            return;
        }
        self.undo = Some(Undo {
            buffer: self.stack[self.layers - 1].buffer,
            remaining: self.tail,
        });
        if matches!(self.state, State::Dubbing | State::Waiting) {
            self.state = State::Playing;
        }
    }

    const fn punch_out(&mut self) {
        if !self.punch_out {
            self.punch_out = true;
            self.out_start = self.env;
            self.env_frames = 0;
        }
    }

    fn clear(&mut self) {
        self.cancel_bounce();
        if matches!(self.state, State::Taking | State::Armed | State::Stopped) {
            self.blank();
            return;
        }
        // Keep the merge mask and any layer fade until the whole sum reaches zero.
        self.clearing = true;
        self.play.aim(0.0);
        if self.state == State::Dubbing {
            self.punch_out();
        }
    }

    const fn blank(&mut self) {
        self.n = None;
        self.layers = 0;
        self.head = 0;
        self.anchor = None;
        self.loop_bpm = 0.0;
        self.merge = None;
        self.undo = None;
        self.locate = None;
        self.fresh_seam = 0;
        self.clearing = false;
        self.stopping = false;
        self.punch_out = false;
        self.state = State::Blank;
        self.play.snap(1.0);
    }

    fn set_playing(&mut self, on: bool) {
        if self.n.is_none() || matches!(self.state, State::Taking | State::Armed) {
            return;
        }
        if on {
            if self.state == State::Stopped || self.stopping {
                if self.stopping {
                    self.locate = Some(Locate {
                        old: self.head,
                        elapsed: 0,
                    });
                }
                self.head = self.aligned(self.previous_beat).unwrap_or(0);
                self.state = State::Playing;
                self.stopping = false;
                self.play.aim(1.0);
            }
        } else if self.state != State::Stopped {
            self.stopping = true;
            if self.state == State::Dubbing {
                self.punch_out();
            } else {
                self.play.aim(0.0);
            }
        }
    }

    fn qsin(&self, fraction: f32) -> f32 {
        let phase = fraction.clamp(0.0, 1.0) * 63.0;
        let cell = phase as usize;
        let next = (cell + 1).min(63);
        (self.quarter_sine[next] - self.quarter_sine[cell])
            .mul_add(phase - cell as f32, self.quarter_sine[cell])
    }

    fn layer_at(&self, index: usize, cell: usize) -> f32 {
        let Some(n) = self.n else {
            return 0.0;
        };
        let layer = self.stack[index];
        if layer.written < n && (cell + n - layer.start) % n >= layer.written {
            return 0.0;
        }
        if index == 1 && self.merge.is_some_and(|merged| cell < merged) {
            return 0.0;
        }
        let mut sample = self.buffers[usize::from(layer.buffer)][cell];
        if index == 0 && self.fresh_seam > 0 && cell < self.tail {
            sample *= self.qsin(cell as f32 / self.tail as f32);
        }
        if let Some(undo) = self.undo
            && undo.buffer == layer.buffer
        {
            sample *= undo.remaining as f32 / self.tail as f32;
        }
        sample
    }

    fn read(&self, cell: usize) -> f32 {
        sanitize_sample(
            (0..self.layers)
                .map(|index| self.layer_at(index, cell))
                .sum(),
        )
    }

    fn aligned(&self, beat: f64) -> Option<usize> {
        let anchor = self.anchor?;
        let n = self.n?;
        if !beat.is_finite() {
            return None;
        }
        let span = f64::from(self.length.bars()) * BEATS_PER_BAR;
        Some((((beat - anchor).rem_euclid(span) / span * n as f64).round() as usize) % n)
    }

    fn desk(&mut self, beat: f64, bpm: f64) {
        if self.state == State::Armed {
            if !beat.is_finite() {
                self.state = State::Taking;
            } else if self.previous_beat.is_finite()
                && (beat / BEATS_PER_BAR).floor() > (self.previous_beat / BEATS_PER_BAR).floor()
            {
                self.anchor = Some(BEATS_PER_BAR * (beat / BEATS_PER_BAR).floor());
                self.state = State::Taking;
            }
        } else if !matches!(
            self.state,
            State::Taking | State::Blank | State::Stopped | State::NoTape
        ) && beat.is_finite()
            && (!self.previous_beat.is_finite()
                || (beat - self.previous_beat - bpm / 60.0 / f64::from(self.rate)).abs() > 0.001)
            && let Some(head) = self.aligned(beat)
        {
            self.locate = Some(Locate {
                old: self.head,
                elapsed: 0,
            });
            self.head = head;
        }
        self.previous_beat = beat;
    }

    fn write(&mut self, sample: f32) {
        if self.state == State::Taking {
            let cell = self.stack[0].written;
            self.buffers[0][cell] = sample;
            self.stack[0].written += 1;
            if self.stack[0].written == self.n.unwrap_or(self.capacity) {
                self.close_take();
            }
            return;
        }
        if self.fresh_seam > 0
            && let Some(n) = self.n
        {
            let cell = self.tail - self.fresh_seam;
            self.buffers[0][n + cell] = sample;
            self.fresh_seam -= 1;
            if self.fresh_seam == 0 {
                for cell in 0..self.tail.min(n) {
                    let phase = cell as f32 / self.tail as f32;
                    self.buffers[0][cell] = sanitize_sample(self.qsin(phase).mul_add(
                        self.buffers[0][cell],
                        self.qsin(1.0 - phase) * self.buffers[0][n + cell],
                    ));
                }
            }
        }
        if self.state == State::Dubbing
            && let Some(n) = self.n
        {
            let layer = &mut self.stack[self.layers - 1];
            let to = &mut self.buffers[usize::from(layer.buffer)][self.head];
            let added = self.env * sample;
            *to = sanitize_sample(if layer.written < n {
                added
            } else {
                *to + added
            });
            layer.written = (layer.written + 1).min(n);
            self.env_frames = (self.env_frames + 1).min(self.tail);
            if self.punch_out {
                self.env = self.out_start * (1.0 - self.env_frames as f32 / self.tail as f32);
                if self.env_frames == self.tail {
                    self.env = 0.0;
                    self.state = State::Playing;
                    self.punch_out = false;
                    if self.stopping {
                        self.play.aim(0.0);
                    }
                }
            } else {
                self.env = self.env_frames as f32 / self.tail as f32;
            }
        }
    }

    fn merge_frame(&mut self) {
        // The bed must finish its splice before any dub is folded into it.
        if self.fresh_seam > 0 || self.clearing {
            return;
        }
        let (Some(start), Some(n)) = (self.merge, self.n) else {
            return;
        };
        let end = (start + MERGE_CELLS_PER_FRAME).min(n);
        let bed = usize::from(self.stack[0].buffer);
        for cell in start..end {
            self.buffers[bed][cell] =
                sanitize_sample(self.buffers[bed][cell] + self.layer_at(1, cell));
        }
        if end == n {
            self.stack.copy_within(2..self.layers, 1);
            self.layers -= 1;
            self.merge = None;
            if self.state == State::Waiting {
                self.punch_in();
            }
        } else {
            self.merge = Some(end);
        }
    }

    fn emit(&mut self, item: LoopOut) {
        if let Err(item) = self.out.try_push(item) {
            let index = usize::from(self.held[0].is_some());
            debug_assert!(self.held[index].is_none());
            self.held[index] = Some(item);
        }
    }

    fn cancel_bounce(&mut self) {
        if let Some(bounce) = self.bounce.take() {
            self.emit(LoopOut::Refused(bounce.samples));
            self.note(Note::SaveCancelled);
        }
    }

    fn service_rings(&mut self) {
        for slot in &mut self.held {
            if let Some(item) = slot.take()
                && let Err(item) = self.out.try_push(item)
            {
                *slot = Some(item);
            }
        }
        if self.bounce.is_none()
            && self.held.iter().any(Option::is_none)
            && let Some(samples) = self.bounce_in.try_pop()
        {
            if self.n == Some(samples.len())
                && self.fresh_seam == 0
                && !matches!(
                    self.state,
                    State::Taking | State::Armed | State::Blank | State::NoTape
                )
                && !self.clearing
            {
                self.bounce = Some(Bounce { samples, copied: 0 });
            } else {
                self.emit(LoopOut::Refused(samples));
                self.note(Note::SaveCancelled);
            }
        }
    }

    fn bounce_frame(&mut self) {
        if let Some(mut bounce) = self.bounce.take() {
            let end = (bounce.copied + BOUNCE_CELLS_PER_FRAME).min(bounce.samples.len());
            for cell in bounce.copied..end {
                bounce.samples[cell] = self.read(cell);
            }
            bounce.copied = end;
            if end == bounce.samples.len() {
                self.emit(LoopOut::Bounced {
                    samples: bounce.samples,
                    rate: self.rate,
                    layers: self.layers as u8,
                });
            } else {
                self.bounce = Some(bounce);
            }
        }
    }

    pub fn process(&mut self, live: &[f32], beats: &[f64], bpm: f64, output: &mut [f32]) {
        self.process_audio(live, beats, bpm, output);
        self.publish(bpm);
    }

    pub fn process_audio(&mut self, live: &[f32], beats: &[f64], bpm: f64, output: &mut [f32]) {
        self.service_rings();
        for (index, to) in output.iter_mut().enumerate() {
            let beat = beats.get(index).copied().unwrap_or(f64::NAN);
            self.desk(beat, bpm);
            let taking = self.state == State::Taking;
            let playing = matches!(self.state, State::Playing | State::Dubbing | State::Waiting);
            let mut sample = if playing { self.read(self.head) } else { 0.0 };
            if playing && let Some(mut locate) = self.locate {
                let phase = locate.elapsed as f32 / self.tail as f32;
                sample = self
                    .qsin(phase)
                    .mul_add(sample, self.qsin(1.0 - phase) * self.read(locate.old));
                locate.old = (locate.old + 1) % self.n.unwrap_or(1);
                locate.elapsed += 1;
                self.locate = (locate.elapsed < self.tail).then_some(locate);
            }
            *to = sanitize_sample(sample * self.play.next() * self.volume.next());
            self.merge_frame();
            self.write(sanitize_sample(live.get(index).copied().unwrap_or(0.0)).clamp(-4.0, 4.0));
            if playing && !taking {
                self.head = (self.head + 1) % self.n.unwrap_or(1);
            }
            if let Some(mut undo) = self.undo {
                undo.remaining -= 1;
                if undo.remaining == 0 {
                    self.layers -= 1;
                    self.undo = None;
                } else {
                    self.undo = Some(undo);
                }
            }
            self.bounce_frame();
            if self.clearing && !self.play.moving() {
                self.blank();
            } else if self.stopping && self.state != State::Dubbing && !self.play.moving() {
                self.state = State::Stopped;
                self.stopping = false;
            }
        }
    }

    pub fn publish(&self, bpm: f64) {
        let gauges = &self.gauges;
        gauges.state.store(self.state as u8, Ordering::Relaxed);
        gauges.n.store(self.n.unwrap_or(0), Ordering::Relaxed);
        gauges.capacity.store(self.capacity, Ordering::Relaxed);
        gauges.head.store(self.head, Ordering::Relaxed);
        gauges.taken.store(
            if self.layers > 0 {
                self.stack[0].written
            } else {
                0
            },
            Ordering::Relaxed,
        );
        gauges.layers.store(self.layers as u8, Ordering::Relaxed);
        gauges
            .merging
            .store(self.merge.is_some() && !self.clearing, Ordering::Relaxed);
        gauges
            .saving
            .store(self.bounce.is_some(), Ordering::Relaxed);
        gauges.bars.store(self.length.bars(), Ordering::Relaxed);
        gauges
            .loop_bpm
            .store(self.loop_bpm.to_bits(), Ordering::Relaxed);
        gauges.off_tempo.store(
            self.length.bars() > 0 && self.loop_bpm > 0.0 && (bpm - self.loop_bpm).abs() > 0.1,
            Ordering::Relaxed,
        );
        gauges
            .beat_in_bar
            .store(self.previous_beat.to_bits(), Ordering::Relaxed);
        for (index, arc) in gauges.arcs.iter().enumerate() {
            let layer = if index < self.layers {
                self.stack[index]
            } else {
                Layer::default()
            };
            arc[0].store(layer.start as u64, Ordering::Relaxed);
            arc[1].store(layer.written as u64, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests;
