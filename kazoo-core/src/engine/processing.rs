//! Audio processing: the real-time audio workhorse.
//!
//! The [`process_block`] function is called by the cpal output callback on
//! every buffer cycle. It drains commands, reads microphone input from the
//! ring buffer, runs the mixer (synths + effects), applies the soft limiter,
//! writes directly to the output buffer, and pushes display snapshots for
//! the UI. All state is owned by the output callback closure, keeping the
//! real-time path explicit and easy to audit.
//!
//! # Real-time rules
//!
//! Nothing here allocates, frees, locks or does I/O:
//!
//! - Everything a command adds (tracks, synths, layers, effects, clip audio)
//!   arrives fully built and prepared in the command; anything prepared for
//!   another sample rate or a smaller block is refused.
//! - Everything taken out, replaced or refused is sent to the reclaim thread
//!   (see [`super::reclaim`]) to be freed. Before taking a command, the
//!   callback makes sure the outbound ring has room for everything that
//!   command can retire; commands wait in their channel otherwise.
//! - Recordings are captured into a fixed pool of resident chunks that
//!   circulate through the reclaim thread, which assembles them into clips;
//!   the timeline the UI draws is built from shared references there too.
//! - Display snapshots are written into frames the UI hands back, whose
//!   buffers were sized when the engine started.
//! - Every collection is created with its maximum capacity.
//! - The command and disk channels are `crossbeam` channels: lock-free as
//!   long as nobody blocks on them, which holds because the UI only uses
//!   `try_send` for commands and the disk thread only polls.

use std::sync::Arc;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender};
use ringbuf::traits::{Consumer, Observer, Producer};
use ringbuf::{HeapCons, HeapProd};

use crate::analysis::{EnvelopeFollower, FormantData, PitchEstimate};
use crate::ipc::client::HubMessage;
use crate::ipc::follow::{TransportChange, TransportFollower};
use crate::ipc::link::{HubLinkAudio, RequestError};
use crate::ipc::types::{
    NOTE_OFF, NOTE_ON, TRANSPORT_PAUSED, TRANSPORT_PLAYING, TRANSPORT_STOPPED, TRANSPORT_UNCHANGED,
};
use crate::mixer::clip::{AudioClip, ClipData, ClipId, MAX_CLIPS_PER_TRACK};
use crate::mixer::{Mixer, Prepared, SynthLayer, Track, TrackId};
use crate::synthesis::SynthesisMode;
use crate::transport::metronome::Metronome;
use crate::transport::{TransportClock, TransportCommand};
use crate::{Db, MAX_TRACKS, Processor, sanitize_buffer, soft_limit_buffer};

use super::command::EngineCommand;
use super::disk::DiskCommand;
use super::display::{
    DisplayState, FORMANT_CAPACITY, RecordingSpan, TrackRecording, WAVEFORM_POINTS,
};
use super::reclaim::{
    FinishedTake, Inbound, Outbound, Parcel, Retired, TIMELINE_SOURCES, TakeId, TimelineSource,
    resident_buffer,
};
use super::stats::EngineStats;

#[cfg(test)]
mod block_tests;
#[cfg(test)]
mod tests;

/// Create a synthesis processor for the given mode and sample rate.
///
/// This factory function centralises the mapping from [`SynthesisMode`] to a
/// concrete `Box<dyn Processor>`. It allocates: build synths off the audio
/// thread (see [`prepared_synth`]).
#[must_use]
pub fn create_synth(mode: SynthesisMode, sample_rate: f32) -> Box<dyn Processor> {
    match mode {
        SynthesisMode::Passthrough => Box::new(crate::synthesis::PassthroughSynth),
        SynthesisMode::PitchTracked => {
            Box::new(crate::synthesis::PitchTrackedSynth::new(sample_rate))
        }
        SynthesisMode::Wavetable => {
            Box::new(crate::synthesis::WavetableOscillator::new(sample_rate))
        }
        SynthesisMode::Granular => Box::new(crate::synthesis::GranularSynth::new(sample_rate)),
        SynthesisMode::Vocoder => Box::new(crate::synthesis::Vocoder::new(sample_rate)),
        SynthesisMode::PhaseVocoder => Box::new(crate::synthesis::PhaseVocoder::new(sample_rate)),
    }
}

/// Create a synth for `mode`, set to `sample_rate` and prepared for blocks
/// of `buffer_size` samples: ready to be sent to the output callback, which
/// never allocates one itself.
#[must_use]
pub fn prepared_synth(mode: SynthesisMode, sample_rate: f32, buffer_size: usize) -> Prepared {
    Prepared::new(create_synth(mode, sample_rate), sample_rate, buffer_size)
}

/// Most objects one command can send out (e.g. a removed track and the take
/// it was recording). A command is only taken while the outbound ring has
/// at least this much room.
const MAX_RETIRED_PER_COMMAND: usize = 2;

/// Most objects accepting one item back from the reclaim thread can retire
/// (a clip that cannot be placed).
const MAX_RETIRED_PER_RETURN: usize = 1;

/// Recording chunks per possible track: one being written, one on its way
/// through the reclaim thread, and one spare.
const CHUNKS_PER_TRACK: usize = 3;

/// Recording chunk length: a quarter of a second, far longer than the
/// reclaim thread takes to hand a chunk back.
const fn chunk_len(sample_rate: u32) -> usize {
    let quarter_second = (sample_rate / 4) as usize;
    if quarter_second == 0 {
        1
    } else {
        quarter_second
    }
}

/// Outbound items the callback can hold when the ring is momentarily full
/// (see [`ReclaimLink::send`]).
const OVERFLOW_CAPACITY: usize = 4 * MAX_TRACKS;

/// Display snapshots pushed to the UI per second. Matches the TUI's frame
/// rate, so the four-slot display ring only overflows (and counts a dropped
/// frame) when the UI has genuinely fallen behind.
const DISPLAY_HZ: u32 = 60;

/// A recording in progress on one track.
struct ActiveTake {
    /// Identifies the take's chunks to the reclaim thread.
    take: TakeId,
    /// The chunk being written. Only `filled` samples are valid.
    chunk: Vec<f32>,
    /// Samples written to `chunk`.
    filled: usize,
    /// Samples recorded in the whole take so far.
    recorded: usize,
    /// Transport position (in samples) when recording began.
    start_position: u64,
    /// The track being recorded.
    track_id: TrackId,
}

/// Recording: the chunk pool and the takes in progress.
///
/// The pool is created (resident) with the engine: [`CHUNKS_PER_TRACK`] for
/// every possible track. Chunks leave full and come back empty from the
/// reclaim thread, so the pool never grows, and `pool` has room for all of
/// them.
struct Takes {
    pool: Vec<Vec<f32>>,
    active: Vec<ActiveTake>,
    next_take: TakeId,
}

/// How the engine keeps in step with the kazoo-mix desk's transport.
struct DeskSync {
    /// Schedules the desk's transport changes onto this engine's stream.
    follower: TransportFollower,
    /// Whether the desk was connected at the start of the current block.
    connected: bool,
    /// A transport command the desk's request queue had no room for, asked
    /// again every block until it goes through.
    pending: Option<TransportCommand>,
}

/// State bundle for audio processing.
///
/// Owned by the output callback closure. All fields are pre-allocated at
/// engine start and reused throughout the lifetime — no allocations in the
/// audio callback.
pub(super) struct ProcessingState {
    transport: TransportClock,
    mixer: Mixer,
    envelope: EnvelopeFollower,
    latest_pitch: PitchEstimate,
    /// The latest spectrum, copied from the analysis thread's frame.
    spectrum: Vec<f32>,
    /// The latest formants, copied from the analysis thread's data.
    formants: FormantData,
    is_recording: bool,
    /// Running index of samples successfully queued for the disk thread.
    /// Sent with every disk Start/Stop so the disk thread splits takes at
    /// exactly the right sample (see [`super::DiskCommand`]).
    disk_samples_queued: u64,
    sample_rate: u32,
    mic_block: Vec<f32>,
    waveform_snapshot: Vec<f32>,
    /// Samples processed since last meter reset.
    meter_sample_counter: u32,
    /// Number of samples between meter resets (~50ms worth).
    meter_reset_interval: u32,
    /// A display frame taken from the UI's recycle ring and not yet
    /// published.
    frame: Option<DisplayState>,
    /// Monotonically increasing counter for assigning unique clip IDs.
    next_clip_id: u64,
    /// Recording chunks and takes.
    takes: Takes,
    /// The empty parcels tracks arrived in, one per track, for sending
    /// removed tracks out.
    parcels: Vec<Parcel<Track>>,
    /// Empty timeline sources, ready to describe the timeline.
    timelines: Vec<TimelineSource>,
    /// Whether tracks or clips changed since the timeline was last sent.
    timeline_dirty: bool,
    /// Metronome click generator.
    metronome: Metronome,
    /// Set to `true` when a Shutdown command is received or the command
    /// channel disconnects. Once set, `process_block` fills silence.
    shutdown: bool,
    /// Last MIDI note that was pressed (for pitch bend calculation).
    midi_last_note: Option<u8>,
    /// Shared health counters. Every dropped sample, lost message or
    /// rejected request in the callback is counted here (relaxed atomics —
    /// real-time safe).
    stats: Arc<EngineStats>,
    /// Following the kazoo-mix desk's transport.
    desk: DeskSync,
    /// Frames rendered since the last display snapshot was pushed.
    frames_since_display: usize,
    /// Frames between display snapshots (see [`DISPLAY_HZ`]).
    display_interval: usize,
}

