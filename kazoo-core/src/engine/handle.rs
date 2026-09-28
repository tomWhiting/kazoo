//! Engine handle: the public API for controlling the engine from the UI thread.
//!
//! [`EngineHandle`] provides methods to send commands and poll display state.
//! It is the sole interface between any frontend and the engine subsystem.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

use crossbeam_channel::Sender;
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};

use crate::io::{read_audio_file, resample_mono, to_mono};
use crate::mixer::clip::{ClipData, ClipId};
use crate::mixer::{Prepared, SynthLayer, Track, TrackId};
use crate::synthesis::SynthesisMode;
use crate::transport::TransportCommand;
use crate::{Db, Pan, Processor};

use super::command::EngineCommand;
use super::display::DisplayState;
use super::reclaim::{Parcel, TimelineMailbox};
use super::stats::{EngineStats, EngineStatsSnapshot};

/// The tracks this handle has added and not removed, and the next id to
/// give one. Ids are assigned here, so the engine and the UI always agree.
#[derive(Debug, Default)]
struct TrackBook {
    next_id: usize,
    live: Vec<TrackId>,
}

/// The handle's end of the display path.
pub(super) struct DisplayLink {
    /// Display frames written by the output callback.
    pub(super) frames: HeapCons<DisplayState>,
    /// Frames the UI has finished with, back to the callback to be written
    /// again.
    pub(super) recycle: HeapProd<DisplayState>,
    /// Timeline snapshots built by the reclaim thread.
    pub(super) timelines: TimelineMailbox,
}

impl DisplayLink {
    /// A display link attached to nothing: no frames or timelines ever
    /// arrive.
    fn detached() -> Self {
        let (_frame_producer, frames) = HeapRb::<DisplayState>::new(1).split();
        let (recycle, _recycle_consumer) = HeapRb::<DisplayState>::new(1).split();
        Self {
            frames,
            recycle,
            timelines: TimelineMailbox::default(),
        }
    }
}

/// Handle to a running engine instance.
///
/// Created by [`super::start`] and used by the UI / TUI to:
/// - send commands to the audio processing callback via `send_command`
///   (the helpers such as [`EngineHandle::add_track`] build what a command
///   carries here, off the audio thread)
/// - poll the latest display state via `poll_display`
///
/// Dropping the handle sends a [`EngineCommand::Shutdown`] to initiate a
/// graceful teardown of all engine threads.
pub struct EngineHandle {
    /// Channel sender for commands destined for the audio processing callback.
    command_tx: Sender<EngineCommand>,

    /// Display frames and timelines from the engine, and the way back for
    /// finished frames.
    display: DisplayLink,

    /// The most recently received display state, with the latest timeline
    /// merged in. The UI sees this until a newer frame arrives.
    last_display: DisplayState,

    /// End of the clips on the latest timeline, before recordings in
    /// progress are added to its length.
    timeline_clip_end: u64,

    /// The negotiated audio sample rate in Hz.
    sample_rate: u32,

    /// The negotiated audio buffer size in samples.
    buffer_size: usize,

    /// Track ids handed out and tracks alive (UI side only, never locked by
    /// the audio thread).
    tracks: Mutex<TrackBook>,

    /// Worker thread join handles. Joined on Drop after sending Shutdown.
    /// Wrapped in `Option` so we can take them during drop.
    thread_handles: Option<ThreadHandles>,

    /// Active MIDI input connection. Dropping disconnects the MIDI device.
    midi_handle: Option<super::midi::MidiHandle>,

    /// Engine health counters shared with every engine thread.
    stats: Arc<EngineStats>,

    /// This engine's plug into the kazoo-mix desk.
    desk: DeskLink,
}

/// The engine's plug into the kazoo-mix desk (see
/// [`super::EngineConfig::desk_instrument_name`]).
#[derive(Debug)]
pub enum DeskLink {
    /// The engine was not asked to plug into the desk.
    Disabled,
    /// The link is running. It keeps looking for the desk while none is
    /// there, and the engine plays locally until the desk takes its audio.
    Running(crate::ipc::link::HubLink),
    /// The link could not be started (the reason is given), so the engine
    /// runs standalone.
    Failed(String),
}

/// Stores all spawned thread join handles and the stream holder shutdown flag.
pub(super) struct ThreadHandles {
    /// Analysis thread handle.
    pub analysis: JoinHandle<()>,
    /// Disk I/O thread handle.
    pub disk: JoinHandle<()>,
    /// Reclaim thread handle (frees what the output callback retires).
    pub reclaim: JoinHandle<()>,
    /// Stream holder thread handle.
    pub stream_holder: JoinHandle<()>,
    /// Flag to signal the stream holder to exit.
    pub stream_shutdown: Arc<AtomicBool>,
    /// Disk command sender — used to signal the disk thread to shut down
    /// during `EngineHandle` drop (since there is no processing thread to
    /// relay the shutdown).
    pub disk_cmd_tx: Sender<super::DiskCommand>,
}

