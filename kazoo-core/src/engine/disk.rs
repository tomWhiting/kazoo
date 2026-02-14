//! Disk I/O thread: writes recorded audio to WAV files.
//!
//! This thread runs independently of the real-time output callback. It
//! reads interleaved stereo samples from a ring buffer and writes them to
//! disk via the [`DiskRecorder`]. Commands to start/stop recording come
//! through a dedicated crossbeam channel.

use std::path::PathBuf;

use crossbeam_channel::Receiver;
use ringbuf::HeapCons;
use ringbuf::traits::Consumer;

use crate::io::DiskRecorder;

/// Commands for controlling the disk recorder.
#[derive(Debug)]
pub enum DiskCommand {
    /// Start recording to the given path.
    Start(PathBuf),
    /// Stop recording and finalize the WAV file.
    Stop,
    /// Shut down the disk I/O thread.
    Shutdown,
}

/// Entry point for the disk I/O thread.
///
/// Reads interleaved stereo samples from `audio_cons` and writes them to a
/// WAV file when recording is active. Recording state is controlled by
/// commands received on `command_rx`.
///
/// The thread exits when a `Shutdown` command is received or when the command
/// channel is disconnected.
///
/// # Arguments
///
/// * `audio_cons` -- ring buffer consumer for interleaved stereo samples
/// * `command_rx` -- channel receiver for `DiskCommand`s
/// * `sample_rate` -- audio sample rate (Hz) for the WAV header
#[allow(clippy::needless_pass_by_value)] // Receiver is owned by this thread
pub fn run(mut audio_cons: HeapCons<f32>, command_rx: Receiver<DiskCommand>, sample_rate: u32) {
    // Pre-allocate a read buffer. 4096 stereo samples = 2048 frames.
    let read_buf_size = 4096;
    let mut read_buf = vec![0.0_f32; read_buf_size];

    let mut recorder: Option<DiskRecorder> = None;

    // Track consecutive idle iterations to detect shutdown.
    let mut consecutive_idle = 0_u32;
    let max_consecutive_idle: u32 = 2000; // ~2s at 1ms sleep

    loop {
        // Drain commands.
        let mut shutdown = false;
        loop {
            match command_rx.try_recv() {
                Ok(DiskCommand::Start(path)) => {
                    // Finalize any prior recording.
                    if let Some(ref mut rec) = recorder {
                        let _ = rec.finish();
                    }

                    let mut new_rec = DiskRecorder::new(path, sample_rate, 2);
                    if let Err(e) = new_rec.start() {
                        eprintln!("disk recorder: failed to start: {e}");
                        recorder = None;
                    } else {
                        recorder = Some(new_rec);
                    }
                }
                Ok(DiskCommand::Stop) => {
                    if let Some(ref mut rec) = recorder {
                        if let Err(e) = rec.finish() {
                            eprintln!("disk recorder: failed to stop: {e}");
                        }
                    }
                    recorder = None;
                }
                Ok(DiskCommand::Shutdown) | Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    shutdown = true;
                    break;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => break,
            }
        }

        if shutdown {
            // Finalize any active recording before exiting.
            if let Some(ref mut rec) = recorder {
                let _ = rec.finish();
            }
            break;
        }

        // Read audio from ring buffer and write to disk.
        let num_read = audio_cons.pop_slice(&mut read_buf);

        if num_read == 0 {
            consecutive_idle += 1;
            if consecutive_idle >= max_consecutive_idle && recorder.is_none() {
                // No data and no recording -- might be shutdown.
                // We do not exit here because the command channel might
                // still send a Start command later.
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
            continue;
        }
        consecutive_idle = 0;

        if let Some(ref mut rec) = recorder {
            if let Err(e) = rec.write_samples(&read_buf[..num_read]) {
                eprintln!("disk recorder: write error: {e}");
                // Stop recording on write error to avoid data corruption.
                let _ = rec.finish();
                recorder = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_command_start_debug() {
        let cmd = DiskCommand::Start(PathBuf::from("/tmp/test.wav"));
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("Start"));
        assert!(dbg.contains("test.wav"));
    }

    #[test]
    fn disk_command_stop_debug() {
        let cmd = DiskCommand::Stop;
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("Stop"));
    }

    #[test]
    fn disk_command_shutdown_debug() {
        let cmd = DiskCommand::Shutdown;
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("Shutdown"));
    }

    #[test]
    fn disk_thread_shutdown_via_channel_disconnect() {
        // Verify the disk thread exits when the command channel is dropped.
        use ringbuf::HeapRb;
        use ringbuf::traits::Split;

        let rb = HeapRb::<f32>::new(256);
        let (_prod, cons) = rb.split();

        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();

        let handle = std::thread::Builder::new()
            .name("test-disk-io".into())
            .spawn(move || {
                run(cons, cmd_rx, 44_100);
            })
            .unwrap();

        // Drop the sender to disconnect the channel.
        drop(cmd_tx);

        // The thread should exit within a reasonable time.
        handle.join().expect("disk thread should exit cleanly");
    }

    #[test]
    fn disk_thread_shutdown_via_command() {
        use ringbuf::HeapRb;
        use ringbuf::traits::Split;

        let rb = HeapRb::<f32>::new(256);
        let (_prod, cons) = rb.split();

        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();

        let handle = std::thread::Builder::new()
            .name("test-disk-io-cmd".into())
            .spawn(move || {
                run(cons, cmd_rx, 44_100);
            })
            .unwrap();

        cmd_tx.send(DiskCommand::Shutdown).unwrap();

        handle
            .join()
            .expect("disk thread should exit on shutdown command");
    }

    #[test]
    fn disk_thread_records_samples_to_file() {
        use ringbuf::HeapRb;
        use ringbuf::traits::{Producer, Split};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("disk_test.wav");

        let rb = HeapRb::<f32>::new(8192);
        let (mut prod, cons) = rb.split();

        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();

        let record_path = path.clone();
        let handle = std::thread::Builder::new()
            .name("test-disk-record".into())
            .spawn(move || {
                run(cons, cmd_rx, 44_100);
            })
            .unwrap();

        // Start recording.
        cmd_tx.send(DiskCommand::Start(record_path)).unwrap();

        // Give the thread time to process the command.
        std::thread::sleep(std::time::Duration::from_millis(10));

        // Push some stereo samples (interleaved: L, R, L, R, ...).
        let samples: Vec<f32> = (0..200).map(|i| (i as f32 / 200.0) * 0.5).collect();
        let pushed = prod.push_slice(&samples);
        assert_eq!(pushed, 200);

        // Give the thread time to write.
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Stop recording.
        cmd_tx.send(DiskCommand::Stop).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));

        // Shutdown.
        cmd_tx.send(DiskCommand::Shutdown).unwrap();
        handle.join().expect("disk thread should exit");

        // Verify the file was written.
        assert!(path.exists(), "WAV file should exist");
        let loaded = crate::io::file::read_wav(&path).unwrap();
        assert_eq!(loaded.channels, 2);
        assert_eq!(loaded.sample_rate, 44_100);
        assert!(
            loaded.samples.len() >= 200,
            "should have at least 200 samples"
        );
    }
}