impl ProcessingState {
    /// Build the processing state. Allocates everything the callback will
    /// ever use; the spectrum copy holds up to `spectrum_len` bins.
    pub(super) fn new(
        sample_rate: u32,
        buffer_size: usize,
        spectrum_len: usize,
        stats: Arc<EngineStats>,
    ) -> Self {
        let sr_f32 = sample_rate as f32;
        let mut mixer = Mixer::new();
        mixer.prepare(sr_f32, buffer_size);

        // Reset meters every ~50ms (sample_rate / 20 samples).
        let meter_reset_interval = sample_rate / 20;

        Self {
            transport: TransportClock::new(sample_rate),
            mixer,
            envelope: EnvelopeFollower::new(5.0, 50.0, sr_f32),
            latest_pitch: PitchEstimate {
                frequency: None,
                voiced_probability: 0.0,
                midi_note: None,
            },
            spectrum: Vec::with_capacity(spectrum_len),
            formants: FormantData {
                frequencies: Vec::with_capacity(FORMANT_CAPACITY),
                bandwidths: Vec::with_capacity(FORMANT_CAPACITY),
                num_formants: 0,
            },
            is_recording: false,
            disk_samples_queued: 0,
            sample_rate,
            mic_block: vec![0.0; buffer_size],
            waveform_snapshot: Vec::with_capacity(WAVEFORM_POINTS),
            meter_sample_counter: 0,
            meter_reset_interval,
            frame: None,
            next_clip_id: 0,
            takes: Takes {
                pool: (0..MAX_TRACKS * CHUNKS_PER_TRACK)
                    .map(|_| resident_buffer(chunk_len(sample_rate)))
                    .collect(),
                active: Vec::with_capacity(MAX_TRACKS),
                next_take: 0,
            },
            parcels: Vec::with_capacity(MAX_TRACKS),
            timelines: (0..TIMELINE_SOURCES)
                .map(|_| TimelineSource::new())
                .collect(),
            timeline_dirty: false,
            metronome: Metronome::new(sample_rate),
            shutdown: false,
            midi_last_note: None,
            stats,
            desk: DeskSync {
                follower: TransportFollower::new(sample_rate),
                connected: false,
                pending: None,
            },
            frames_since_display: 0,
            display_interval: (sample_rate / DISPLAY_HZ).max(1) as usize,
        }
    }
}

/// The callback's end of the ring to the reclaim thread.
pub(super) struct ReclaimLink {
    outbound: HeapProd<Outbound>,
    /// Items sent while the ring was full, in order, waiting for room.
    overflow: Vec<Outbound>,
    stats: Arc<EngineStats>,
}

impl ReclaimLink {
    /// Allocates the overflow list: not for the audio thread.
    pub(super) fn new(outbound: HeapProd<Outbound>, stats: Arc<EngineStats>) -> Self {
        Self {
            outbound,
            overflow: Vec::with_capacity(OVERFLOW_CAPACITY),
            stats,
        }
    }

    /// How many items commands may send right now: the ring's free room,
    /// or none while earlier items still wait in the overflow list.
    fn room(&self) -> usize {
        if self.overflow.is_empty() {
            self.outbound.vacant_len()
        } else {
            0
        }
    }

    /// Move overflowed items into the ring, oldest first, while it has
    /// room.
    fn flush(&mut self) {
        let free = self.outbound.vacant_len().min(self.overflow.len());
        for item in self.overflow.drain(..free) {
            if let Err(item) = self.outbound.try_push(item) {
                discard(&self.stats, item);
            }
        }
    }

    /// Send `item` to the reclaim thread, in order after anything already
    /// waiting.
    ///
    /// Commands are only taken while the ring has room for what they send,
    /// so the ring is normally never full. Recording chunks and take ends
    /// (which cannot wait for room) fall back to a pre-allocated overflow
    /// list, sent on as soon as the ring drains. Only if that is full too is
    /// the item freed here; that is counted ([`EngineStats`]
    /// `callback_frees`) so it cannot happen unnoticed.
    fn send(&mut self, item: Outbound) {
        self.flush();
        let item = if self.overflow.is_empty() {
            match self.outbound.try_push(item) {
                Ok(()) => return,
                Err(item) => item,
            }
        } else {
            item
        };
        if self.overflow.len() < self.overflow.capacity() {
            self.overflow.push(item);
        } else {
            discard(&self.stats, item);
        }
    }

    /// Send `retired` to the reclaim thread to be freed.
    fn retire(&mut self, retired: Retired) {
        self.send(Outbound::Retired(retired));
    }
}

/// Free `value` on the audio thread, counting it. Only reached if a
/// real-time invariant is broken (see [`ReclaimLink::send`]).
fn discard<T>(stats: &EngineStats, value: T) {
    stats.callback_free();
    drop(value);
}

/// All ring buffer handles and channels used by the audio processing callback.
pub(super) struct ProcessingIO {
    pub(super) mic_cons: HeapCons<f32>,
    pub(super) display_prod: HeapProd<DisplayState>,
    /// Display frames the UI has finished with, to be written again.
    pub(super) display_recycle: HeapCons<DisplayState>,
    pub(super) analysis_prod: HeapProd<f32>,
    pub(super) disk_prod: HeapProd<f32>,
    pub(super) pitch_cons: HeapCons<PitchEstimate>,
    pub(super) spectrum_cons: HeapCons<Vec<f32>>,
    pub(super) formant_cons: HeapCons<Option<FormantData>>,
    pub(super) command_rx: Receiver<EngineCommand>,
    pub(super) disk_cmd_tx: Sender<DiskCommand>,
    /// Objects leaving the callback, to the reclaim thread.
    pub(super) reclaim: ReclaimLink,
    /// Recorded clips, emptied recording chunks and cleared timeline sources back from
    /// the reclaim thread.
    pub(super) inbound: HeapCons<Inbound>,
    /// The audio half of the link to the kazoo-mix desk, when the engine was
    /// asked to plug into it.
    pub(super) desk: Option<HubLinkAudio>,
}

/// Process one audio callback, writing directly to the cpal output buffer.
///
/// Called by the cpal output callback on every buffer cycle. This is the
/// main audio processing entry point — it takes back what the reclaim thread
/// returned, drains commands and desk messages, then renders the buffer in
/// blocks of at most the engine's buffer size (reading mic input, running
/// the mixer, applying the soft limiter, and sending the result to the desk
/// or the speakers), and publishes the timeline and a display snapshot when
/// due.
///
/// # Contract
///
/// - `output_buffer` is interleaved stereo (`[L, R, L, R, ...]`) of any
///   length: callbacks longer than the engine's buffer size are rendered in
///   several blocks, so every frame is always written.
/// - This function MUST fill the entire `output_buffer` every time, even
///   if no mic data is available or after shutdown (fills silence).
/// - No allocations, no frees, no locks, and no panics.
pub(super) fn process_block(
    state: &mut ProcessingState,
    io: &mut ProcessingIO,
    output_buffer: &mut [f32],
) {
    // After shutdown, fill silence and return immediately.
    if state.shutdown {
        output_buffer.fill(0.0);
        return;
    }

    io.reclaim.flush();
    accept_returns(io, state);
    retry_desk_request(io, state);
    // Drain commands — may set state.shutdown.
    drain_commands(io, state);
    if state.shutdown {
        output_buffer.fill(0.0);
        return;
    }
    drain_desk_messages(io, state);

    let block_start = Instant::now();
    let chunk_len = (state.mic_block.len() * 2).max(2);
    let mut input_level_db = Db::from_linear(state.envelope.current()).value();
    let mut total_frames = 0_usize;
    for chunk in output_buffer.chunks_mut(chunk_len) {
        total_frames += chunk.len() / 2;
        input_level_db = render_chunk(state, io, chunk);
    }

    publish_timeline(io, state);

    let cpu_load = compute_cpu_load(block_start, total_frames, state.sample_rate);
    state.frames_since_display = state.frames_since_display.saturating_add(total_frames);
    if state.frames_since_display >= state.display_interval {
        // Keep the overshoot so snapshots average exactly DISPLAY_HZ; a
        // long callback still pushes only one.
        state.frames_since_display =
            (state.frames_since_display - state.display_interval).min(state.display_interval - 1);
        push_display_state(io, state, input_level_db, cpu_load);
    }

    // Only reset meters every ~50ms to preserve meaningful peak-hold.
    let n = u32::try_from(total_frames).unwrap_or(u32::MAX);
    state.meter_sample_counter = state.meter_sample_counter.saturating_add(n);
    if state.meter_sample_counter >= state.meter_reset_interval {
        state.mixer.reset_meters();
        state.meter_sample_counter = 0;
    }
}

