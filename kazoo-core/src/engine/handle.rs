//! Engine handle: the public API for controlling the engine from the UI thread.
//!
//! [`EngineHandle`] provides methods to send commands and poll display state.
//! It is the sole interface between any frontend and the engine subsystem.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use crossbeam_channel::Sender;
use ringbuf::HeapCons;
use ringbuf::traits::Consumer;

use crate::io::{read_audio_file, resample_mono, to_mono};
use crate::mixer::TrackId;
use crate::mixer::clip::{ClipData, ClipId};
use crate::synthesis::SynthesisMode;
use crate::transport::TransportCommand;
use crate::{Db, Pan, Processor};

use super::command::EngineCommand;
use super::display::DisplayState;

/// Handle to a running engine instance.
///
/// Created by [`super::start`] and used by the UI / TUI to:
/// - send commands to the processing thread via `send_command`
/// - poll the latest display state via `poll_display`
///
/// Dropping the handle sends a [`EngineCommand::Shutdown`] to initiate a
/// graceful teardown of all engine threads.
pub struct EngineHandle {
    /// Channel sender for commands destined for the processing thread.
    command_tx: Sender<EngineCommand>,

    /// Ring buffer consumer for display snapshots produced by the processing
    /// thread. The UI drains this each frame and keeps the latest.
    display_rx: HeapCons<DisplayState>,

    /// Cached copy of the most recently received display state. This avoids
    /// the UI seeing stale data if no new snapshot has arrived since the last
    /// poll.
    last_display: DisplayState,

    /// The negotiated audio sample rate in Hz.
    sample_rate: u32,

    /// The negotiated audio buffer size in samples.
    buffer_size: usize,

    /// Worker thread join handles. Joined on Drop after sending Shutdown.
    /// Wrapped in `Option` so we can take them during drop.
    thread_handles: Option<ThreadHandles>,
}

/// Stores all spawned thread join handles and the stream holder shutdown flag.
pub(super) struct ThreadHandles {
    /// Processing thread handle.
    pub processing: JoinHandle<()>,
    /// Analysis thread handle.
    pub analysis: JoinHandle<()>,
    /// Disk I/O thread handle.
    pub disk: JoinHandle<()>,
    /// Stream holder thread handle.
    pub stream_holder: JoinHandle<()>,
    /// Flag to signal the stream holder to exit.
    pub stream_shutdown: Arc<AtomicBool>,
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
    /// Construct a new engine handle.
    ///
    /// Normally called by [`super::start`]. Exposed publicly so that
    /// downstream crates (e.g. `kazoo-tui`) can construct test handles
    /// without starting real audio threads.
    pub fn new(
        command_tx: Sender<EngineCommand>,
        display_rx: HeapCons<DisplayState>,
        sample_rate: u32,
        buffer_size: usize,
    ) -> Self {
        Self {
            command_tx,
            display_rx,
            last_display: DisplayState::initial(sample_rate),
            sample_rate,
            buffer_size,
            thread_handles: None,
        }
    }

    /// Attach worker thread handles so they can be joined on shutdown.
    ///
    /// Called by [`super::start`] after all threads have been spawned.
    pub(super) fn set_thread_handles(&mut self, handles: ThreadHandles) {
        self.thread_handles = Some(handles);
    }

    // -----------------------------------------------------------------------
    // Core API
    // -----------------------------------------------------------------------

