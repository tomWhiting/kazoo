//! The sample player: a polyphonic voice for a loaded sample.
//!
//! # Modes
//!
//! The `mode` knob picks how a trigger plays (see [`Mode`]): one-shot, gate,
//! loop, ping-pong, slice or granular. The `reverse` knob turns any of them
//! round, loop points and slices included.
//!
//! # Inputs and pitch
//!
//! A host drives the player with audio-rate [`PlayerInputs`]: a `gate`
//! (high above 0.5; each rise starts a voice), a `pitch` input in volts per
//! octave and a `velocity` (0 to 1, read at the rise). Pitch 0.0 plays the
//! sample as recorded, +1.0 an octave up, -1.0 an octave down: on the wall,
//! where 0.0 is C4, a sample's recorded pitch is heard at C4. The `pitch`
//! (semitones) and `fine` (cents) knobs transpose on top. The voice the gate
//! is holding follows the pitch input continuously; once released a voice
//! keeps the pitch it had. Hosts with notes instead call
//! [`SamplePlayer::note_on`], where key 60 is the recorded pitch.
//!
//! # Sound
//!
//! Every read is four-point Hermite; above an octave up it reads the
//! sample's band-limited copies, so pitching up does not alias. Up to
//! [`VOICES`] voices sound at once; a trigger beyond that steals the oldest,
//! which fades out over 5 ms instead of cutting. Region ends fade over
//! 1.5 ms, loops crossfade over the `crossfade` knob, and swapping the
//! sample fades its voices out; nothing clicks.
//!
//! # Real-time contract
//!
//! [`SamplePlayer::new`] allocates. Every other method is real-time safe:
//! no allocation, lock, I/O, free or panic, whatever it is given. A sample
//! arrives with [`SamplePlayer::load`] and leaves through the
//! [`SampleReaper`] returned by `new`, which the host drains off the audio
//! thread; the player itself never drops the last reference to a sample.
//! Dropping the player drops its samples, so drop it off the audio thread.

mod envelope;
mod params;
mod voice;

use std::fmt;
use std::sync::Arc;

use kazoo_fx::dsp::Smoothed;
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};

pub use params::{MAX_SLICES, Mode, PARAM_COUNT, PARAMS, index, param_index};

use crate::SampleData;
use params::{Settings, defaults};
use voice::{Context, Region, Source, Start, Voice};

/// Voices that can sound at once.
pub const VOICES: usize = 8;

/// Voice slots: the playing voices plus room for stolen ones to fade.
const SLOTS: usize = VOICES + 4;

/// How long a stolen or swapped-out voice takes to fade, in seconds.
const STEAL_SECONDS: f32 = 0.005;

/// The fade into a region's end, in seconds.
const EDGE_SECONDS: f64 = 0.0015;

/// Samples waiting to be dropped off the audio thread.
const RETIRE_CAPACITY: usize = 8;

/// Points in the grain window table.
const HANN_POINTS: usize = 1024;

/// How long level and pan take to follow their knobs, in seconds.
const SMOOTH_SECONDS: f32 = 0.01;

/// The player's audio-rate inputs for one block. An empty slice is an
/// unplugged input: gate low, pitch 0, velocity 1. A slice shorter than
/// the block holds its last value past its end; a NaN or infinite value
/// reads as unplugged.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlayerInputs<'a> {
    /// Gate or trigger: high above 0.5.
    pub gate: &'a [f32],
    /// Pitch in volts per octave; 0.0 is the recorded pitch.
    pub pitch: &'a [f32],
    /// Velocity 0 to 1, read when the gate rises.
    pub velocity: &'a [f32],
}

/// The off-thread end of a player's retire queue: samples the player is
/// finished with. Call [`Self::collect`] regularly from a control thread.
pub struct SampleReaper {
    consumer: HeapCons<Arc<SampleData>>,
}

impl fmt::Debug for SampleReaper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SampleReaper").finish_non_exhaustive()
    }
}

impl SampleReaper {
    /// Drop every retired sample; returns how many there were. Freeing a
    /// sample's memory happens here, never on the audio thread.
    pub fn collect(&mut self) -> usize {
        let mut count = 0;
        while let Some(sample) = self.consumer.try_pop() {
            drop(sample);
            count += 1;
        }
        count
    }
}

/// The voices and the gate's state.
#[derive(Debug, Clone)]
struct Bank {
    voices: [Voice; SLOTS],
    /// Triggers so far, to tell the oldest voice.
    clock: u64,
    gate_high: bool,
}