/// Render one chunk of at most the engine's buffer size, splitting it on
/// the exact frames where the desk's transport changes fall due. Returns
/// the input level (dB) of the last part rendered.
fn render_chunk(state: &mut ProcessingState, io: &mut ProcessingIO, chunk: &mut [f32]) -> f32 {
    let frames = chunk.len() / 2;
    let mut start = 0;
    if state.desk.connected && !state.desk.follower.is_idle() {
        // Stream frame of this chunk's first frame, as the desk counts them:
        // the link's own count of the frames it has been handed.
        let base = io.desk.as_ref().map_or(0, HubLinkAudio::stream_frame);
        for frame in 0..frames {
            let Some(change) = state.desk.follower.due(base.wrapping_add(frame as u64)) else {
                continue;
            };
            if frame > start {
                render_segment(state, io, &mut chunk[start * 2..frame * 2]);
            }
            apply_desk_transport(state, io, change);
            start = frame;
        }
    }
    render_segment(state, io, &mut chunk[start * 2..])
}

/// Render `output` (interleaved stereo, at most the engine's buffer size)
/// and return the input level in dB.
fn render_segment(state: &mut ProcessingState, io: &mut ProcessingIO, output: &mut [f32]) -> f32 {
    let num_samples = (output.len() / 2).min(state.mic_block.len());

    let num_read = read_mic_input(io, state, num_samples);

    feed_analysis(io, state, num_read);
    drain_analysis_results(io, state);

    let input_level_db = compute_input_level(state, num_read);
    let position_before_advance = state.transport.position_samples();
    let master_slice_len = run_mixer(state, num_samples, position_before_advance);

    feed_disk(io, state, master_slice_len);
    // While the desk plays this engine, it gets the limited master (without
    // the local metronome: the desk keeps its own) and the speakers stay
    // silent, so the engine is not heard twice.
    let routed = send_to_desk(io, state, master_slice_len);
    if !routed {
        mix_metronome_and_limit(
            state,
            num_samples,
            position_before_advance,
            master_slice_len,
        );
    }
    feed_clip_analysis(io, state, num_samples);

    let copy_len = if routed {
        0
    } else {
        let master_buf = state.mixer.master_buffer();
        let len = master_slice_len.min(output.len());
        output[..len].copy_from_slice(&master_buf[..len]);
        len
    };
    // Fill any remainder (all of it while routed) with silence.
    output[copy_len..].fill(0.0);

    // Advance transport BEFORE capturing recordings so that count-in
    // completion starts the takes in time for this block's mic data to be
    // captured (avoids one-block latency at count-in start).
    let recording_before = state.transport.is_recording();
    advance_transport(io, state, num_samples);
    let wrapped = if recording_before {
        loop_wrap(&state.transport, position_before_advance, num_samples)
    } else {
        None
    };
    match wrapped {
        // The loop wrapped inside this block while recording: the part
        // before the loop end closes each take, and the rest starts a new
        // take (one per pass) at the loop start.
        Some((split, loop_start)) if !state.takes.active.is_empty() => {
            capture_track_recordings(io, state, 0, split);
            finalize_track_recordings(io, state);
            start_track_recordings_at(state, loop_start);
            capture_track_recordings(io, state, split, num_samples);
        }
        _ => capture_track_recordings(io, state, 0, num_samples),
    }
    capture_waveform(state, num_read);
    input_level_db
}

/// If advancing `num_samples` from `before` wrapped the transport round its
/// loop, the offset of the loop end within the block and the loop start.
fn loop_wrap(transport: &TransportClock, before: u64, num_samples: usize) -> Option<(usize, u64)> {
    let (start, end) = transport.snapshot().loop_region?;
    let after = transport.position_samples();
    // A wrap is the only way advancing moves the position backwards.
    if after >= before.saturating_add(num_samples as u64) || before >= end || end <= start {
        return None;
    }
    let split = usize::try_from(end - before).unwrap_or(num_samples);
    Some((split.min(num_samples), start))
}

/// Soft-limit the master and hand it to the desk, if the desk is playing
/// this engine. Returns whether it is (the local output must then be silent).
fn send_to_desk(io: &mut ProcessingIO, state: &mut ProcessingState, stereo_len: usize) -> bool {
    let Some(desk) = io.desk.as_mut() else {
        return false;
    };
    if !desk.is_connected() {
        return false;
    }
    let master = state.mixer.master_buffer_mut();
    let len = stereo_len.min(master.len()) / 2 * 2;
    soft_limit_buffer(&mut master[..len]);
    let frames = len / 2;
    // At most the engine's buffer size, which `start` checked fits a u32; a
    // mismatch would be refused by the link and counted as a dropped block.
    let frame_count = u32::try_from(frames).unwrap_or(u32::MAX);
    desk.send_audio(frame_count, &master[..len])
}

/// Take the desk's messages: schedule its transport syncs and play its note
/// events. Resets the transport follower when the desk goes away, so stale
/// changes cannot fire when it comes back.
fn drain_desk_messages(io: &mut ProcessingIO, state: &mut ProcessingState) {
    let Some(desk) = io.desk.as_mut() else {
        return;
    };
    let connected = desk.is_connected();
    if state.desk.connected && !connected {
        state.desk.follower = TransportFollower::new(state.sample_rate);
    }
    state.desk.connected = connected;
    while let Some(message) = desk.try_recv() {
        match message {
            HubMessage::TransportSync(sync) => {
                if state.desk.follower.schedule(&sync).is_err() {
                    state.stats.desk_sync_rejected();
                }
            }
            HubMessage::NoteEvent(event) => match event.event_type {
                NOTE_ON => apply_midi_note_on(state, event.note),
                NOTE_OFF => state.midi_last_note = None,
                // Control changes are not mapped (as for local MIDI), and
                // the desk defines no pitch-bend encoding.
                _ => {}
            },
            // The desk's own mixer parameters are its business, and the
            // link handles the desk shutting down.
            HubMessage::ParameterChange(_) | HubMessage::Shutdown => {}
        }
    }
}

/// Apply a transport change from the desk on the frame it falls due: its
/// tempo, and either play from its song position or stop.
fn apply_desk_transport(
    state: &mut ProcessingState,
    io: &mut ProcessingIO,
    change: TransportChange,
) {
    apply_transport_command(state, io, TransportCommand::SetTempo(change.bpm));
    match change.beat {
        Some(beat) => {
            // The clock's (range-limited) tempo maps beats to samples, the
            // same mapping it uses to show bars and beats.
            let samples_per_beat = f64::from(state.sample_rate) * 60.0 / state.transport.bpm();
            let position = (beat.max(0.0) * samples_per_beat).round();
            let position = if position.is_finite() {
                position as u64
            } else {
                0
            };
            apply_transport_command(state, io, TransportCommand::Seek(position));
            if !state.transport.is_playing() && !state.transport.is_recording() {
                apply_transport_command(state, io, TransportCommand::Play);
            }
        }
        None => {
            if state.transport.is_playing() || state.transport.is_recording() {
                apply_transport_command(state, io, TransportCommand::Stop);
            }
        }
    }
}

/// While plugged into the desk, play, stop, pause and tempo changes are the
/// studio's: ask the desk, which answers every instrument (this engine
/// included) with a transport sync. Returns whether the command was handled
/// that way; `false` means act on it locally (no desk, or a command the
/// desk does not own).
fn ask_desk(io: &mut ProcessingIO, state: &mut ProcessingState, cmd: TransportCommand) -> bool {
    let Some(desk) = io.desk.as_mut() else {
        return false;
    };
    let (wanted, bpm) = match cmd {
        TransportCommand::Play => (TRANSPORT_PLAYING, None),
        TransportCommand::Stop => (TRANSPORT_STOPPED, None),
        TransportCommand::Pause => (TRANSPORT_PAUSED, None),
        // Only the tempo: the desk keeps its own play state, which this
        // engine may not have heard yet. Tempos are tens to hundreds of BPM:
        // f32 holds them exactly enough.
        TransportCommand::SetTempo(bpm) => (TRANSPORT_UNCHANGED, Some(bpm as f32)),
        _ => return false,
    };
    match desk.request_transport(wanted, bpm) {
        Ok(()) => true,
        Err(RequestError::NotConnected) => false,
        Err(RequestError::Full) => {
            // Asked again every block until the desk's queue has room; a
            // newer request supersedes (and counts) one still waiting.
            if state.desk.pending.replace(cmd).is_some() {
                state.stats.desk_request_dropped();
            }
            true
        }
        Err(RequestError::Invalid) => {
            state.stats.desk_request_dropped();
            true
        }
    }
}

