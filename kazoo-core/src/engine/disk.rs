//! Disk I/O thread: writes recorded audio to WAV files.
//!
//! This thread runs independently of the real-time output callback. It
//! reads interleaved stereo samples from a ring buffer and writes them to
//! disk via the [`DiskRecorder`]. Commands to start/stop recording come
//! through a dedicated crossbeam channel.

use std::path::PathBuf;

use crossbeam_channel::Receiver;
use ringbuf::HeapCons;
use ringbuf::traits::{Consumer, Observer};

use super::stats::EngineStats;
use crate::io::DiskRecorder;

/// Commands for controlling the disk recorder.
///
/// The output callback numbers every sample it successfully queues in the
/// disk ring buffer (a running `u64` index). `Start` and `Stop` carry the
/// index at which they take effect, so the disk thread attributes every
/// queued sample to exactly the right take even though commands and audio
/// travel on separate channels.
#[derive(Debug)]
pub enum DiskCommand {
    /// Start recording to `path`. Samples with index `>= from_sample` belong
    /// to this take.
    Start {
        /// Destination WAV file.
        path: PathBuf,
        /// First sample index of the take.
        from_sample: u64,
    },
    /// Stop recording and finalize the WAV file. Samples with index
    /// `< at_sample` belong to the take being stopped.
    Stop {
        /// One past the last sample index of the take.
        at_sample: u64,
    },
    /// Write everything still queued, finalize, and shut down the thread.
    Shutdown,
}