/// A polyphonic sample player. See the [module docs](self).
pub struct SamplePlayer {
    rate: f32,
    values: [f32; PARAM_COUNT],
    bank: Bank,
    current: Option<Arc<SampleData>>,
    outgoing: Option<Arc<SampleData>>,
    pending: Option<Arc<SampleData>>,
    unload: bool,
    retire: HeapProd<Arc<SampleData>>,
    hann: [f32; HANN_POINTS],
    level: Smoothed,
    pan: Smoothed,
    faults: u64,
}

impl fmt::Debug for SamplePlayer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SamplePlayer")
            .field("rate", &self.rate)
            .field("sample", &self.current.as_ref().map(|data| data.name()))
            .field("voices", &self.active_voices())
            .field("faults", &self.faults)
            .finish_non_exhaustive()
    }
}

impl SamplePlayer {
    /// A player at 48 kHz with every knob at its default and no sample,
    /// and the reaper for the samples it lets go of. Allocates; call
    /// [`Self::prepare`] with the real rate before playing.
    #[must_use]
    pub fn new() -> (Self, SampleReaper) {
        let (retire, consumer) = HeapRb::new(RETIRE_CAPACITY).split();
        let mut hann = [0.0f32; HANN_POINTS];
        for (i, point) in hann.iter_mut().enumerate() {
            let phase = i as f32 / (HANN_POINTS - 1) as f32;
            *point = (std::f32::consts::PI * phase).sin().powi(2);
        }
        let values = defaults();
        let voices = std::array::from_fn(|i| Voice::new(0x5A17_0000 + i as u32 * 7_919));
        let mut player = Self {
            rate: 48_000.0,
            values,
            bank: Bank {
                voices,
                clock: 0,
                gate_high: false,
            },
            current: None,
            outgoing: None,
            pending: None,
            unload: false,
            retire,
            hann,
            level: Smoothed::new(values[index::LEVEL]),
            pan: Smoothed::new(values[index::PAN]),
            faults: 0,
        };
        player.prepare(48_000.0);
        (player, SampleReaper { consumer })
    }

    /// Set the output rate and silence every voice. A rate that is not a
    /// positive, finite number is ignored.
    pub fn prepare(&mut self, sample_rate: f32) {
        if sample_rate.is_finite() && sample_rate > 0.0 {
            self.rate = sample_rate;
        }
        self.level.set_time(SMOOTH_SECONDS, self.rate);
        self.pan.set_time(SMOOTH_SECONDS, self.rate);
        self.reset();
    }

    /// Silence every voice at once and forget the gate.
    pub fn reset(&mut self) {
        for voice in &mut self.bank.voices {
            voice.stop();
        }
        self.bank.gate_high = false;
        self.level.snap(self.values[index::LEVEL]);
        self.pan.snap(self.values[index::PAN]);
    }

    /// Set knob `index` (see [`PARAMS`]). Clamped to its range and rounded
    /// if stepped; a NaN, an infinity or an unknown index is ignored.
    pub fn set_param(&mut self, index: usize, value: f32) {
        if !value.is_finite() {
            return;
        }
        if let (Some(slot), Some(spec)) = (self.values.get_mut(index), PARAMS.get(index)) {
            *slot = spec.clamp(value);
        }
    }

    /// Knob `index`'s value.
    #[must_use]
    pub fn param(&self, index: usize) -> Option<f32> {
        self.values.get(index).copied()
    }

    /// Hand the player a sample. It takes over at the start of the next
    /// block, once the voices on the old one (if any) have faded out. If a
    /// sample handed in earlier had not taken over yet, it is retired
    /// unplayed. Returns the sample back only if the retire queue is full;
    /// drain the reaper and try again.
    pub fn load(&mut self, sample: Arc<SampleData>) -> Result<(), Arc<SampleData>> {
        if let Some(waiting) = self.pending.take() {
            if let Err(waiting) = self.retire.try_push(waiting) {
                self.pending = Some(waiting);
                return Err(sample);
            }
        }
        self.pending = Some(sample);
        self.unload = false;
        Ok(())
    }

    /// Let go of the sample: its voices fade and it goes to the reaper.
    pub const fn unload(&mut self) {
        self.unload = true;
    }

    /// The sample playing now.
    #[must_use]
    pub const fn sample(&self) -> Option<&Arc<SampleData>> {
        self.current.as_ref()
    }

    /// Start a voice for `key` (60 is the recorded pitch) at `velocity`
    /// (0 to 1). Ignored without a sample.
    pub fn note_on(&mut self, key: u8, velocity: f32) {
        let settings = Settings::new(&self.values, self.rate);
        let Some(data) = self.current.as_deref() else {
            return;
        };
        let note = (f32::from(key) - 60.0) / 12.0;
        self.bank
            .trigger(data, &settings, Some(key), note, velocity, self.rate);
    }