/// Ask the desk again for a transport change its queue had no room for. If
/// the desk has gone meanwhile, the change is made locally.
fn retry_desk_request(io: &mut ProcessingIO, state: &mut ProcessingState) {
    let Some(cmd) = state.desk.pending.take() else {
        return;
    };
    if !ask_desk(io, state, cmd) {
        apply_transport_command(state, io, cmd);
    }
}

/// Drain pending commands from the command channel.
///
/// A command is only taken while the outbound ring has room for everything
/// it can retire; otherwise the rest wait in the channel until the reclaim
/// thread catches up (nothing is lost: the channel is bounded, and a full
/// channel is reported to the sender).
///
/// Sets `state.shutdown` to `true` if a Shutdown command is received or the
/// channel disconnects. The caller should check `state.shutdown` after this
/// returns.
fn drain_commands(io: &mut ProcessingIO, state: &mut ProcessingState) {
    while io.reclaim.room() >= MAX_RETIRED_PER_COMMAND {
        match io.command_rx.try_recv() {
            Ok(EngineCommand::Shutdown) | Err(crossbeam_channel::TryRecvError::Disconnected) => {
                state.shutdown = true;
                return;
            }
            Ok(EngineCommand::Transport(cmd)) if ask_desk(io, state, cmd) => {}
            Ok(cmd) => apply_command(cmd, state, io),
            Err(crossbeam_channel::TryRecvError::Empty) => return,
        }
    }
}

/// Take back what the reclaim thread returned: recorded clips (placed on
/// their tracks), emptied recording chunks, and cleared timeline sources.
fn accept_returns(io: &mut ProcessingIO, state: &mut ProcessingState) {
    while io.reclaim.room() >= MAX_RETIRED_PER_RETURN {
        let Some(item) = io.inbound.try_pop() else {
            return;
        };
        match item {
            Inbound::Clip { track_id, clip } => place_recorded_clip(io, state, track_id, clip),
            Inbound::Chunk(chunk) => {
                if state.takes.pool.len() < state.takes.pool.capacity() {
                    state.takes.pool.push(chunk);
                } else {
                    // Every chunk was made with the pool, so it always
                    // has room; one that does not fit is not ours.
                    io.reclaim.retire(Retired::Samples(chunk));
                }
            }
            Inbound::Timeline(source) => {
                if source.is_empty() && state.timelines.len() < state.timelines.capacity() {
                    state.timelines.push(source);
                } else {
                    io.reclaim.retire(Retired::Timeline(source));
                }
            }
        }
    }
}

/// Place a recorded clip on its track.
fn place_recorded_clip(
    io: &mut ProcessingIO,
    state: &mut ProcessingState,
    track_id: TrackId,
    clip: AudioClip,
) {
    // The recording is lost if its track is already full; make that
    // visible instead of dropping it silently.
    let refused = match state.mixer.track_mut(track_id) {
        Some(track) => track.add_clip(clip).err(),
        None => Some(clip),
    };
    match refused {
        None => state.timeline_dirty = true,
        Some(clip) => {
            state.stats.clip_rejected();
            io.reclaim.retire(Retired::Clip(clip));
        }
    }
}

/// Read mic samples from the ring buffer into the state's mic block.
///
/// Always reads the **newest** samples available, discarding any stale
/// data that has accumulated in the ring buffer. This prevents latency
/// from growing when the output callback runs slightly behind the input
/// callback — we always process the most recent audio rather than a
/// backlog of old samples.
///
/// Reads at most `max_read` samples to match the output buffer size. Any
/// remaining slots up to `max_read` are zero-padded (silence). Returns
/// the number of samples actually read from the ring buffer.
fn read_mic_input(io: &mut ProcessingIO, state: &mut ProcessingState, max_read: usize) -> usize {
    let limit = max_read.min(state.mic_block.len());
    let available = io.mic_cons.occupied_len();

    // If more than one block has accumulated, skip stale samples so we
    // always process the newest audio. This is the key latency reduction:
    // without this, accumulated samples add proportional latency.
    if available > limit {
        let excess = available - limit;
        io.mic_cons.skip(excess);
    }

    let num_read = io.mic_cons.pop_slice(&mut state.mic_block[..limit]);
    for sample in &mut state.mic_block[num_read..limit] {
        *sample = 0.0;
    }
    sanitize_buffer(&mut state.mic_block[..num_read]);
    num_read
}

/// Feed raw mic samples to the analysis thread's ring buffer.
///
/// Called before the mixer runs to feed mic audio for pitch detection during
/// recording and monitoring. During playback, [`feed_clip_analysis`] is
/// called after the mixer to feed clip audio instead.
fn feed_analysis(io: &mut ProcessingIO, state: &ProcessingState, num_read: usize) {
    // During playback (not recording), skip mic — clip audio will be fed after the mixer.
    if state.transport.is_playing() && !state.transport.is_recording() {
        return;
    }
    if num_read > 0 {
        let pushed = io.analysis_prod.push_slice(&state.mic_block[..num_read]);
        state.stats.record_analysis_push(num_read, pushed);
    }
}

/// Feed mixed clip audio to the analysis thread during playback.
///
/// Called after `run_mixer` so the `clip_mix_buffer` is populated.
/// This enables pitch detection on clip content so synths receive the correct
/// frequency data when processing clips.
fn feed_clip_analysis(io: &mut ProcessingIO, state: &ProcessingState, num_samples: usize) {
    if state.transport.is_playing() && !state.transport.is_recording() {
        let clip_buf = state.mixer.clip_mix_buffer();
        let len = num_samples.min(clip_buf.len());
        if len > 0 {
            let pushed = io.analysis_prod.push_slice(&clip_buf[..len]);
            state.stats.record_analysis_push(len, pushed);
        }
    }
}

/// Replace `dst`'s contents with as much of `src` as fits in its capacity.
/// Never allocates: display buffers are sized when the engine starts.
fn copy_within_capacity<T: Copy>(dst: &mut Vec<T>, src: &[T]) {
    dst.clear();
    let len = src.len().min(dst.capacity());
    dst.extend_from_slice(&src[..len]);
}

/// Copy formant data within `dst`'s capacity. Never allocates.
fn copy_formants(dst: &mut FormantData, src: &FormantData) {
    copy_within_capacity(&mut dst.frequencies, &src.frequencies);
    copy_within_capacity(&mut dst.bandwidths, &src.bandwidths);
    dst.num_formants = src
        .num_formants
        .min(dst.frequencies.len())
        .min(dst.bandwidths.len());
}

/// Drain analysis results (pitch, spectrum, formants) from ring buffers
/// and feed detected pitch to armed tracks' synths.
///
/// Spectrum and formant results arrive in buffers the analysis thread
/// allocated: they are copied into pre-allocated state and the buffers sent
/// to the reclaim thread to be freed. A result is only taken while there is
/// room to send its buffer on; otherwise it waits (and the analysis thread
/// counts any result it then cannot queue).
fn drain_analysis_results(io: &mut ProcessingIO, state: &mut ProcessingState) {
    while let Some(pitch) = io.pitch_cons.try_pop() {
        state.latest_pitch = pitch;
    }
    while io.reclaim.room() > 0 {
        let Some(spectrum) = io.spectrum_cons.try_pop() else {
            break;
        };
        copy_within_capacity(&mut state.spectrum, &spectrum);
        io.reclaim.retire(Retired::Samples(spectrum));
    }
    while io.reclaim.room() > 0 {
        let Some(formants) = io.formant_cons.try_pop() else {
            break;
        };
        if let Some(data) = formants {
            copy_formants(&mut state.formants, &data);
            io.reclaim.retire(Retired::Formants(data));
        } else {
            state.formants.frequencies.clear();
            state.formants.bandwidths.clear();
            state.formants.num_formants = 0;
        }
    }

    // Feed detected pitch to all synth layers on tracks.
    // During playback: all tracks need pitch (synths process clip audio).
    // During recording/monitoring: only armed tracks need pitch.
    if let Some(freq) = state.latest_pitch.frequency {
        let playback_mode = state.transport.is_playing() && !state.transport.is_recording();
        for track in state.mixer.tracks_mut() {
            if playback_mode || track.is_armed() {
                for layer in track.layers_mut() {
                    layer.synth_mut().set_pitch(freq);
                }
            }
        }
    }
}

/// Compute the input signal level in dB via the envelope follower.
fn compute_input_level(state: &mut ProcessingState, num_read: usize) -> f32 {
    let linear = if num_read > 0 {
        state.envelope.process_block(&state.mic_block[..num_read])
    } else {
        state.envelope.current()
    };
    Db::from_linear(linear).value()
}