// `HeapCons` is not `Debug`, so implement manually.
impl std::fmt::Debug for EngineHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineHandle")
            .field("sample_rate", &self.sample_rate)
            .field("buffer_size", &self.buffer_size)
            .finish_non_exhaustive()
    }
}

impl EngineHandle {
    /// Construct an engine handle around the display path of a started
    /// engine. `first_display` is the frame shown until the engine's first
    /// arrives; it joins the frames handed back to the engine afterwards,
    /// so it must be sized like them ([`DisplayState::with_capacity`]).
    pub(super) fn new(
        command_tx: Sender<EngineCommand>,
        display: DisplayLink,
        first_display: DisplayState,
        sample_rate: u32,
        buffer_size: usize,
    ) -> Self {
        Self {
            command_tx,
            display,
            last_display: first_display,
            timeline_clip_end: 0,
            sample_rate,
            buffer_size,
            tracks: Mutex::new(TrackBook::default()),
            thread_handles: None,
            midi_handle: None,
            stats: Arc::new(EngineStats::new()),
            desk: DeskLink::Disabled,
        }
    }

    /// A handle attached to no engine: commands go to `command_tx` and no
    /// display frames ever arrive. For driving a frontend without audio
    /// threads (tests, previews).
    #[must_use]
    pub fn detached(
        command_tx: Sender<EngineCommand>,
        sample_rate: u32,
        buffer_size: usize,
    ) -> Self {
        Self::new(
            command_tx,
            DisplayLink::detached(),
            DisplayState::initial(sample_rate),
            sample_rate,
            buffer_size,
        )
    }

    /// Attach the desk link created during engine startup.
    pub(super) fn set_desk_link(&mut self, desk: DeskLink) {
        self.desk = desk;
    }

    /// This engine's plug into the kazoo-mix desk: whether one was asked
    /// for, and if so its live status (see
    /// [`crate::ipc::link::HubLink::status`]).
    #[must_use]
    pub const fn desk_link(&self) -> &DeskLink {
        &self.desk
    }

    /// Share the engine's health counters with this handle.
    ///
    /// Called by [`super::start`] so the handle reports the same counters
    /// the engine threads increment.
    pub(super) fn set_stats(&mut self, stats: Arc<EngineStats>) {
        self.stats = stats;
    }

    /// Current engine health counters: dropped samples, lost commands,
    /// rejected parameters and clips, and disk failures. Every counter is
    /// zero while the engine has lost nothing; frontends should surface
    /// non-zero values.
    #[must_use]
    pub fn stats(&self) -> EngineStatsSnapshot {
        self.stats.snapshot()
    }

    /// Attach worker thread handles so they can be joined on shutdown.
    ///
    /// Called by [`super::start`] after all threads have been spawned.
    pub(super) fn set_thread_handles(&mut self, handles: ThreadHandles) {
        self.thread_handles = Some(handles);
    }

    /// Store the MIDI input handle. Dropping it disconnects the device.
    pub(super) fn set_midi_handle(&mut self, handle: Option<super::midi::MidiHandle>) {
        self.midi_handle = handle;
    }

    /// Whether a MIDI input device is connected.
    #[must_use]
    pub const fn midi_connected(&self) -> bool {
        self.midi_handle.is_some()
    }

    /// Name of the connected MIDI port, if any.
    #[must_use]
    pub fn midi_port_name(&self) -> Option<&str> {
        self.midi_handle
            .as_ref()
            .map(super::midi::MidiHandle::port_name)
    }

    // -----------------------------------------------------------------------
    // Core API
    // -----------------------------------------------------------------------