    /// Release every voice held by `key`.
    pub fn note_off(&mut self, key: u8) {
        for voice in &mut self.bank.voices {
            if voice.active && voice.held && voice.key == Some(key) {
                voice.release();
            }
        }
    }

    /// Release every held voice.
    pub fn all_notes_off(&mut self) {
        for voice in &mut self.bank.voices {
            if voice.active && voice.held {
                voice.release();
            }
        }
    }

    /// Voices sounding now, fading ones included.
    #[must_use]
    pub fn active_voices(&self) -> usize {
        self.bank.voices.iter().filter(|voice| voice.active).count()
    }

    /// Where the newest voice is in the sample, 0 to 1.
    #[must_use]
    pub fn playhead(&self) -> Option<f32> {
        let data = self.current.as_deref()?;
        self.bank
            .voices
            .iter()
            .filter(|voice| voice.active && voice.source == Source::Current)
            .max_by_key(|voice| voice.age)
            .map(|voice| voice.playhead(data.frames()))
    }

    /// Blocks whose output was not finite and was silenced. Each one also
    /// stops every voice.
    #[must_use]
    pub const fn faults(&self) -> u64 {
        self.faults
    }

    /// Render one block into `output` (left, right), replacing what is
    /// there. The block is the shorter of the two outputs; anything past it
    /// is silenced.
    pub fn process(&mut self, inputs: &PlayerInputs<'_>, output: [&mut [f32]; 2]) {
        let [out_left, out_right] = output;
        out_left.fill(0.0);
        out_right.fill(0.0);
        let frames = out_left.len().min(out_right.len());
        self.settle_samples();
        let settings = Settings::new(&self.values, self.rate);
        self.level.set(settings.level);
        self.pan.set(settings.pan);
        let edge = (EDGE_SECONDS * f64::from(self.rate)).max(1.0);
        let current = self
            .current
            .as_deref()
            .map(|data| context(data, &settings, self.rate, edge, &self.hann));
        let outgoing = self
            .outgoing
            .as_deref()
            .map(|data| context(data, &settings, self.rate, edge, &self.hann));
        for i in 0..frames {
            let gate = input(inputs.gate, i, 0.0) > 0.5;
            let cv = input(inputs.pitch, i, 0.0);
            if gate && !self.bank.gate_high {
                if let Some(current) = &current {
                    let velocity = input(inputs.velocity, i, 1.0);
                    self.bank
                        .trigger(current.data, &settings, None, cv, velocity, self.rate);
                }
            } else if !gate && self.bank.gate_high {
                self.bank.release_gate();
            }
            self.bank.gate_high = gate;
            let (mut left, mut right) = (0.0f32, 0.0f32);
            for voice in &mut self.bank.voices {
                if !voice.active {
                    continue;
                }
                if voice.follows_cv {
                    voice.note = cv;
                }
                let reader = match voice.source {
                    Source::Current => current.as_ref(),
                    Source::Outgoing => outgoing.as_ref(),
                };
                let Some(reader) = reader else {
                    voice.stop();
                    continue;
                };
                let (l, r) = voice.tick(reader);
                left += l;
                right += r;
            }
            let level = self.level.step();
            let pan = self.pan.step();
            out_left[i] = left * level * (1.0 - pan).min(1.0);
            out_right[i] = right * level * (1.0 + pan).min(1.0);
        }
        let finite = out_left
            .iter()
            .chain(out_right.iter())
            .all(|x| x.is_finite());
        if !finite {
            out_left.fill(0.0);
            out_right.fill(0.0);
            for voice in &mut self.bank.voices {
                voice.stop();
            }
            self.faults += 1;
        }
    }

    /// Move samples along: retire the outgoing one once its voices are
    /// silent, then bring in a pending one (or let go of the current one).
    fn settle_samples(&mut self) {
        if let Some(outgoing) = self.outgoing.take() {
            let sounding = self
                .bank
                .voices
                .iter()
                .any(|voice| voice.active && voice.source == Source::Outgoing);
            if sounding {
                self.outgoing = Some(outgoing);
                return;
            }
            if let Err(outgoing) = self.retire.try_push(outgoing) {
                self.outgoing = Some(outgoing);
                return;
            }
        }
        if self.unload {
            if let Some(waiting) = self.pending.take() {
                if let Err(waiting) = self.retire.try_push(waiting) {
                    self.pending = Some(waiting);
                    return;
                }
            }
            self.unload = false;
            self.swap(None);
        } else if let Some(next) = self.pending.take() {
            self.swap(Some(next));
        }
    }