/// Advance the transport clock by the given number of samples.
///
/// After advancing, checks the returned [`AdvanceFlags`] for count-in
/// completion and auto-stop triggers, starting or finalizing per-track
/// recordings accordingly.
fn advance_transport(io: &mut ProcessingIO, state: &mut ProcessingState, num_samples: usize) {
    let n = u32::try_from(num_samples).unwrap_or(u32::MAX);
    let flags = state.transport.advance(n);

    if flags.count_in_completed {
        // Count-in finished — start recording on all armed tracks and
        // transition the transport from Playing to Recording. Use the
        // exact bar-boundary position from AdvanceFlags rather than the
        // current (overshot) transport position for bar-aligned clips.
        start_track_recordings_at(state, flags.record_start_position);
        state.transport.apply_command(TransportCommand::Record);
    }

    if flags.auto_stop_triggered {
        // Auto-stop boundary reached — finalize recordings and stop.
        finalize_track_recordings(io, state);
        state.transport.apply_command(TransportCommand::Stop);
        state.metronome.reset();
    }
}

/// Run the mixer to produce a stereo master buffer from all tracks.
///
/// Returns the number of interleaved stereo samples written. The master
/// buffer is ready for disk recording at this point (no metronome mixed in).
fn run_mixer(state: &mut ProcessingState, num_samples: usize, position: u64) -> usize {
    state.mixer.process(
        &state.mic_block[..num_samples],
        num_samples,
        position,
        state.transport.is_playing() || state.transport.is_recording(),
        state.transport.is_recording(),
    );

    (num_samples * 2).min(state.mixer.master_buffer().len())
}

/// Mix metronome clicks into the master buffer and apply the soft limiter.
///
/// The metronome is mixed AFTER the disk recorder has already captured the
/// clean master buffer, so clicks go to speakers but NOT to disk recordings.
/// A sanitization pass runs after the metronome to guard against NaN/Inf.
///
/// The caller is responsible for copying `master_buffer[..stereo_len]` to
/// the output buffer after this returns.
fn mix_metronome_and_limit(
    state: &mut ProcessingState,
    num_samples: usize,
    position: u64,
    stereo_len: usize,
) {
    if state.transport.metronome_enabled()
        && (state.transport.is_playing() || state.transport.is_recording())
    {
        let master_buf = state.mixer.master_buffer_mut();
        state.metronome.generate(
            &mut master_buf[..stereo_len],
            position,
            state.transport.bpm(),
            state.transport.beats_per_bar(),
            num_samples,
        );
        // Sanitize after metronome mixing to uphold NaN/Inf defense.
        sanitize_buffer(&mut master_buf[..stereo_len]);
    }

    // Apply soft limiter — last processing step before audio reaches the DAC.
    // Without limiting, multi-track summing and master volume (up to +24 dB)
    // can produce samples well above 1.0 which get hard-clipped by the
    // hardware, producing harsh "bit-crushed" distortion.
    let master_buf = state.mixer.master_buffer_mut();
    soft_limit_buffer(&mut master_buf[..stereo_len]);
}

/// Feed interleaved stereo output to the disk recorder ring buffer.
fn feed_disk(io: &mut ProcessingIO, state: &mut ProcessingState, stereo_len: usize) {
    if state.is_recording {
        let master_buf = state.mixer.master_buffer();
        let len = stereo_len.min(master_buf.len());
        let pushed = io.disk_prod.push_slice(&master_buf[..len]);
        state.stats.record_disk_push(len, pushed);
        state.disk_samples_queued = state
            .disk_samples_queued
            .saturating_add(u64::try_from(pushed).unwrap_or(u64::MAX));
    }
}

/// Capture a downsampled waveform snapshot for the oscilloscope display.
fn capture_waveform(state: &mut ProcessingState, num_read: usize) {
    // Only update when we actually received new mic samples. Keeping the
    // previous snapshot avoids blinking on frames where the ring buffer
    // had nothing new (common at higher UI refresh rates).
    if num_read == 0 {
        return;
    }
    state.waveform_snapshot.clear();
    let max_len = state.waveform_snapshot.capacity().min(WAVEFORM_POINTS);
    let step = (num_read / max_len.max(1)).max(1);
    let mut i = 0;
    while i < num_read && state.waveform_snapshot.len() < max_len {
        state.waveform_snapshot.push(state.mic_block[i]);
        i += step;
    }
}