    /// Send a command to the audio processing callback.
    ///
    /// This is non-blocking. Commands are queued and drained by the output
    /// callback at the start of each audio block.
    ///
    /// # Errors
    ///
    /// [`crate::Error::CommandQueueFull`] if the command channel is full: the
    /// command is dropped (and counted in
    /// [`EngineStatsSnapshot::commands_dropped`], see [`Self::stats`]) and did
    /// not take effect. [`crate::Error::EngineNotRunning`] if the channel is
    /// disconnected.
    pub fn send_command(&self, cmd: EngineCommand) -> crate::Result<()> {
        let removed = match cmd {
            EngineCommand::RemoveTrack(id) => Some(id),
            _ => None,
        };
        match self.command_tx.try_send(cmd) {
            Ok(()) => {
                if let Some(id) = removed {
                    self.track_book().live.retain(|live| *live != id);
                }
                Ok(())
            }
            Err(crossbeam_channel::TrySendError::Full(_)) => {
                self.stats.command_dropped();
                Err(crate::Error::CommandQueueFull)
            }
            Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
                Err(crate::Error::EngineNotRunning)
            }
        }
    }

    /// Poll the display ring buffer and return the most recent snapshot.
    ///
    /// Takes the newest display frame (handing the ones it replaces back to
    /// the engine for reuse) and the newest timeline, and marks each
    /// timeline track's recording in progress. If nothing new has arrived,
    /// returns the last known state.
    pub fn poll_display(&mut self) -> &DisplayState {
        let mut changed = if let Some(timeline) = self.display.timelines.take() {
            self.timeline_clip_end = timeline.total_length;
            self.last_display.timeline = timeline;
            true
        } else {
            false
        };
        while let Some(mut frame) = self.display.frames.try_pop() {
            // The engine never touches a frame's timeline: move the current
            // one onto the new frame, and hand the old frame back.
            std::mem::swap(&mut frame.timeline, &mut self.last_display.timeline);
            let finished = std::mem::replace(&mut self.last_display, frame);
            if let Err(orphan) = self.display.recycle.try_push(finished) {
                // The recycle ring holds every frame in circulation, so this
                // only happens with no engine attached.
                drop(orphan);
            }
            changed = true;
        }
        if changed {
            self.merge_recordings();
        }
        &self.last_display
    }

    /// Mark each timeline track's recording in progress from the latest
    /// frame, and stretch the timeline's length over them.
    fn merge_recordings(&mut self) {
        let display = &mut self.last_display;
        let mut total_length = self.timeline_clip_end;
        for index in 0..display.timeline.tracks.len() {
            let track_id = display.timeline.tracks[index].track_id;
            let recording = display.recording_on(track_id);
            if let Some(span) = recording {
                total_length = total_length.max(span.end());
            }
            display.timeline.tracks[index].recording = recording;
        }
        display.timeline.total_length = total_length;
    }

    /// The negotiated audio sample rate in Hz.
    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The negotiated audio buffer size in samples.
    #[must_use]
    pub const fn buffer_size(&self) -> usize {
        self.buffer_size
    }

    // -----------------------------------------------------------------------
    // Transport convenience methods
    // -----------------------------------------------------------------------

    /// Start playback.
    pub fn play(&self) -> crate::Result<()> {
        self.send_command(EngineCommand::Transport(TransportCommand::Play))
    }

    /// Stop playback and reset to the beginning.
    pub fn stop(&self) -> crate::Result<()> {
        self.send_command(EngineCommand::Transport(TransportCommand::Stop))
    }

    /// Pause playback at the current position.
    pub fn pause(&self) -> crate::Result<()> {
        self.send_command(EngineCommand::Transport(TransportCommand::Pause))
    }

    /// Begin recording (implies playback).
    pub fn record(&self) -> crate::Result<()> {
        self.send_command(EngineCommand::Transport(TransportCommand::Record))
    }

    /// Set the transport tempo in beats per minute.
    pub fn set_tempo(&self, bpm: f64) -> crate::Result<()> {
        self.send_command(EngineCommand::Transport(TransportCommand::SetTempo(bpm)))
    }

    // -----------------------------------------------------------------------
    // Mixer convenience methods
    // -----------------------------------------------------------------------

    /// The track book. Every update to it is a single push or retain, so a
    /// panic elsewhere while it was held cannot leave it inconsistent: a
    /// poisoned lock is used as is.
    fn track_book(&self) -> std::sync::MutexGuard<'_, TrackBook> {
        self.tracks.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Add a new track with the given name and synthesis mode, returning its
    /// id.
    ///
    /// Builds the whole track here — synth and buffers — so the audio thread
    /// only moves it into place. The id is assigned here, so it is the one
    /// the engine uses.
    ///
    /// # Errors
    ///
    /// [`crate::Error::TrackLimit`] if this handle already has
    /// [`crate::MAX_TRACKS`] tracks, or [`EngineHandle::send_command`]'s
    /// errors; the track is not added either way.
    pub fn add_track(&self, name: String, synthesis_mode: SynthesisMode) -> crate::Result<TrackId> {
        // Reserve the id and the place first, so concurrent callers can
        // never exceed the limit; give the place back if sending fails.
        let id = {
            let mut book = self.track_book();
            if book.live.len() >= crate::MAX_TRACKS {
                return Err(crate::Error::TrackLimit);
            }
            let id = TrackId(book.next_id);
            book.next_id += 1;
            book.live.push(id);
            id
        };
        let sample_rate = self.sample_rate as f32;
        let track = Track::new(
            id,
            name,
            super::create_synth(synthesis_mode, sample_rate),
            synthesis_mode,
            sample_rate,
            self.buffer_size,
        );
        let sent = self.send_command(EngineCommand::AddTrack {
            track: Parcel::new(track),
        });
        match sent {
            Ok(()) => Ok(id),
            Err(err) => {
                self.track_book().live.retain(|live| *live != id);
                Err(err)
            }
        }
    }

    /// Remove a track.
    pub fn remove_track(&self, track_id: TrackId) -> crate::Result<()> {
        self.send_command(EngineCommand::RemoveTrack(track_id))
    }

    /// Change a track's synthesis mode, building the new synth here.
    pub fn set_track_synthesis_mode(
        &self,
        track_id: TrackId,
        mode: SynthesisMode,
    ) -> crate::Result<()> {
        let synth = super::prepared_synth(mode, self.sample_rate as f32, self.buffer_size);
        self.send_command(EngineCommand::SetTrackSynthesisMode {
            track_id,
            synth,
            mode,
        })
    }

    /// Add a synth layer to a track, building its synth here.
    pub fn add_synth_layer(
        &self,
        track_id: TrackId,
        mode: SynthesisMode,
        label: String,
    ) -> crate::Result<()> {
        let synth = super::prepared_synth(mode, self.sample_rate as f32, self.buffer_size);
        self.send_command(EngineCommand::AddSynthLayer {
            track_id,
            layer: SynthLayer::new(synth, mode, label),
        })
    }

    /// Set the volume of a specific track.
    pub fn set_track_volume(&self, track_id: TrackId, db: Db) -> crate::Result<()> {
        self.send_command(EngineCommand::SetTrackVolume(track_id, db))
    }

    /// Set the stereo pan position of a specific track.
    pub fn set_track_pan(&self, track_id: TrackId, pan: Pan) -> crate::Result<()> {
        self.send_command(EngineCommand::SetTrackPan(track_id, pan))
    }

    /// Mute or unmute a specific track.
    pub fn set_track_mute(&self, track_id: TrackId, muted: bool) -> crate::Result<()> {
        self.send_command(EngineCommand::SetTrackMute(track_id, muted))
    }

    /// Solo or unsolo a specific track.
    pub fn set_track_solo(&self, track_id: TrackId, soloed: bool) -> crate::Result<()> {
        self.send_command(EngineCommand::SetTrackSolo(track_id, soloed))
    }

    /// Set the master bus volume.
    pub fn set_master_volume(&self, db: Db) -> crate::Result<()> {
        self.send_command(EngineCommand::SetMasterVolume(db))
    }

    /// Add an effect to a track's chain. The effect is set to the engine's
    /// sample rate and prepared for its buffer size here, off the audio
    /// thread.
    pub fn add_effect(&self, track_id: TrackId, effect: Box<dyn Processor>) -> crate::Result<()> {
        let effect = Prepared::new(effect, self.sample_rate as f32, self.buffer_size);
        self.send_command(EngineCommand::AddEffect { track_id, effect })
    }

    /// Start recording the master output to a WAV file.
    pub fn start_recording(&self, path: std::path::PathBuf) -> crate::Result<()> {
        self.send_command(EngineCommand::StartRecording { path })
    }

    /// Stop recording.
    pub fn stop_recording(&self) -> crate::Result<()> {
        self.send_command(EngineCommand::StopRecording)
    }

    // -----------------------------------------------------------------------
    // Clip convenience methods
    // -----------------------------------------------------------------------

    /// Load an audio file, resample to the engine rate, and add as a clip on a track.
    ///
    /// This reads and decodes the file, converts to mono, resamples to the
    /// engine's sample rate if necessary, and sends an `AddClip` command.
    /// File I/O happens on the calling thread (UI thread) -- acceptable for
    /// files under ~10 minutes.
    pub fn load_clip(&self, track_id: TrackId, path: &Path, position: u64) -> crate::Result<()> {
        let audio = read_audio_file(path)?;
        let mono = to_mono(&audio.samples, audio.channels);
        let resampled = if audio.sample_rate == self.sample_rate {
            mono
        } else {
            resample_mono(&mono, audio.sample_rate, self.sample_rate)?
        };

        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Untitled")
            .to_string();

        let clip_data = ClipData::new(resampled, name, Some(path.to_path_buf()), audio.sample_rate);

        self.send_command(EngineCommand::AddClip {
            track_id,
            clip_data,
            position,
        })
    }

    /// Remove a clip from a track.
    pub fn remove_clip(&self, track_id: TrackId, clip_id: ClipId) -> crate::Result<()> {
        self.send_command(EngineCommand::RemoveClip { track_id, clip_id })
    }

    /// Move a clip to a new timeline position.
    pub fn move_clip(
        &self,
        track_id: TrackId,
        clip_id: ClipId,
        position: u64,
    ) -> crate::Result<()> {
        self.send_command(EngineCommand::MoveClip {
            track_id,
            clip_id,
            new_position: position,
        })
    }

    /// Split a clip at the given timeline position.
    pub fn split_clip(
        &self,
        track_id: TrackId,
        clip_id: ClipId,
        position: u64,
    ) -> crate::Result<()> {
        self.send_command(EngineCommand::SplitClip {
            track_id,
            clip_id,
            split_position: position,
        })
    }

    /// Duplicate a clip to a new timeline position.
    pub fn duplicate_clip(
        &self,
        track_id: TrackId,
        clip_id: ClipId,
        position: u64,
    ) -> crate::Result<()> {
        self.send_command(EngineCommand::DuplicateClip {
            track_id,
            clip_id,
            new_position: position,
        })
    }

    /// Set the gain of a specific clip.
    pub fn set_clip_gain(&self, track_id: TrackId, clip_id: ClipId, gain: Db) -> crate::Result<()> {
        self.send_command(EngineCommand::SetClipGain {
            track_id,
            clip_id,
            gain,
        })
    }

    /// Mute or unmute a specific clip.
    pub fn set_clip_mute(
        &self,
        track_id: TrackId,
        clip_id: ClipId,
        muted: bool,
    ) -> crate::Result<()> {
        self.send_command(EngineCommand::SetClipMute {
            track_id,
            clip_id,
            muted,
        })
    }

    /// Initiate a graceful shutdown of the engine.
    pub fn shutdown(&self) -> crate::Result<()> {
        self.send_command(EngineCommand::Shutdown)
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        // Ask the output callback to go silent. A disconnected channel means
        // the callback is already gone, which is the goal. A full channel is
        // also harmless: the stream holder below drops the streams, which
        // stops the callback regardless.
        if let Err(crossbeam_channel::TrySendError::Full(_)) =
            self.command_tx.try_send(EngineCommand::Shutdown)
        {
            self.stats.command_dropped();
        }

        if let Some(handles) = self.thread_handles.take() {
            // Stop the stream holder first. This drops the cpal streams,
            // which drops the output callback closure, which drops
            // ProcessingIO — disconnecting the analysis ring buffer producer,
            // ending the disk sample stream, and releasing the reclaim
            // thread, which frees what is left and exits.
            handles.stream_shutdown.store(true, Ordering::Release);
            handles.stream_holder.thread().unpark();
            report_join("stream holder", handles.stream_holder.join());
            report_join("reclaim", handles.reclaim.join());

            // Tell the disk thread to write what is queued and finalize. If
            // the disk thread has already exited the send fails, and the
            // join below reports why.
            if handles
                .disk_cmd_tx
                .send(super::DiskCommand::Shutdown)
                .is_err()
            {
                eprintln!("kazoo engine: disk thread exited before shutdown");
            }

            // Now the analysis and disk threads can exit.
            report_join("analysis", handles.analysis.join());
            report_join("disk I/O", handles.disk.join());
        }
    }
}