    /// Make `next` the current sample; the old one fades out.
    fn swap(&mut self, next: Option<Arc<SampleData>>) {
        self.outgoing = std::mem::replace(&mut self.current, next);
        let frames = STEAL_SECONDS * self.rate;
        for voice in &mut self.bank.voices {
            if voice.active {
                voice.source = Source::Outgoing;
                voice.fade_out(frames);
            }
        }
    }
}

impl Bank {
    /// Start a voice, stealing the oldest if all are busy.
    fn trigger(
        &mut self,
        data: &SampleData,
        settings: &Settings,
        key: Option<u8>,
        note: f32,
        velocity: f32,
        rate: f32,
    ) {
        let velocity = if velocity.is_finite() {
            velocity.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let region = Region::new(settings, data.frames(), settings.reverse);
        let slice = if settings.mode == Mode::Slice {
            slice_bounds(data, settings)
        } else {
            (region.start, region.end)
        };
        // A new gate voice takes over the pitch input from the last one.
        if key.is_none() {
            for voice in &mut self.voices {
                voice.follows_cv = false;
            }
        }
        let playing = self
            .voices
            .iter()
            .filter(|voice| voice.active && !voice.is_fading())
            .count();
        if playing >= VOICES {
            let oldest = self
                .voices
                .iter_mut()
                .filter(|voice| voice.active && !voice.is_fading())
                .min_by_key(|voice| voice.age);
            if let Some(oldest) = oldest {
                oldest.fade_out(STEAL_SECONDS * rate);
            }
        }
        let slot = self
            .voices
            .iter()
            .position(|voice| !voice.active)
            .unwrap_or_else(|| {
                // Every slot is busy fading: take the quietest.
                (0..SLOTS)
                    .min_by(|&a, &b| {
                        self.voices[a]
                            .fade_level()
                            .total_cmp(&self.voices[b].fade_level())
                    })
                    .unwrap_or(0)
            });
        self.clock += 1;
        self.voices[slot].start(&Start {
            age: self.clock,
            key,
            follows_cv: key.is_none(),
            note,
            gain: (1.0 - settings.velocity) + settings.velocity * velocity,
            mode: settings.mode,
            reverse: settings.reverse,
            slice,
            position: slice.0,
            scan: 0.0,
        });
    }

    /// The gate fell: release the voices it started.
    fn release_gate(&mut self) {
        for voice in &mut self.voices {
            if voice.active && voice.held && voice.key.is_none() {
                voice.release();
            }
        }
    }
}

/// What a voice reading `data` needs for one block at output `rate`.
fn context<'a>(
    data: &'a SampleData,
    settings: &'a Settings,
    rate: f32,
    edge: f64,
    hann: &'a [f32],
) -> Context<'a> {
    Context {
        data,
        settings,
        regions: [
            Region::new(settings, data.frames(), false),
            Region::new(settings, data.frames(), true),
        ],
        rate_ratio: f64::from(data.rate()) / f64::from(rate),
        edge,
        hann,
    }
}

/// The slice the `slice` knob picks, in playback coordinates.
fn slice_bounds(data: &SampleData, settings: &Settings) -> (f64, f64) {
    let forward = Region::new(settings, data.frames(), false);
    let (start, end) = (forward.start, forward.end);
    let pick =
        |count: usize| ((settings.slice * count as f32) as usize).min(count.saturating_sub(1));
    let (low, high) = if settings.slices == 0 {
        let inside = || {
            data.onsets()
                .iter()
                .map(|&at| at as f64)
                .filter(|&at| at > start && at < end)
        };
        let chosen = pick(inside().count() + 1);
        let low = if chosen == 0 {
            start
        } else {
            inside().nth(chosen - 1).unwrap_or(start)
        };
        (low, inside().nth(chosen).unwrap_or(end))
    } else {
        let width = (end - start) / f64::from(settings.slices);
        let low = (pick(settings.slices as usize) as f64).mul_add(width, start);
        (low, low + width)
    };
    if settings.reverse {
        (forward.frames - high, forward.frames - low)
    } else {
        (low, high)
    }
}

/// Input `i` of `signal`, or `unplugged` past its end or when not finite.
fn input(signal: &[f32], i: usize, unplugged: f32) -> f32 {
    match signal.get(i).or_else(|| signal.last()) {
        Some(&value) if value.is_finite() => value,
        Some(_) | None => unplugged,
    }
}

#[cfg(test)]
mod tests;