/// Estimate CPU load as the ratio of processing time to audio buffer duration.
fn compute_cpu_load(block_start: Instant, num_samples: usize, sample_rate: u32) -> f32 {
    let elapsed = block_start.elapsed();
    let n = u32::try_from(num_samples).unwrap_or(u32::MAX);
    let budget = std::time::Duration::from_secs_f64(f64::from(n) / f64::from(sample_rate.max(1)));
    if budget.as_nanos() == 0 {
        return 0.0;
    }
    let ratio = elapsed.as_nanos() as f64 / budget.as_nanos() as f64;
    let load = ratio as f32;
    if load.is_finite() {
        load.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Write a display snapshot into a frame the UI handed back, and push it.
///
/// Frames circulate between the callback and the UI with their buffers
/// sized at engine start, so nothing is allocated or freed. With no frame
/// available (the UI is holding them all) or the display ring full, the
/// snapshot is skipped and counted.
fn push_display_state(
    io: &mut ProcessingIO,
    state: &mut ProcessingState,
    input_level_db: f32,
    cpu_load: f32,
) {
    if io.display_prod.is_full() {
        state.stats.display_frame_dropped();
        return;
    }
    if state.frame.is_none() {
        state.frame = io.display_recycle.try_pop();
    }
    let Some(mut frame) = state.frame.take() else {
        state.stats.display_frame_dropped();
        return;
    };
    write_display_frame(&mut frame, state, input_level_db, cpu_load);
    if let Err(frame) = io.display_prod.try_push(frame) {
        state.frame = Some(frame);
        state.stats.display_frame_dropped();
    }
}

/// Fill `frame` with the current engine state, within its buffers'
/// capacity. The frame's timeline is the UI's business and is not touched.
fn write_display_frame(
    frame: &mut DisplayState,
    state: &ProcessingState,
    input_level_db: f32,
    cpu_load: f32,
) {
    frame.transport = state.transport.snapshot();
    state.mixer.write_snapshot(&mut frame.mixer);
    frame.pitch = state.latest_pitch;
    copy_within_capacity(&mut frame.spectrum_magnitudes, &state.spectrum);
    copy_within_capacity(&mut frame.waveform, &state.waveform_snapshot);
    frame.input_level_db = input_level_db;
    frame.is_recording = state.is_recording;
    copy_formants(&mut frame.formants, &state.formants);
    frame.cpu_load = cpu_load;
    frame.recordings = [None; MAX_TRACKS];
    for (slot, take) in frame.recordings.iter_mut().zip(&state.takes.active) {
        *slot = Some(TrackRecording {
            track_id: take.track_id.0,
            span: RecordingSpan {
                start: take.start_position,
                length: take.recorded as u64,
            },
        });
    }
}

/// When tracks or clips have changed, describe the timeline in an empty
/// source and send it to the reclaim thread, which builds the snapshot the
/// UI draws. Waits (staying dirty) while no source is free.
fn publish_timeline(io: &mut ProcessingIO, state: &mut ProcessingState) {
    if !state.timeline_dirty || io.reclaim.room() == 0 {
        return;
    }
    let Some(mut source) = state.timelines.pop() else {
        return;
    };
    // Sources come back empty, so this fills; a source that somehow is not
    // empty is sent out regardless, to be cleared and returned.
    if source.fill(&state.mixer) {
        state.timeline_dirty = false;
    }
    io.reclaim.send(Outbound::Timeline(source));
}

// ---------------------------------------------------------------------------
// Command application (split into sub-functions for line count compliance)
// ---------------------------------------------------------------------------

/// Apply a single engine command to the processing state.
fn apply_command(cmd: EngineCommand, state: &mut ProcessingState, io: &mut ProcessingIO) {
    match cmd {
        // Shutdown is handled in drain_commands; MidiCC mapping is Phase 2.
        EngineCommand::Shutdown | EngineCommand::MidiCC { .. } => {}
        EngineCommand::Transport(c) => apply_transport_command(state, io, c),
        EngineCommand::AddTrack { track } => apply_add_track(state, io, track),
        EngineCommand::RemoveTrack(id) => apply_remove_track(state, io, id),
        EngineCommand::SetTrackVolume(id, db) => with_track(state, id, |t| t.set_volume(db)),
        EngineCommand::SetTrackPan(id, pan) => with_track(state, id, |t| t.set_pan(pan)),
        EngineCommand::SetTrackMute(id, m) => with_track(state, id, |t| t.set_muted(m)),
        EngineCommand::SetTrackSolo(id, s) => with_track(state, id, |t| t.set_soloed(s)),
        EngineCommand::SetTrackArm(id, a) => apply_set_arm(state, io, id, a),
        EngineCommand::SetTrackSynthesisMode {
            track_id,
            synth,
            mode,
        } => apply_set_synth_mode(state, io, track_id, synth, mode),
        EngineCommand::AddEffect { track_id, effect } => {
            apply_add_effect(state, io, track_id, effect);
        }
        EngineCommand::RemoveEffect {
            track_id,
            effect_index,
        } => {
            let removed = state
                .mixer
                .track_mut(track_id)
                .and_then(|t| t.effects_mut().remove(effect_index));
            if let Some(effect) = removed {
                io.reclaim.retire(Retired::Processor(effect));
            }
        }
        EngineCommand::SetEffectBypass {
            track_id,
            effect_index,
            bypassed,
        } => with_track(state, track_id, |t| {
            t.effects_mut().set_bypass(effect_index, bypassed);
        }),
        EngineCommand::SetEffectParameter {
            track_id,
            effect_index,
            param_index,
            value,
        } => apply_set_effect_param(state, track_id, effect_index, param_index, value),
        EngineCommand::SetSynthParameter {
            track_id,
            param_index,
            value,
        } => apply_set_synth_param(state, track_id, param_index, value),
        EngineCommand::AddSynthLayer { track_id, layer } => {
            apply_add_synth_layer(state, io, track_id, layer);
        }
        EngineCommand::RemoveSynthLayer {
            track_id,
            layer_index,
        } => {
            let removed = state
                .mixer
                .track_mut(track_id)
                .and_then(|t| t.remove_layer(layer_index));
            if let Some(layer) = removed {
                io.reclaim.retire(Retired::Layer(layer));
            }
        }
        cmd @ (EngineCommand::SetSynthLayerGain { .. }
        | EngineCommand::SetSynthLayerEnabled { .. }
        | EngineCommand::SetSynthLayerParameter { .. }) => apply_layer_command(&cmd, state),
        EngineCommand::SetMasterVolume(db) => state.mixer.set_master_volume(db),
        EngineCommand::StartRecording { path } => apply_start_recording(state, io, path),
        EngineCommand::StopRecording => apply_stop_recording(state, io),
        cmd @ (EngineCommand::AddClip { .. }
        | EngineCommand::RemoveClip { .. }
        | EngineCommand::MoveClip { .. }
        | EngineCommand::TrimClipStart { .. }
        | EngineCommand::TrimClipEnd { .. }
        | EngineCommand::SplitClip { .. }
        | EngineCommand::SetClipGain { .. }
        | EngineCommand::SetClipMute { .. }
        | EngineCommand::DuplicateClip { .. }) => apply_clip_command(cmd, state, io),

        // -- MIDI input --------------------------------------------------------
        EngineCommand::MidiNoteOn { note, .. } => apply_midi_note_on(state, note),
        EngineCommand::MidiNoteOff { .. } => state.midi_last_note = None,
        EngineCommand::MidiPitchBend { value, .. } => apply_midi_pitch_bend(state, value),
    }
}

/// Apply a synth-layer gain, enable or parameter command.
fn apply_layer_command(cmd: &EngineCommand, state: &mut ProcessingState) {
    match *cmd {
        EngineCommand::SetSynthLayerGain {
            track_id,
            layer_index,
            gain,
        } => with_synth_layer(state, track_id, layer_index, |layer| layer.set_gain(gain)),
        EngineCommand::SetSynthLayerEnabled {
            track_id,
            layer_index,
            enabled,
        } => with_synth_layer(state, track_id, layer_index, |layer| {
            layer.set_enabled(enabled);
        }),
        EngineCommand::SetSynthLayerParameter {
            track_id,
            layer_index,
            param_index,
            value,
        } => apply_set_synth_layer_param(state, track_id, layer_index, param_index, value),
        // Other commands are never passed to this function; none of them
        // carries anything that would need retiring.
        _ => {}
    }
}

/// Run `f` on the given track, if it exists.
fn with_track(state: &mut ProcessingState, track_id: TrackId, f: impl FnOnce(&mut Track)) {
    if let Some(track) = state.mixer.track_mut(track_id) {
        f(track);
    }
}

/// Add a track built off the audio thread, keeping its parcel (for sending
/// the track out again when it is removed). A track the mixer refuses (it
/// is full, has a track with the same id, or the track was built for
/// another sample rate or a smaller block size) is counted and sent back
/// out.
fn apply_add_track(state: &mut ProcessingState, io: &mut ProcessingIO, mut parcel: Parcel<Track>) {
    let placed = state.parcels.len() < state.parcels.capacity()
        && state.mixer.insert_track(parcel.slot_mut()).is_some();
    if placed {
        state.parcels.push(parcel);
        state.timeline_dirty = true;
    } else {
        if !parcel.is_empty() {
            state.stats.track_rejected();
        }
        io.reclaim.retire(Retired::Track(parcel));
    }
}

/// Remove a track, sending it out in one of the parcels tracks arrived in.
/// A take it was recording is abandoned: its audio is discarded by the
/// reclaim thread, which returns the chunk.
fn apply_remove_track(state: &mut ProcessingState, io: &mut ProcessingIO, id: TrackId) {
    let Some(track) = state.mixer.remove_track(id) else {
        return;
    };
    state.timeline_dirty = true;
    if let Some(index) = state.takes.active.iter().position(|t| t.track_id == id) {
        let take = state.takes.active.swap_remove(index);
        io.reclaim.send(Outbound::TakeAbort {
            take: take.take,
            chunk: take.chunk,
        });
    }
    // Every track arrived in a parcel, so there is always an empty one.
    match state.parcels.pop() {
        Some(mut parcel) => {
            if let Some(displaced) = parcel.refill(track) {
                discard(&state.stats, displaced);
            }
            io.reclaim.retire(Retired::Track(parcel));
        }
        None => discard(&state.stats, track),
    }
}

/// Arm or disarm a track. While recording, disarming ends the track's take
/// and arming starts one where the transport is.
fn apply_set_arm(state: &mut ProcessingState, io: &mut ProcessingIO, id: TrackId, armed: bool) {
    let Some(track) = state.mixer.track_mut(id) else {
        return;
    };
    track.set_armed(armed);
    if !armed {
        finish_track_take(io, state, id);
    } else if state.transport.is_recording() {
        let position = state.transport.position_samples();
        let recording = state.takes.active.iter().any(|take| take.track_id == id);
        if !recording {
            start_take(&mut state.takes, &state.stats, id, position);
        }
    }
}

/// Start a disk recording. The disk ring is only fed once the disk thread
/// has actually been told to open a file; otherwise every recorded sample
/// would be discarded on the other side. An undelivered command (holding
/// the file path) is retired, not dropped here.
fn apply_start_recording(
    state: &mut ProcessingState,
    io: &mut ProcessingIO,
    path: std::path::PathBuf,
) {
    let start = DiskCommand::Start {
        path,
        from_sample: state.disk_samples_queued,
    };
    match io.disk_cmd_tx.try_send(start) {
        Ok(()) => state.is_recording = true,
        Err(err) => {
            state.stats.disk_command_dropped();
            io.reclaim.retire(Retired::Disk(err.into_inner()));
        }
    }
}

/// Stop a disk recording. Feeding stops regardless; if the Stop cannot be
/// delivered, the disk thread finalizes the file on the next Start or at
/// shutdown. (A Stop holds nothing on the heap.)
fn apply_stop_recording(state: &mut ProcessingState, io: &ProcessingIO) {
    let stop = DiskCommand::Stop {
        at_sample: state.disk_samples_queued,
    };
    if io.disk_cmd_tx.try_send(stop).is_err() {
        state.stats.disk_command_dropped();
    }
    state.is_recording = false;
}

/// Run `f` on the given synth layer, if both the track and layer exist.
fn with_synth_layer(
    state: &mut ProcessingState,
    track_id: TrackId,
    layer_index: usize,
    f: impl FnOnce(&mut SynthLayer),
) {
    if let Some(layer) = state
        .mixer
        .track_mut(track_id)
        .and_then(|t| t.layer_mut(layer_index))
    {
        f(layer);
    }
}

/// Route a MIDI note-on to every armed track: MIDI drives synth layers just
/// like the pitch detector does, but with explicit note events.
///
/// Velocity is not applied: the [`crate::Processor`] interface has no
/// velocity/amplitude parameter, and parameter index 0 means something
/// different for every synth (oscillator shape, wavetable frequency, grain
/// size, ...), so it must not be overwritten with a velocity value.
fn apply_midi_note_on(state: &mut ProcessingState, note: u8) {
    let freq = crate::midi_note_to_frequency(note);
    for track in state.mixer.tracks_mut() {
        if track.is_armed() {
            for layer in track.layers_mut() {
                layer.synth_mut().set_pitch(freq);
            }
        }
    }
    state.midi_last_note = Some(note);
}

/// Apply MIDI pitch bend (8192 = centre, range +/- 2 semitones) relative to
/// the last pressed note on every armed track.
fn apply_midi_pitch_bend(state: &mut ProcessingState, value: u16) {
    let bend_semitones = (f32::from(value) - 8192.0) / 8192.0 * 2.0;
    if let Some(base_note) = state.midi_last_note {
        let base_freq = crate::midi_note_to_frequency(base_note);
        let bent_freq = base_freq * (bend_semitones / 12.0).exp2();
        for track in state.mixer.tracks_mut() {
            if track.is_armed() {
                for layer in track.layers_mut() {
                    layer.synth_mut().set_pitch(bent_freq);
                }
            }
        }
    }
}

/// Apply a clip-related engine command to the processing state.
///
/// Factored out of [`apply_command`] to keep each function within clippy's
/// line-count limit.
fn apply_clip_command(cmd: EngineCommand, state: &mut ProcessingState, io: &mut ProcessingIO) {
    match cmd {
        EngineCommand::AddClip {
            track_id,
            clip_data,
            position,
        } => apply_add_clip(state, io, track_id, clip_data, position),
        EngineCommand::RemoveClip { track_id, clip_id } => {
            let removed = state
                .mixer
                .track_mut(track_id)
                .and_then(|t| t.remove_clip(clip_id));
            if let Some(clip) = removed {
                state.timeline_dirty = true;
                io.reclaim.retire(Retired::Clip(clip));
            }
        }
        EngineCommand::MoveClip {
            track_id,
            clip_id,
            new_position,
        } => with_clip(state, track_id, clip_id, |clip| {
            clip.set_position(new_position);
        }),
        EngineCommand::TrimClipStart {
            track_id,
            clip_id,
            samples,
        } => with_clip(state, track_id, clip_id, |clip| clip.trim_start(samples)),
        EngineCommand::TrimClipEnd {
            track_id,
            clip_id,
            samples,
        } => with_clip(state, track_id, clip_id, |clip| clip.trim_end(samples)),
        EngineCommand::SplitClip {
            track_id,
            clip_id,
            split_position,
        } => apply_split_clip(state, io, track_id, clip_id, split_position),
        EngineCommand::SetClipGain {
            track_id,
            clip_id,
            gain,
        } => with_clip(state, track_id, clip_id, |clip| clip.set_gain(gain)),
        EngineCommand::SetClipMute {
            track_id,
            clip_id,
            muted,
        } => with_clip(state, track_id, clip_id, |clip| clip.set_muted(muted)),
        EngineCommand::DuplicateClip {
            track_id,
            clip_id,
            new_position,
        } => apply_duplicate_clip(state, io, track_id, clip_id, new_position),
        // Other commands are never passed to this function; none of them
        // carries anything that would need retiring.
        _ => {}
    }
}

/// Run `f` on the given clip, if the track and clip exist, marking the
/// timeline changed.
fn with_clip(
    state: &mut ProcessingState,
    track_id: TrackId,
    clip_id: ClipId,
    f: impl FnOnce(&mut AudioClip),
) {
    if let Some(clip) = state
        .mixer
        .track_mut(track_id)
        .and_then(|t| t.find_clip_mut(clip_id))
    {
        f(clip);
        state.timeline_dirty = true;
    }
}

/// Handle a transport command, starting or finalizing per-track recordings
/// when the transport enters or leaves Record state.
fn apply_transport_command(
    state: &mut ProcessingState,
    io: &mut ProcessingIO,
    cmd: TransportCommand,
) {
    match cmd {
        TransportCommand::Record => {
            // Start per-track recording for each armed track.
            start_track_recordings(state);
            state.transport.apply_command(cmd);
        }
        TransportCommand::RecordWithCountIn => {
            apply_record_with_count_in(state);
        }
        TransportCommand::Stop | TransportCommand::Pause => {
            // Finalize any active track recordings before changing state.
            finalize_track_recordings(io, state);
            state.transport.apply_command(cmd);
            // Reset metronome click state on stop so it doesn't
            // continue mid-click when playback resumes.
            if matches!(cmd, TransportCommand::Stop) {
                state.metronome.reset();
            }
        }
        TransportCommand::SetMetronomeVolume(db) => {
            state.metronome.set_volume(db);
        }
        TransportCommand::Seek(_) if !state.takes.active.is_empty() => {
            // A jump while recording ends each take where it was and starts
            // a new one where the transport lands.
            finalize_track_recordings(io, state);
            state.transport.apply_command(cmd);
            start_track_recordings(state);
        }
        other => state.transport.apply_command(other),
    }
}

/// Apply the `RecordWithCountIn` command based on the configured workflow.
///
/// Depending on the [`RecordingWorkflow`], this either:
/// - `FreeRecord`: starts recording immediately (same as `Record`)
/// - `CountIn`: begins a count-in (transport plays with metronome, then
///   recording starts automatically when `advance()` signals completion)
/// - `FixedLength`: starts recording immediately with auto-stop
fn apply_record_with_count_in(state: &mut ProcessingState) {
    use crate::transport::RecordingWorkflow;

    match state.transport.recording_workflow() {
        RecordingWorkflow::FreeRecord => {
            // Behaves identically to a normal Record command.
            start_track_recordings(state);
            state.transport.apply_command(TransportCommand::Record);
        }
        RecordingWorkflow::CountIn {
            count_in_bars,
            record_bars,
        } => {
            // Start playing with count-in. The metronome will sound.
            // When advance() signals count_in_completed, we start
            // track recordings and transition to Recording.
            state.transport.start_count_in(count_in_bars, record_bars);
        }
        RecordingWorkflow::FixedLength { bars } => {
            // Start recording immediately with auto-stop.
            start_track_recordings(state);
            state.transport.start_fixed_length(bars);
        }
    }
}

/// Begin recording on all armed tracks using the current transport position.
fn start_track_recordings(state: &mut ProcessingState) {
    let position = state.transport.position_samples();
    start_track_recordings_at(state, position);
}

/// Begin recording on every armed track not already recording, at the
/// given timeline position.
///
/// Used by the count-in workflow to place clips at the exact bar boundary
/// (the count-in end) rather than the current (overshot) transport position.
fn start_track_recordings_at(state: &mut ProcessingState, position: u64) {
    let takes = &mut state.takes;
    for track in state.mixer.tracks() {
        let recording = takes.active.iter().any(|take| take.track_id == track.id());
        if track.is_armed() && !recording {
            start_take(takes, &state.stats, track.id(), position);
        }
    }
}

/// Start a take on `track_id` into a chunk from the pool. With no chunk
/// left (the reclaim thread has fallen far behind), the track does not
/// record and that is counted (`takes_unavailable`).
fn start_take(takes: &mut Takes, stats: &EngineStats, track_id: TrackId, position: u64) {
    if takes.active.len() == takes.active.capacity() {
        stats.take_unavailable();
        return;
    }
    let Some(chunk) = takes.pool.pop() else {
        stats.take_unavailable();
        return;
    };
    let take = takes.next_take;
    takes.next_take = takes.next_take.wrapping_add(1);
    takes.active.push(ActiveTake {
        take,
        chunk,
        filled: 0,
        recorded: 0,
        start_position: position,
        track_id,
    });
}

/// Finish every active take (see [`finish_take`]).
fn finalize_track_recordings(io: &mut ProcessingIO, state: &mut ProcessingState) {
    while let Some(take) = state.takes.active.pop() {
        finish_take(io, state, take);
    }
}

/// Finish the take recording on `track_id`, if there is one.
fn finish_track_take(io: &mut ProcessingIO, state: &mut ProcessingState, track_id: TrackId) {
    let Some(index) = state
        .takes
        .active
        .iter()
        .position(|take| take.track_id == track_id)
    else {
        return;
    };
    let take = state.takes.active.swap_remove(index);
    finish_take(io, state, take);
}

/// Finish a take. One with audio reserves its clip id and its last chunk
/// leaves for the reclaim thread, which returns the take as a clip (see
/// [`place_recorded_clip`]); an empty take's chunk goes straight back to
/// the pool.
fn finish_take(io: &mut ProcessingIO, state: &mut ProcessingState, take: ActiveTake) {
    use crate::transport::RecordingWorkflow;

    if take.recorded == 0 {
        // The chunk came from the pool, which has room for every chunk.
        state.takes.pool.push(take.chunk);
        return;
    }
    let quantize = matches!(
        state.transport.recording_workflow(),
        RecordingWorkflow::FreeRecord
    );
    let (position, len) = clip_placement(&state.transport, &take, quantize);
    let clip_id = ClipId(state.next_clip_id);
    state.next_clip_id += 1;
    io.reclaim.send(Outbound::TakeEnd(FinishedTake {
        take: take.take,
        track_id: take.track_id,
        clip_id,
        position,
        chunk: take.chunk,
        filled: take.filled,
        len,
        sample_rate: state.sample_rate,
    }));
}

/// Where a finished take's clip goes on the timeline, and how long it is.
///
/// For `FreeRecord`, quantize clip boundaries to the nearest bar so
/// recordings align with the tempo grid. Only quantize when the recording
/// spans at least one full bar — very short recordings (e.g. quick test
/// punches) keep their raw boundaries.
fn clip_placement(transport: &TransportClock, take: &ActiveTake, quantize: bool) -> (u64, usize) {
    let spb = transport.samples_per_bar();
    let spb_usize = spb as usize;
    if quantize && spb > 0 && take.recorded >= spb_usize {
        let quantized_start = transport.quantize_to_bar(take.start_position);
        let raw_end = take.start_position.saturating_add(take.recorded as u64);
        let quantized_end = transport.quantize_to_bar(raw_end);
        // Ensure quantized end is at least one bar past the start.
        let quantized_end = quantized_end.max(quantized_start.saturating_add(spb));
        let q_len = quantized_end.saturating_sub(quantized_start);
        let final_len = (q_len as usize).min(take.recorded);
        (quantized_start, final_len.max(1))
    } else {
        (take.start_position, take.recorded)
    }
}

/// Capture raw mic input `mic_block[from..to]` into every take in progress.
///
/// Records the unprocessed microphone signal so clips contain the user's
/// actual voice rather than synthesized output. This avoids feedback loops
/// and produces clean recordings suitable for later playback through the
/// track's effect chain.
///
/// A full chunk is sent to the reclaim thread and replaced from the pool;
/// with no chunk left, the rest of the block is lost and counted
/// (`take_samples_dropped`). Takes have no length limit.
fn capture_track_recordings(
    io: &mut ProcessingIO,
    state: &mut ProcessingState,
    from: usize,
    to: usize,
) {
    let to = to.min(state.mic_block.len());
    for index in 0..state.takes.active.len() {
        let take = &mut state.takes.active[index];
        let mut offset = from;
        while offset < to {
            if take.filled == take.chunk.len() {
                let Some(next) = state.takes.pool.pop() else {
                    state.stats.take_samples_dropped(to - offset);
                    break;
                };
                let full = std::mem::replace(&mut take.chunk, next);
                let filled = std::mem::replace(&mut take.filled, 0);
                io.reclaim.send(Outbound::TakeChunk {
                    take: take.take,
                    chunk: full,
                    filled,
                });
                continue;
            }
            let count = (to - offset).min(take.chunk.len() - take.filled);
            take.chunk[take.filled..take.filled + count]
                .copy_from_slice(&state.mic_block[offset..offset + count]);
            take.filled += count;
            take.recorded += count;
            offset += count;
        }
    }
}

/// Swap a track's primary synth for one built off the audio thread,
/// retiring the old synth (or the new one, if the track is gone).
fn apply_set_synth_mode(
    state: &mut ProcessingState,
    io: &mut ProcessingIO,
    track_id: TrackId,
    synth: Prepared,
    mode: SynthesisMode,
) {
    let retired = match state.mixer.track_mut(track_id) {
        Some(track) => match track.replace_synth(synth, mode) {
            Ok(old) => old,
            Err(refused) => {
                state.stats.processor_rejected();
                refused.into_processor()
            }
        },
        None => synth.into_processor(),
    };
    io.reclaim.retire(Retired::Processor(retired));
}

/// Append a prepared effect. An effect with no track to go to, prepared
/// for the wrong rate or block size, or with no room in the chain, is
/// retired (and counted unless the track is gone).
fn apply_add_effect(
    state: &mut ProcessingState,
    io: &mut ProcessingIO,
    track_id: TrackId,
    effect: Prepared,
) {
    let refused = match state.mixer.track_mut(track_id) {
        Some(track) => {
            let refused = track.add_effect(effect).err();
            if refused.is_some() {
                state.stats.processor_rejected();
            }
            refused
        }
        None => Some(effect.into_processor()),
    };
    if let Some(effect) = refused {
        io.reclaim.retire(Retired::Processor(effect));
    }
}

/// Add a synth layer built off the audio thread. A layer with no track to
/// go to, prepared for the wrong rate or block size, or with no room on
/// the track, is retired (and counted unless the track is gone).
fn apply_add_synth_layer(
    state: &mut ProcessingState,
    io: &mut ProcessingIO,
    track_id: TrackId,
    layer: SynthLayer,
) {
    let refused = match state.mixer.track_mut(track_id) {
        Some(track) => {
            let refused = track.add_layer(layer).err();
            if refused.is_some() {
                state.stats.processor_rejected();
            }
            refused
        }
        None => Some(layer),
    };
    if let Some(layer) = refused {
        io.reclaim.retire(Retired::Layer(layer));
    }
}

fn apply_set_synth_layer_param(
    state: &mut ProcessingState,
    track_id: TrackId,
    layer_index: usize,
    param_index: usize,
    value: f32,
) {
    let applied = state
        .mixer
        .track_mut(track_id)
        .and_then(|track| track.layer_mut(layer_index))
        .is_some_and(|layer| layer.synth_mut().set_param(param_index, value).is_ok());
    if !applied {
        // No such track or layer, or the synth refused the value.
        state.stats.param_rejected();
    }
}

fn apply_set_synth_param(
    state: &mut ProcessingState,
    track_id: TrackId,
    param_index: usize,
    value: f32,
) {
    let applied = state
        .mixer
        .track_mut(track_id)
        .is_some_and(|track| track.synth_mut().set_param(param_index, value).is_ok());
    if !applied {
        // No such track, or the synth refused the value.
        state.stats.param_rejected();
    }
}

fn apply_set_effect_param(
    state: &mut ProcessingState,
    track_id: TrackId,
    effect_index: usize,
    param_index: usize,
    value: f32,
) {
    let applied = state.mixer.track_mut(track_id).is_some_and(|track| {
        track
            .effects_mut()
            .set_effect_param(effect_index, param_index, value)
            .is_ok()
    });
    if !applied {
        // No such track or effect, or the effect refused the value.
        state.stats.param_rejected();
    }
}

/// Place clip audio loaded off the audio thread. Wrapping it in a clip
/// never allocates; audio with no track, or a clip the track has no room
/// for, is counted and retired. The id is only consumed when the clip is
/// placed.
fn apply_add_clip(
    state: &mut ProcessingState,
    io: &mut ProcessingIO,
    track_id: TrackId,
    clip_data: ClipData,
    position: u64,
) {
    let Some(track) = state.mixer.track_mut(track_id) else {
        state.stats.clip_rejected();
        io.reclaim.retire(Retired::ClipData(clip_data));
        return;
    };
    let clip = AudioClip::new(ClipId(state.next_clip_id), clip_data, position);
    match track.add_clip(clip) {
        Ok(()) => {
            state.next_clip_id += 1;
            state.timeline_dirty = true;
        }
        Err(clip) => {
            state.stats.clip_rejected();
            io.reclaim.retire(Retired::Clip(clip));
        }
    }
}

fn apply_split_clip(
    state: &mut ProcessingState,
    io: &mut ProcessingIO,
    track_id: TrackId,
    clip_id: ClipId,
    split_position: u64,
) {
    let new_clip_id = ClipId(state.next_clip_id);
    let Some(t) = state.mixer.track_mut(track_id) else {
        return;
    };
    // `split_at` trims the original clip in place and returns the right half,
    // so a full track must be detected *before* splitting — otherwise the
    // right half's audio would be cut off and then discarded.
    if t.clips().len() >= MAX_CLIPS_PER_TRACK {
        state.stats.clip_rejected();
        return;
    }
    let right_half = t
        .find_clip_mut(clip_id)
        .and_then(|clip| clip.split_at(split_position, new_clip_id));
    if let Some(right) = right_half {
        match t.add_clip(right) {
            Ok(()) => state.next_clip_id += 1,
            Err(right) => {
                // Unreachable given the capacity check above, but never lose
                // audio silently.
                state.stats.clip_rejected();
                io.reclaim.retire(Retired::Clip(right));
            }
        }
        state.timeline_dirty = true;
    }
}

fn apply_duplicate_clip(
    state: &mut ProcessingState,
    io: &mut ProcessingIO,
    track_id: TrackId,
    clip_id: ClipId,
    new_position: u64,
) {
    let new_id = ClipId(state.next_clip_id);
    let Some(t) = state.mixer.track_mut(track_id) else {
        return;
    };
    // Sharing the source's audio is a reference-count bump: no allocation.
    let Some(clip) = t
        .find_clip(clip_id)
        .map(|source| AudioClip::new(new_id, source.data().clone(), new_position))
    else {
        return;
    };
    match t.add_clip(clip) {
        Ok(()) => {
            state.next_clip_id += 1;
            state.timeline_dirty = true;
        }
        Err(clip) => {
            state.stats.clip_rejected();
            io.reclaim.retire(Retired::Clip(clip));
        }
    }
}