/// Entry point for the disk I/O thread.
///
/// Reads interleaved stereo samples from `audio_cons` and writes them to a
/// WAV file while a take is active. Recording state is controlled by
/// commands received on `command_rx`; see [`DiskCommand`] for how samples
/// are attributed to takes. Recorder failures are reported on stderr and
/// counted in [`EngineStats`].
///
/// The thread exits when a `Shutdown` command is received or when the command
/// channel is disconnected, after writing everything still queued.
///
/// # Arguments
///
/// * `audio_cons` -- ring buffer consumer for interleaved stereo samples
/// * `command_rx` -- channel receiver for `DiskCommand`s
/// * `sample_rate` -- audio sample rate (Hz) for the WAV header
/// * `stats` -- shared engine counters
pub fn run(
    audio_cons: HeapCons<f32>,
    command_rx: &Receiver<DiskCommand>,
    sample_rate: u32,
    stats: &EngineStats,
) {
    let mut writer = TakeWriter {
        audio_cons,
        // Pre-allocate a read buffer. 4096 stereo samples = 2048 frames.
        read_buf: vec![0.0_f32; 4096],
        recorder: None,
        consumed: 0,
        stats,
    };

    loop {
        // Snapshot the queue length *before* polling for commands. Any sample
        // counted here was pushed before this point, so any Start/Stop whose
        // boundary falls inside it was sent earlier and is visible to the
        // `try_recv` below. Draining at most this many samples on `Empty`
        // therefore never crosses an unseen take boundary.
        let queued = writer.audio_cons.occupied_len();

        match command_rx.try_recv() {
            Ok(DiskCommand::Start { path, from_sample }) => {
                writer.write_until(from_sample);
                writer.finish_take();
                writer.begin_take(path, sample_rate);
            }
            Ok(DiskCommand::Stop { at_sample }) => {
                writer.write_until(at_sample);
                writer.finish_take();
            }
            Ok(DiskCommand::Shutdown) | Err(crossbeam_channel::TryRecvError::Disconnected) => {
                writer.write_until(u64::MAX);
                writer.finish_take();
                return;
            }
            Err(crossbeam_channel::TryRecvError::Empty) => {
                let target = writer
                    .consumed
                    .saturating_add(u64::try_from(queued).unwrap_or(u64::MAX));
                if writer.write_until(target) == 0 {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
        }
    }
}

/// Disk-thread state: the ring buffer, the active take and the running
/// sample index.
struct TakeWriter<'a> {
    audio_cons: HeapCons<f32>,
    read_buf: Vec<f32>,
    recorder: Option<DiskRecorder>,
    /// Number of samples popped from the ring so far (the index of the next
    /// sample).
    consumed: u64,
    stats: &'a EngineStats,
}

impl TakeWriter<'_> {
    /// Pop samples until the running index reaches `target` or the ring is
    /// empty, writing them to the active take. Samples outside any take
    /// (e.g. belonging to a take whose file could not be created) are
    /// consumed and discarded. Returns the number of samples popped.
    ///
    /// On a write error the take is finalized and ended, so the file never
    /// silently contains a gap.
    fn write_until(&mut self, target: u64) -> usize {
        let mut popped = 0;
        while self.consumed < target {
            let remaining = usize::try_from(target - self.consumed).unwrap_or(usize::MAX);
            let want = remaining.min(self.read_buf.len());
            let n = self.audio_cons.pop_slice(&mut self.read_buf[..want]);
            if n == 0 {
                break;
            }
            popped += n;
            self.consumed = self
                .consumed
                .saturating_add(u64::try_from(n).unwrap_or(u64::MAX));

            let write_result = self
                .recorder
                .as_mut()
                .map(|rec| rec.write_samples(&self.read_buf[..n]));
            if let Some(Err(e)) = write_result {
                eprintln!("disk recorder: write error: {e}");
                self.stats.disk_error();
                self.finish_take();
            }
        }
        popped
    }

    /// Open a new take at `path`, reporting and counting a failure.
    fn begin_take(&mut self, path: PathBuf, sample_rate: u32) {
        let mut rec = DiskRecorder::new(path, sample_rate, 2);
        match rec.start() {
            Ok(()) => self.recorder = Some(rec),
            Err(e) => {
                eprintln!(
                    "disk recorder: failed to start {}: {e}",
                    rec.path().display()
                );
                self.stats.disk_error();
            }
        }
    }

    /// Finalize the active take (if any), reporting and counting a failure.
    fn finish_take(&mut self) {
        let Some(mut rec) = self.recorder.take() else {
            return;
        };
        if let Err(e) = rec.finish() {
            eprintln!(
                "disk recorder: failed to finalize {}: {e}",
                rec.path().display()
            );
            self.stats.disk_error();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_command_start_debug() {
        let cmd = DiskCommand::Start {
            path: PathBuf::from("/tmp/test.wav"),
            from_sample: 0,
        };
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("Start"));
        assert!(dbg.contains("test.wav"));
    }

    #[test]
    fn disk_command_stop_debug() {
        let cmd = DiskCommand::Stop { at_sample: 0 };
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
                run(cons, &cmd_rx, 44_100, &EngineStats::new());
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
                run(cons, &cmd_rx, 44_100, &EngineStats::new());
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
                run(cons, &cmd_rx, 44_100, &EngineStats::new());
            })
            .unwrap();

        // Start recording.
        cmd_tx
            .send(DiskCommand::Start {
                path: record_path,
                from_sample: 0,
            })
            .unwrap();

        // Push some stereo samples (interleaved: L, R, L, R, ...).
        let samples: Vec<f32> = (0..200).map(|i| (i as f32 / 200.0) * 0.5).collect();
        let pushed = prod.push_slice(&samples);
        assert_eq!(pushed, 200);

        // Stop immediately: samples still queued in the ring must be
        // written before the file is finalized.
        cmd_tx.send(DiskCommand::Stop { at_sample: 200 }).unwrap();

        // Shutdown.
        cmd_tx.send(DiskCommand::Shutdown).unwrap();
        handle.join().expect("disk thread should exit");

        // Verify the file was written.
        assert!(path.exists(), "WAV file should exist");
        let loaded = crate::io::file::read_wav(&path).unwrap();
        assert_eq!(loaded.channels, 2);
        assert_eq!(loaded.sample_rate, 44_100);
        assert_eq!(loaded.samples.len(), 200, "every queued sample is written");
    }

    #[test]
    fn back_to_back_takes_split_exactly_at_boundaries() {
        use ringbuf::HeapRb;
        use ringbuf::traits::{Producer, Split};

        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.wav");
        let second = dir.path().join("second.wav");

        let (mut prod, cons) = HeapRb::<f32>::new(8192).split();
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();

        // Queue everything before the thread starts, exactly as a fast
        // output callback could: take 1, stop, take 2, stop.
        cmd_tx
            .send(DiskCommand::Start {
                path: first.clone(),
                from_sample: 0,
            })
            .unwrap();
        assert_eq!(prod.push_slice(&[0.25; 100]), 100);
        cmd_tx.send(DiskCommand::Stop { at_sample: 100 }).unwrap();
        cmd_tx
            .send(DiskCommand::Start {
                path: second.clone(),
                from_sample: 100,
            })
            .unwrap();
        assert_eq!(prod.push_slice(&[-0.5; 60]), 60);
        cmd_tx.send(DiskCommand::Stop { at_sample: 160 }).unwrap();
        cmd_tx.send(DiskCommand::Shutdown).unwrap();

        let stats = EngineStats::new();
        run(cons, &cmd_rx, 44_100, &stats);

        let a = crate::io::file::read_wav(&first).unwrap();
        let b = crate::io::file::read_wav(&second).unwrap();
        assert_eq!(a.samples.len(), 100);
        assert_eq!(b.samples.len(), 60);
        assert!(a.samples.iter().all(|&s| (s - 0.25).abs() < 1e-3));
        assert!(b.samples.iter().all(|&s| (s + 0.5).abs() < 1e-3));
        assert!(stats.snapshot().is_clean());
    }

    #[test]
    fn failed_start_is_counted_and_its_audio_discarded() {
        use ringbuf::HeapRb;
        use ringbuf::traits::{Producer, Split};

        let dir = tempfile::tempdir().unwrap();
        let (mut prod, cons) = HeapRb::<f32>::new(1024).split();
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();

        // A directory cannot be opened as a WAV file.
        cmd_tx
            .send(DiskCommand::Start {
                path: dir.path().to_path_buf(),
                from_sample: 0,
            })
            .unwrap();
        assert_eq!(prod.push_slice(&[0.1; 50]), 50);
        cmd_tx.send(DiskCommand::Stop { at_sample: 50 }).unwrap();
        cmd_tx.send(DiskCommand::Shutdown).unwrap();

        let stats = EngineStats::new();
        run(cons, &cmd_rx, 44_100, &stats);
        assert_eq!(stats.snapshot().disk_errors, 1);
    }
}
