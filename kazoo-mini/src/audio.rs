//! The cpal output stream and the audio-callback side of the UI plumbing.
//!
//! Everything here that runs inside the callback is real-time safe: no
//! allocation, no freeing of heap memory, no locks, no I/O and no panics.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cpal::traits::DeviceTrait;
use crossbeam_channel::{Receiver, Sender, TryRecvError, TrySendError};
use kazoo_core::ipc::client::HubMessage;
use kazoo_core::ipc::link::HubLinkAudio;
use kazoo_core::ipc::types::{NOTE_OFF, NOTE_ON};

use crate::command::AudioCommand;
use crate::synth::MiniVoice;

/// Largest block rendered in one pass. Device buffers larger than this are
/// rendered in several passes, so every frame is always written.
pub const MAX_CALLBACK_FRAMES: usize = 4096;

/// Push a display snapshot every this many callbacks (~57 Hz at 44.1 kHz
/// with 256-frame buffers).
const DISPLAY_INTERVAL: u32 = 3;

/// Waveform samples carried by each display snapshot.
pub const DISPLAY_BUF_SIZE: usize = 1024;

/// Display snapshot sent from the audio callback to the UI thread.
///
/// Plain arrays only: creating, sending or discarding one never touches the
/// heap.
#[derive(Debug, Clone, Copy)]
pub struct DisplaySnapshot {
    /// Current MIDI note (None when no note is active).
    pub current_note: Option<u8>,
    /// Waveform display buffer — circular buffer contents.
    pub waveform: [f32; DISPLAY_BUF_SIZE],
    /// Write position at time of snapshot (for ring buffer linearization).
    pub write_pos: usize,
}

/// Counters written by the audio thread and read by the UI, so nothing the
/// audio side fails to deliver goes unseen.
#[derive(Debug, Default)]
pub struct AudioStats {
    display_dropped: AtomicU64,
    stream_errors: AtomicU64,
    stream_errors_unreported: AtomicU64,
}

impl AudioStats {
    /// Display snapshots that could not be handed to the UI.
    #[must_use]
    pub fn display_dropped(&self) -> u64 {
        self.display_dropped.load(Ordering::Relaxed)
    }

    /// Errors reported by the audio backend.
    #[must_use]
    pub fn stream_errors(&self) -> u64 {
        self.stream_errors.load(Ordering::Relaxed)
    }

    /// Backend errors whose message could not be queued for the UI (they are
    /// still included in [`Self::stream_errors`]).
    #[must_use]
    pub fn stream_errors_unreported(&self) -> u64 {
        self.stream_errors_unreported.load(Ordering::Relaxed)
    }

    fn record_display_dropped(&self) {
        self.display_dropped.fetch_add(1, Ordering::Relaxed);
    }