    /// Send a command to the processing thread.
    ///
    /// This is non-blocking. Commands are queued and drained by the processing
    /// thread at the start of each audio block. If the command channel is full,
    /// the command is silently dropped (the processing thread has fallen
    /// behind). Returns `Err` only if the channel is disconnected (engine
    /// not running).
    pub fn send_command(&self, cmd: EngineCommand) -> crate::Result<()> {
        match self.command_tx.try_send(cmd) {
            Ok(()) | Err(crossbeam_channel::TrySendError::Full(_)) => Ok(()),
            Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
                Err(crate::Error::EngineNotRunning)
            }
        }
    }

    /// Poll the display ring buffer and return the most recent snapshot.
    ///
    /// Drains all available snapshots from the ring buffer, keeping only the
    /// latest. If no new snapshot is available, returns a clone of the last
    /// known state.
    pub fn poll_display(&mut self) -> &DisplayState {
        // Drain all available snapshots, keeping only the latest.
        while let Some(state) = self.display_rx.try_pop() {
            self.last_display = state;
        }
        &self.last_display
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

    /// Add a new track with the given name and synthesis mode.
    pub fn add_track(&self, name: String, synthesis_mode: SynthesisMode) -> crate::Result<()> {
        self.send_command(EngineCommand::AddTrack {
            name,
            synthesis_mode,
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

    /// Add an effect to a track's chain.
    pub fn add_effect(&self, track_id: TrackId, effect: Box<dyn Processor>) -> crate::Result<()> {
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
        // Best-effort shutdown: if the channel is full or already closed
        // this is a harmless no-op.
        let _ = self.command_tx.try_send(EngineCommand::Shutdown);

        if let Some(handles) = self.thread_handles.take() {
            // Signal the stream holder thread to exit.
            handles.stream_shutdown.store(true, Ordering::Release);
            handles.stream_holder.thread().unpark();

            // Join all worker threads. Errors from panicked threads are
            // intentionally swallowed during shutdown.
            let _ = handles.processing.join();
            let _ = handles.analysis.join();
            let _ = handles.disk.join();
            let _ = handles.stream_holder.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ringbuf::HeapRb;
    use ringbuf::traits::Split;

    /// Helper: create an `EngineHandle` backed by real channels/buffers but
    /// with no actual audio threads running.
    fn test_handle() -> (EngineHandle, crossbeam_channel::Receiver<EngineCommand>) {
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        let rb = HeapRb::<DisplayState>::new(4);
        let (_prod, cons) = rb.split();
        let handle = EngineHandle::new(cmd_tx, cons, 44_100, 256);
        (handle, cmd_rx)
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
        let (cmd_tx, _cmd_rx) = crossbeam_channel::unbounded();
        let rb = HeapRb::<DisplayState>::new(4);
        let (mut prod, cons) = rb.split();

        let mut handle = EngineHandle::new(cmd_tx, cons, 44_100, 256);

        // Push two snapshots with different cpu_load values.
        let mut s1 = DisplayState::initial(44_100);
        s1.cpu_load = 0.25;
        let mut s2 = DisplayState::initial(44_100);
        s2.cpu_load = 0.75;

        use ringbuf::traits::Producer;
        let _ = prod.try_push(s1);
        let _ = prod.try_push(s2);

        // Poll should drain both and return the latest.
        let state = handle.poll_display();
        assert!((state.cpu_load - 0.75).abs() < f32::EPSILON);
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
        handle
            .add_track("Lead".into(), SynthesisMode::PitchTracked)
            .unwrap();
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(
            cmd,
            EngineCommand::AddTrack {
                name,
                synthesis_mode: SynthesisMode::PitchTracked,
            } if name == "Lead"
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
        let rb = HeapRb::<DisplayState>::new(4);
        let (_prod, cons) = rb.split();

        {
            let _handle = EngineHandle::new(cmd_tx, cons, 44_100, 256);
            // handle drops here
        }

        // The drop impl should have sent a Shutdown command.
        let cmd = cmd_rx.try_recv().unwrap();
        assert!(matches!(cmd, EngineCommand::Shutdown));
    }

    #[test]
    fn send_command_after_receiver_dropped_returns_error() {
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        let rb = HeapRb::<DisplayState>::new(4);
        let (_prod, cons) = rb.split();

        let handle = EngineHandle::new(cmd_tx, cons, 44_100, 256);

        // Drop the receiver to simulate the processing thread having terminated.
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