/// Report a worker thread that terminated by panicking. `Drop` cannot return
/// an error, and the handle (with its counters) is going away, so stderr is
/// the only place left to surface it.
fn report_join(thread: &str, result: std::thread::Result<()>) {
    if let Err(panic) = result {
        let message = panic
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("non-string panic payload");
        eprintln!("kazoo engine: {thread} thread panicked: {message}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::display::{
        RecordingSpan, TimelineSnapshot, TrackClipSnapshot, TrackRecording,
    };

    /// Helper: create an `EngineHandle` backed by real channels/buffers but
    /// with no actual audio threads running.
    fn test_handle() -> (EngineHandle, crossbeam_channel::Receiver<EngineCommand>) {
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        let handle = EngineHandle::detached(cmd_tx, 44_100, 256);
        (handle, cmd_rx)
    }

    /// The engine's ends of a handle's display path.
    struct EngineSide {
        frames: HeapProd<DisplayState>,
        recycled: HeapCons<DisplayState>,
        timelines: TimelineMailbox,
    }

    /// A handle wired to display rings the test drives as the engine.
    fn wired_handle() -> (EngineHandle, EngineSide) {
        let (cmd_tx, _cmd_rx) = crossbeam_channel::unbounded();
        let (frame_producer, frames) = HeapRb::<DisplayState>::new(4).split();
        let (recycle, recycled) = HeapRb::<DisplayState>::new(8).split();
        let timelines = TimelineMailbox::default();
        let handle = EngineHandle::new(
            cmd_tx,
            DisplayLink {
                frames,
                recycle,
                timelines: timelines.clone(),
            },
            DisplayState::with_capacity(44_100, 16),
            44_100,
            256,
        );
        let timelines_for_engine = timelines;
        let side = EngineSide {
            frames: frame_producer,
            recycled,
            timelines: timelines_for_engine,
        };
        (handle, side)
    }

    fn timeline_track(track_id: usize) -> TrackClipSnapshot {
        TrackClipSnapshot {
            track_id,
            track_name: format!("{track_id}"),
            clips: Vec::new(),
            recording: None,
        }
    }

    #[test]
    fn send_command_is_received() {
        let (handle, cmd_rx) = test_handle();
        handle
            .send_command(EngineCommand::SetMasterVolume(Db::new(-3.0)))
            .unwrap();

        let received = cmd_rx.try_recv().unwrap();
        assert!(matches!(received, EngineCommand::SetMasterVolume(_)));
    }

    #[test]
    fn poll_display_returns_initial_when_empty() {
        let (mut handle, _rx) = test_handle();
        let state = handle.poll_display();
        assert!(state.spectrum_magnitudes.is_empty());
        assert!(state.pitch.frequency.is_none());
    }

    #[test]
    fn poll_display_returns_latest_snapshot() {
        let (mut handle, mut engine) = wired_handle();

        // Push two snapshots with different cpu_load values.
        let mut s1 = DisplayState::initial(44_100);
        s1.cpu_load = 0.25;
        let mut s2 = DisplayState::initial(44_100);
        s2.cpu_load = 0.75;

        assert!(
            engine.frames.try_push(s1).is_ok(),
            "first snapshot must fit"
        );
        assert!(
            engine.frames.try_push(s2).is_ok(),
            "second snapshot must fit"
        );

        // Poll should drain both and return the latest.
        let state = handle.poll_display();
        assert!((state.cpu_load - 0.75).abs() < f32::EPSILON);
    }

    #[test]
    fn poll_display_hands_replaced_frames_back_to_the_engine() {
        let (mut handle, mut engine) = wired_handle();
        let first_spectrum = handle.poll_display().spectrum_magnitudes.as_ptr();

        let mut s1 = DisplayState::initial(44_100);
        s1.cpu_load = 0.25;
        let mut s2 = DisplayState::initial(44_100);
        s2.cpu_load = 0.75;
        assert!(engine.frames.try_push(s1).is_ok());
        assert!(engine.frames.try_push(s2).is_ok());
        handle.poll_display();

        // The initial frame (with its buffers) and the first engine frame
        // come back, in order; the newest stays with the handle.
        let recycled: Vec<DisplayState> =
            std::iter::from_fn(|| engine.recycled.try_pop()).collect();
        assert_eq!(recycled.len(), 2);
        assert_eq!(recycled[0].spectrum_magnitudes.as_ptr(), first_spectrum);
        assert!((recycled[1].cpu_load - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn poll_display_carries_the_timeline_onto_new_frames() {
        let (mut handle, mut engine) = wired_handle();
        let timeline = TimelineSnapshot {
            tracks: vec![timeline_track(0), timeline_track(1)],
            total_length: 500,
        };
        engine.timelines.post(timeline);
        assert_eq!(handle.poll_display().timeline.tracks.len(), 2);

        // A new frame from the engine (whose own timeline is empty) keeps
        // the timeline; the frame handed back has an empty one.
        assert!(
            engine
                .frames
                .try_push(DisplayState::initial(44_100))
                .is_ok()
        );
        let display = handle.poll_display();
        assert_eq!(display.timeline.tracks.len(), 2);
        assert_eq!(display.timeline.total_length, 500);
        let recycled = engine.recycled.try_pop().unwrap();
        assert!(recycled.timeline.tracks.is_empty());
    }

    #[test]
    fn poll_display_marks_recordings_on_their_tracks() {
        let (mut handle, mut engine) = wired_handle();
        let timeline = TimelineSnapshot {
            tracks: vec![timeline_track(0), timeline_track(4)],
            total_length: 500,
        };
        engine.timelines.post(timeline);

        let span = RecordingSpan {
            start: 400,
            length: 300,
        };
        let mut frame = DisplayState::initial(44_100);
        frame.recordings[0] = Some(TrackRecording { track_id: 4, span });
        assert!(engine.frames.try_push(frame).is_ok());

        let display = handle.poll_display();
        assert_eq!(display.timeline.tracks[0].recording, None);
        assert_eq!(display.timeline.tracks[1].recording, Some(span));
        assert_eq!(display.timeline.total_length, 700);

        // The take ends: the span goes and the length falls back.
        assert!(
            engine
                .frames
                .try_push(DisplayState::initial(44_100))
                .is_ok()
        );
        let display = handle.poll_display();
        assert_eq!(display.timeline.tracks[1].recording, None);
        assert_eq!(display.timeline.total_length, 500);
    }

    #[test]
    fn convenience_play_sends_transport_play() {
        let (handle, rx) = test_handle();
        handle.play().unwrap();
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(
            cmd,
            EngineCommand::Transport(TransportCommand::Play)
        ));
    }

    #[test]
    fn convenience_stop_sends_transport_stop() {
        let (handle, rx) = test_handle();
        handle.stop().unwrap();
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(
            cmd,
            EngineCommand::Transport(TransportCommand::Stop)
        ));
    }

    #[test]
    fn convenience_pause_sends_transport_pause() {
        let (handle, rx) = test_handle();
        handle.pause().unwrap();
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(
            cmd,
            EngineCommand::Transport(TransportCommand::Pause)
        ));
    }

    #[test]
    fn convenience_record_sends_transport_record() {
        let (handle, rx) = test_handle();
        handle.record().unwrap();
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(
            cmd,
            EngineCommand::Transport(TransportCommand::Record)
        ));
    }

    #[test]
    fn convenience_set_tempo() {
        let (handle, rx) = test_handle();
        handle.set_tempo(140.0).unwrap();
        let cmd = rx.try_recv().unwrap();
        assert!(
            matches!(cmd, EngineCommand::Transport(TransportCommand::SetTempo(bpm)) if (bpm - 140.0).abs() < f64::EPSILON)
        );
    }

    #[test]
    fn convenience_add_track() {
        let (handle, rx) = test_handle();
        let id = handle
            .add_track("Lead".into(), SynthesisMode::PitchTracked)
            .unwrap();
        assert_eq!(id, TrackId(0));
        let cmd = rx.try_recv().unwrap();
        let EngineCommand::AddTrack { track } = cmd else {
            panic!("expected AddTrack, got {cmd:?}");
        };
        // The whole track arrives built and prepared for the engine's
        // blocks, with the id the handle returned.
        let track = track.get().unwrap();
        assert_eq!(track.id(), id);
        assert_eq!(track.name(), "Lead");
        assert_eq!(track.synth().name(), "Pitch Tracked Synth");
        assert_eq!(track.buffer_size(), 256);
    }

    #[test]
    fn add_track_ids_count_up_and_the_limit_is_enforced() {
        let (handle, _rx) = test_handle();
        for i in 0..crate::MAX_TRACKS {
            let id = handle
                .add_track(format!("{i}"), SynthesisMode::Passthrough)
                .unwrap();
            assert_eq!(id, TrackId(i));
        }
        assert!(matches!(
            handle.add_track("Extra".into(), SynthesisMode::Passthrough),
            Err(crate::Error::TrackLimit)
        ));
        // Removing one makes room; ids are never reused.
        handle.remove_track(TrackId(3)).unwrap();
        let id = handle
            .add_track("Again".into(), SynthesisMode::Passthrough)
            .unwrap();
        assert_eq!(id, TrackId(crate::MAX_TRACKS));
    }

    #[test]
    fn a_refused_add_track_takes_no_place() {
        let (cmd_tx, cmd_rx) = crossbeam_channel::bounded(1);
        let handle = EngineHandle::detached(cmd_tx, 44_100, 256);
        handle.play().unwrap();
        assert!(matches!(
            handle.add_track("Full".into(), SynthesisMode::Passthrough),
            Err(crate::Error::CommandQueueFull)
        ));
        assert!(handle.track_book().live.is_empty());
        drop(cmd_rx.try_recv().unwrap());
        // The next track gets a fresh id (ids are never reused).
        let id = handle
            .add_track("Next".into(), SynthesisMode::Passthrough)
            .unwrap();
        assert_eq!(id, TrackId(1));
        assert_eq!(handle.track_book().live, vec![TrackId(1)]);
    }

    #[test]
    fn convenience_set_track_synthesis_mode_builds_the_synth() {
        let (handle, rx) = test_handle();
        handle
            .set_track_synthesis_mode(TrackId(2), SynthesisMode::Granular)
            .unwrap();
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(
            cmd,
            EngineCommand::SetTrackSynthesisMode {
                track_id: TrackId(2),
                ref synth,
                mode: SynthesisMode::Granular,
            } if synth.processor().name() == "Granular Synth"
        ));
    }

    #[test]
    fn convenience_add_synth_layer_builds_the_layer() {
        let (handle, rx) = test_handle();
        handle
            .add_synth_layer(TrackId(1), SynthesisMode::Vocoder, "Pad".into())
            .unwrap();
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(
            cmd,
            EngineCommand::AddSynthLayer {
                track_id: TrackId(1),
                ref layer,
            } if layer.label() == "Pad"
                && layer.mode() == SynthesisMode::Vocoder
                && layer.synth().name() == "Vocoder"
        ));
    }

    #[test]
    fn convenience_set_track_volume() {
        let (handle, rx) = test_handle();
        handle.set_track_volume(TrackId(0), Db::new(-6.0)).unwrap();
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(cmd, EngineCommand::SetTrackVolume(TrackId(0), _)));
    }

    #[test]
    fn convenience_set_track_pan() {
        let (handle, rx) = test_handle();
        handle.set_track_pan(TrackId(1), Pan::new(0.5)).unwrap();
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(cmd, EngineCommand::SetTrackPan(TrackId(1), _)));
    }

    #[test]
    fn convenience_set_track_mute() {
        let (handle, rx) = test_handle();
        handle.set_track_mute(TrackId(0), true).unwrap();
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(cmd, EngineCommand::SetTrackMute(TrackId(0), true)));
    }

    #[test]
    fn convenience_set_track_solo() {
        let (handle, rx) = test_handle();
        handle.set_track_solo(TrackId(0), true).unwrap();
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(cmd, EngineCommand::SetTrackSolo(TrackId(0), true)));
    }

    #[test]
    fn convenience_set_master_volume() {
        let (handle, rx) = test_handle();
        handle.set_master_volume(Db::new(-12.0)).unwrap();
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(cmd, EngineCommand::SetMasterVolume(_)));
    }

    #[test]
    fn convenience_start_recording() {
        let (handle, rx) = test_handle();
        handle
            .start_recording(std::path::PathBuf::from("/tmp/rec.wav"))
            .unwrap();
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(cmd, EngineCommand::StartRecording { .. }));
    }

    #[test]
    fn convenience_stop_recording() {
        let (handle, rx) = test_handle();
        handle.stop_recording().unwrap();
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(cmd, EngineCommand::StopRecording));
    }

    #[test]
    fn convenience_shutdown() {
        let (handle, rx) = test_handle();
        handle.shutdown().unwrap();
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(cmd, EngineCommand::Shutdown));
    }

    #[test]
    fn sample_rate_accessor() {
        let (handle, _rx) = test_handle();
        assert_eq!(handle.sample_rate(), 44_100);
    }

    #[test]
    fn buffer_size_accessor() {
        let (handle, _rx) = test_handle();
        assert_eq!(handle.buffer_size(), 256);
    }

    #[test]
    fn debug_format_does_not_panic() {
        let (handle, _rx) = test_handle();
        let dbg = format!("{handle:?}");
        assert!(dbg.contains("EngineHandle"));
        assert!(dbg.contains("44100"));
    }

    #[test]
    fn drop_sends_shutdown() {
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        {
            let _handle = EngineHandle::detached(cmd_tx, 44_100, 256);
            // handle drops here
        }

        // The drop impl should have sent a Shutdown command.
        let cmd = cmd_rx.try_recv().unwrap();
        assert!(matches!(cmd, EngineCommand::Shutdown));
    }

    #[test]
    fn send_command_on_full_queue_reports_and_counts_the_drop() {
        let (cmd_tx, cmd_rx) = crossbeam_channel::bounded(1);
        let handle = EngineHandle::detached(cmd_tx, 44_100, 256);

        handle.play().unwrap();
        assert!(matches!(handle.stop(), Err(crate::Error::CommandQueueFull)));
        assert_eq!(handle.stats().commands_dropped, 1);

        // Only the command that fitted was delivered.
        assert!(matches!(cmd_rx.try_recv(), Ok(EngineCommand::Transport(_))));
        assert!(cmd_rx.try_recv().is_err());
        drop(handle);
    }

    #[test]
    fn send_command_after_receiver_dropped_returns_error() {
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        let handle = EngineHandle::detached(cmd_tx, 44_100, 256);

        // Drop the receiver to simulate the output callback having terminated.
        drop(cmd_rx);

        let result = handle.send_command(EngineCommand::Shutdown);
        assert!(result.is_err());

        // Prevent the drop impl from printing an error by forgetting the handle.
        std::mem::forget(handle);
    }

    // -----------------------------------------------------------------------
    // Clip convenience method tests
    // -----------------------------------------------------------------------

    #[test]
    fn convenience_remove_clip() {
        let (handle, rx) = test_handle();
        handle.remove_clip(TrackId(0), ClipId(1)).unwrap();
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(
            cmd,
            EngineCommand::RemoveClip {
                track_id: TrackId(0),
                clip_id: ClipId(1),
            }
        ));
    }

    #[test]
    fn convenience_move_clip() {
        let (handle, rx) = test_handle();
        handle.move_clip(TrackId(2), ClipId(5), 44_100).unwrap();
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(
            cmd,
            EngineCommand::MoveClip {
                track_id: TrackId(2),
                clip_id: ClipId(5),
                new_position: 44_100,
            }
        ));
    }

    #[test]
    fn convenience_split_clip() {
        let (handle, rx) = test_handle();
        handle.split_clip(TrackId(1), ClipId(3), 88_200).unwrap();
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(
            cmd,
            EngineCommand::SplitClip {
                track_id: TrackId(1),
                clip_id: ClipId(3),
                split_position: 88_200,
            }
        ));
    }

    #[test]
    fn convenience_duplicate_clip() {
        let (handle, rx) = test_handle();
        handle
            .duplicate_clip(TrackId(0), ClipId(7), 132_300)
            .unwrap();
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(
            cmd,
            EngineCommand::DuplicateClip {
                track_id: TrackId(0),
                clip_id: ClipId(7),
                new_position: 132_300,
            }
        ));
    }

    #[test]
    fn convenience_set_clip_gain() {
        let (handle, rx) = test_handle();
        handle
            .set_clip_gain(TrackId(1), ClipId(2), Db::new(-6.0))
            .unwrap();
        let cmd = rx.try_recv().unwrap();
        match cmd {
            EngineCommand::SetClipGain {
                track_id: TrackId(1),
                clip_id: ClipId(2),
                gain,
            } => {
                assert!(
                    (gain.value() - (-6.0)).abs() < f32::EPSILON,
                    "expected -6.0 dB, got {}",
                    gain.value()
                );
            }
            other => panic!("expected SetClipGain, got {other:?}"),
        }
    }

    #[test]
    fn convenience_set_clip_mute() {
        let (handle, rx) = test_handle();
        handle.set_clip_mute(TrackId(0), ClipId(4), true).unwrap();
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(
            cmd,
            EngineCommand::SetClipMute {
                track_id: TrackId(0),
                clip_id: ClipId(4),
                muted: true,
            }
        ));
    }

    #[test]
    fn convenience_set_clip_mute_false() {
        let (handle, rx) = test_handle();
        handle.set_clip_mute(TrackId(3), ClipId(9), false).unwrap();
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(
            cmd,
            EngineCommand::SetClipMute {
                track_id: TrackId(3),
                clip_id: ClipId(9),
                muted: false,
            }
        ));
    }
}