    fn record_stream_error(&self, message_queued: bool) {
        self.stream_errors.fetch_add(1, Ordering::Relaxed);
        if !message_queued {
            self.stream_errors_unreported
                .fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Result of [`publish_latest`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Publish {
    /// Queued without displacing anything.
    Delivered,
    /// Queued after discarding the oldest unread value.
    ReplacedStale,
    /// Could not be queued.
    Dropped,
}

/// Queue `value` so the newest value always reaches the reader.
///
/// If the channel is full, the oldest unread value is taken out through
/// `evict` (a clone of the reader's receiver) to make room. With a single
/// producer this can only fail once the reader has gone. `T` must not own
/// heap memory, since a displaced value is dropped on the calling thread.
pub fn publish_latest<T>(tx: &Sender<T>, evict: &Receiver<T>, value: T) -> Publish {
    let value = match tx.try_send(value) {
        Ok(()) => return Publish::Delivered,
        Err(TrySendError::Disconnected(_)) => return Publish::Dropped,
        Err(TrySendError::Full(value)) => value,
    };
    let replaced = match evict.try_recv() {
        Ok(_stale) => true,
        // The reader emptied the queue in the meantime: there is room now.
        Err(TryRecvError::Empty) => false,
        Err(TryRecvError::Disconnected) => return Publish::Dropped,
    };
    match tx.try_send(value) {
        Ok(()) if replaced => Publish::ReplacedStale,
        Ok(()) => Publish::Delivered,
        Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => Publish::Dropped,
    }
}

/// Everything the audio callback takes ownership of.
#[derive(Debug)]
pub struct AudioSetup {
    pub sample_rate: f32,
    pub channels: usize,
    pub cmd_rx: Receiver<AudioCommand>,
    pub display_tx: Sender<DisplaySnapshot>,
    /// Receiver clone used only to evict stale snapshots.
    pub display_evict: Receiver<DisplaySnapshot>,
    pub stream_error_tx: Sender<cpal::StreamError>,
    pub hub_audio: HubLinkAudio,
    pub stats: Arc<AudioStats>,
}

/// Apply one UI command to the voice.
fn apply_command(voice: &mut MiniVoice, cmd: AudioCommand) {
    match cmd {
        AudioCommand::NoteOn { note } => voice.note_on(note),
        AudioCommand::NoteOff { note } => voice.note_off(note),
        AudioCommand::UpdateParams(params) => params.apply_to(voice),
        AudioCommand::AllNotesOff => voice.reset(),
    }
}

/// Apply one hub message to the voice.
fn apply_hub_message(voice: &mut MiniVoice, msg: &HubMessage) {
    match msg {
        HubMessage::NoteEvent(event) => match event.event_type {
            NOTE_ON => voice.note_on(event.note),
            NOTE_OFF => voice.note_off(event.note),
            // Other event types carry nothing the Mini plays.
            _ => {}
        },
        // The Mini has no tempo-dependent features and no remote parameters.
        HubMessage::TransportSync(_) | HubMessage::ParameterChange(_) | HubMessage::Shutdown => {}
    }
}

/// Build the cpal output stream. All synthesis happens in the audio callback.
///
/// The `MiniVoice` is owned entirely by this callback closure. While plugged
/// into the desk, audio goes to kazoo-mix instead of the local output.
pub fn build_audio_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    setup: AudioSetup,
) -> color_eyre::Result<cpal::Stream> {
    let AudioSetup {
        sample_rate,
        channels,
        cmd_rx,
        display_tx,
        display_evict,
        stream_error_tx,
        mut hub_audio,
        stats,
    } = setup;
    if channels == 0 {
        return Err(color_eyre::eyre::eyre!(
            "output device reports zero channels"
        ));
    }

    let mut voice = MiniVoice::new(sample_rate);

    // Pre-allocated scratch buffers.
    let mut mono_buf = vec![0.0_f32; MAX_CALLBACK_FRAMES];
    let mut stereo_buf = vec![0.0_f32; MAX_CALLBACK_FRAMES * 2];

    let mut display_counter: u32 = 0;
    let error_stats = Arc::clone(&stats);

    let stream = device.build_output_stream(
        config,
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
            while let Ok(cmd) = cmd_rx.try_recv() {
                apply_command(&mut voice, cmd);
            }
            while let Some(msg) = hub_audio.try_recv() {
                apply_hub_message(&mut voice, &msg);
            }

            for chunk in data.chunks_mut(MAX_CALLBACK_FRAMES * channels) {
                let frames = chunk.len() / channels;
                // MiniVoice applies soft_limit + sanitize_sample per sample.
                voice.process_block(&mut mono_buf[..frames]);
                for (frame, &sample) in chunk.chunks_exact_mut(channels).zip(&mono_buf[..frames]) {
                    frame.fill(sample);
                }
                // A trailing partial frame (never expected) is silenced.
                chunk[frames * channels..].fill(0.0);

                // Send audio to the hub for mixing (if connected).
                if hub_audio.is_connected() {
                    for (idx, &sample) in mono_buf[..frames].iter().enumerate() {
                        stereo_buf[idx * 2] = sample;
                        stereo_buf[idx * 2 + 1] = sample;
                    }
                    if hub_audio.send_audio(frames as u32, &stereo_buf[..frames * 2]) {
                        // The desk is playing this instrument: don't play it
                        // twice.
                        chunk.fill(0.0);
                    }
                }
            }

            display_counter += 1;
            if display_counter >= DISPLAY_INTERVAL {
                display_counter = 0;
                let mut snapshot = DisplaySnapshot {
                    current_note: voice.current_note(),
                    waveform: [0.0; DISPLAY_BUF_SIZE],
                    write_pos: voice.display_write_pos(),
                };
                let src = voice.display_samples();
                let copy_len = src.len().min(DISPLAY_BUF_SIZE);
                snapshot.waveform[..copy_len].copy_from_slice(&src[..copy_len]);
                match publish_latest(&display_tx, &display_evict, snapshot) {
                    Publish::Delivered | Publish::ReplacedStale => {}
                    Publish::Dropped => stats.record_display_dropped(),
                }
            }
        },
        move |err| {
            // Queue the error for the UI; the count is kept either way.
            let queued = stream_error_tx.try_send(err).is_ok();
            error_stats.record_stream_error(queued);
        },
        None,
    )?;

    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_delivers_when_room() {
        let (tx, rx) = crossbeam_channel::bounded::<u32>(2);
        assert_eq!(publish_latest(&tx, &rx, 1), Publish::Delivered);
        assert_eq!(rx.try_recv().unwrap(), 1);
    }

    #[test]
    fn publish_replaces_oldest_when_full() {
        let (tx, rx) = crossbeam_channel::bounded::<u32>(2);
        let evict = rx.clone();
        assert_eq!(publish_latest(&tx, &evict, 1), Publish::Delivered);
        assert_eq!(publish_latest(&tx, &evict, 2), Publish::Delivered);
        assert_eq!(publish_latest(&tx, &evict, 3), Publish::ReplacedStale);
        let got: Vec<u32> = rx.try_iter().collect();
        assert_eq!(got, vec![2, 3], "newest value must survive");
    }

    #[test]
    fn publish_reports_drop_when_reader_gone() {
        let (tx, rx) = crossbeam_channel::bounded::<u32>(1);
        drop(rx);
        let (_other_tx, orphan) = crossbeam_channel::bounded::<u32>(1);
        assert_eq!(publish_latest(&tx, &orphan, 1), Publish::Dropped);
    }

    #[test]
    fn stats_count_stream_errors() {
        let stats = AudioStats::default();
        stats.record_stream_error(true);
        stats.record_stream_error(false);
        stats.record_display_dropped();
        assert_eq!(stats.stream_errors(), 2);
        assert_eq!(stats.stream_errors_unreported(), 1);
        assert_eq!(stats.display_dropped(), 1);
    }

    #[test]
    fn all_notes_off_silences_voice() {
        let mut voice = MiniVoice::new(44100.0);
        apply_command(&mut voice, AudioCommand::NoteOn { note: 60 });
        let mut buf = [0.0_f32; 512];
        voice.process_block(&mut buf);
        assert_eq!(voice.current_note(), Some(60));

        apply_command(&mut voice, AudioCommand::AllNotesOff);
        assert_eq!(voice.current_note(), None);
        voice.process_block(&mut buf);
        assert!(buf.iter().all(|s| s.abs() < 1e-6), "voice must be silent");
    }

    #[test]
    fn display_snapshot_channel() {
        let (tx, rx) = crossbeam_channel::bounded::<DisplaySnapshot>(2);
        let mut waveform = [0.0_f32; DISPLAY_BUF_SIZE];
        waveform[0] = 0.5;
        waveform[1] = -0.3;
        let snap = DisplaySnapshot {
            current_note: Some(60),
            waveform,
            write_pos: 512,
        };
        assert_eq!(publish_latest(&tx, &rx, snap), Publish::Delivered);
        let received = rx.try_recv().unwrap();
        assert_eq!(received.current_note, Some(60));
        assert!((received.waveform[0] - 0.5).abs() < f32::EPSILON);
        assert!((received.waveform[1] - (-0.3)).abs() < f32::EPSILON);
        assert_eq!(received.write_pos, 512);
    }
}
