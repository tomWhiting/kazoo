//! Audio engine: real-time audio graph, buffer management, thread coordination.
//!
//! The engine orchestrates five threads:
//!
//! 1. **cpal input callback** (OS-managed) -- writes mic samples to ring buffer.
//! 2. **cpal output callback** (OS-managed) -- reads mixed output from ring buffer.
//! 3. **Processing thread** (`kazoo-processing`) -- main workhorse that drains
//!    commands, runs the mixer, writes display state.
//! 4. **Analysis thread** (`kazoo-analysis`) -- runs pitch, spectrum, formant,
//!    and onset detection.
//! 5. **Disk I/O thread** (`kazoo-disk-io`) -- writes recorded audio to WAV files.
//!
//! Communication between threads uses lock-free ring buffers (`ringbuf` crate)
//! for audio data and `crossbeam-channel` for commands.
//!
//! The sole public entry point is [`start`], which returns an [`EngineHandle`]
//! that provides command dispatch and display state polling.

pub mod analysis_thread;
pub mod command;
pub mod disk;
pub mod display;
pub mod handle;
pub mod processing;

pub use command::EngineCommand;
pub use disk::DiskCommand;
pub use display::{ClipSnapshot, DisplayState, TimelineSnapshot, TrackClipSnapshot};
pub use handle::EngineHandle;
pub use processing::create_synth;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Producer, Split};

use crate::analysis::{FormantData, PitchDetectorConfig, PitchEstimate};
use crate::io::StreamConfig;
use crate::{DEFAULT_BUFFER_SIZE, DEFAULT_SAMPLE_RATE, Result, SPECTRUM_FFT_SIZE};

use analysis_thread::AnalysisConfig;
use handle::ThreadHandles;

// ---------------------------------------------------------------------------
// EngineConfig
// ---------------------------------------------------------------------------

/// Configuration for the audio engine.
///
/// All fields have sensible defaults via [`Default`]. Pass to [`start`] to
/// boot the engine.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Audio stream configuration (device selection, sample rate, buffer size).
    pub stream: StreamConfig,

    /// Pitch detector configuration.
    pub pitch: PitchDetectorConfig,

    /// FFT size for spectrum analysis (default 2048).
    pub spectrum_fft_size: usize,

    /// EMA smoothing factor for spectrum display (default 0.8).
    pub spectrum_smoothing: f32,

    /// FFT size for onset detection (default 1024).
    pub onset_fft_size: usize,

    /// Threshold factor for onset detection (default 0.3).
    pub onset_threshold: f32,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            stream: StreamConfig::default(),
            pitch: PitchDetectorConfig::default(),
            spectrum_fft_size: SPECTRUM_FFT_SIZE,
            spectrum_smoothing: 0.8,
            onset_fft_size: 1024,
            onset_threshold: 0.3,
        }
    }
}

// ---------------------------------------------------------------------------
// Ring buffer capacity helpers
// ---------------------------------------------------------------------------

/// Compute capacities for all ring buffers based on buffer size.
struct RingBufferCapacities {
    /// Mic input and output: `buffer_size` * 8.
    mic_and_output: usize,
    /// Display state: 4 slots.
    display: usize,
    /// Analysis input: `buffer_size` * 16.
    analysis_input: usize,
    /// Analysis results (pitch, spectrum, formant): 32 slots each.
    analysis_results: usize,
    /// Disk recording: `buffer_size` * 32.
    disk: usize,
}

impl RingBufferCapacities {
    fn from_buffer_size(buffer_size: usize) -> Self {
        let bs = buffer_size.max(1);
        Self {
            mic_and_output: bs.saturating_mul(16),
            display: 4,
            analysis_input: bs.saturating_mul(16),
            analysis_results: 32,
            disk: bs.saturating_mul(32),
        }
    }
}

// ---------------------------------------------------------------------------
// start()
// ---------------------------------------------------------------------------

/// Boot the audio engine and return a handle for controlling it.
///
/// This function:
/// 1. Builds audio I/O streams via `cpal`.
/// 2. Creates all inter-thread ring buffers.
/// 3. Spawns the processing, analysis, and disk I/O threads.
/// 4. Returns an [`EngineHandle`] for the caller to send commands and poll
///    display state.
///
/// # Errors
///
/// Returns [`crate::Error::AudioDevice`] if audio streams cannot be created,
/// or [`crate::Error::Config`] for invalid configuration values.
#[allow(clippy::too_many_lines)]
pub fn start(config: EngineConfig) -> Result<EngineHandle> {
    let stream_config = &config.stream;

    // Determine effective sample rate and buffer size.
    let sample_rate = stream_config.sample_rate.unwrap_or(DEFAULT_SAMPLE_RATE);
    let buffer_size = stream_config.buffer_size.unwrap_or(DEFAULT_BUFFER_SIZE);

    if sample_rate == 0 {
        return Err(crate::Error::Config("sample rate must be > 0".into()));
    }
    if buffer_size == 0 {
        return Err(crate::Error::Config("buffer size must be > 0".into()));
    }

    // -----------------------------------------------------------------------
    // 1. Create ring buffers
    // -----------------------------------------------------------------------
    let caps = RingBufferCapacities::from_buffer_size(buffer_size);

    // Mic input: cpal input callback -> processing thread.
    let mic_rb = HeapRb::<f32>::new(caps.mic_and_output);
    let (mic_prod, mic_cons) = mic_rb.split();

    // Output: processing thread -> cpal output callback.
    let output_rb = HeapRb::<f32>::new(caps.mic_and_output);
    let (output_prod, output_cons) = output_rb.split();

    // Display: processing thread -> UI thread.
    let display_rb = HeapRb::<DisplayState>::new(caps.display);
    let (display_prod, display_cons) = display_rb.split();

    // Analysis input: processing thread -> analysis thread.
    let analysis_in_rb = HeapRb::<f32>::new(caps.analysis_input);
    let (analysis_in_prod, analysis_in_cons) = analysis_in_rb.split();

    // Analysis results: analysis thread -> processing thread.
    let pitch_rb = HeapRb::<PitchEstimate>::new(caps.analysis_results);
    let (pitch_prod, pitch_cons) = pitch_rb.split();

    let spectrum_rb = HeapRb::<Vec<f32>>::new(caps.analysis_results);
    let (spectrum_prod, spectrum_cons) = spectrum_rb.split();

    let formant_rb = HeapRb::<Option<FormantData>>::new(caps.analysis_results);
    let (formant_prod, formant_cons) = formant_rb.split();

    // Disk recording: processing thread -> disk I/O thread.
    let disk_rb = HeapRb::<f32>::new(caps.disk);
    let (disk_prod, disk_cons) = disk_rb.split();

    // -----------------------------------------------------------------------
    // 2. Create command channels (bounded per CLAUDE.md)
    // -----------------------------------------------------------------------
    let (command_tx, command_rx) = crossbeam_channel::bounded::<EngineCommand>(256);
    let (disk_cmd_tx, disk_cmd_rx) = crossbeam_channel::bounded::<DiskCommand>(64);

    // -----------------------------------------------------------------------
    // 3. Build audio streams
    // -----------------------------------------------------------------------
    // Wrap the ring buffer producers/consumers in Mutex-free closures for the
    // cpal callbacks. We use `move` closures that own one side of each ring
    // buffer.

    let mut cpal_mic_prod = mic_prod;
    let mut cpal_output_cons = output_cons;

    // Capture input channel count so the callback can downmix to mono.
    let input_channels = usize::from(stream_config.input_channels.max(1));

    let streams = crate::io::build_streams(
        stream_config,
        move |data: &[f32]| {
            // Input callback: push mic samples into the ring buffer as mono.
            // If the device delivers multi-channel data we average across
            // channels to produce a single mono sample per frame.
            if input_channels <= 1 {
                let _ = cpal_mic_prod.push_slice(data);
            } else {
                let frames = data.len() / input_channels;
                let inv_ch = 1.0 / input_channels as f32;
                for f in 0..frames {
                    let base = f * input_channels;
                    let mut sum = 0.0f32;
                    for ch in 0..input_channels {
                        sum += data[base + ch];
                    }
                    let _ = cpal_mic_prod.try_push(sum * inv_ch);
                }
            }
        },
        move |data: &mut [f32]| {
            // Output callback: pop mixed stereo samples from the ring buffer.
            let filled = cpal_output_cons.pop_slice(data);
            // Ensure stereo frame alignment — avoid leaving a lone L sample
            // without its R partner, which would swap channels for the rest
            // of the buffer.
            let aligned = filled & !1;
            for sample in &mut data[aligned..] {
                *sample = 0.0;
            }
        },
    )?;

    // cpal::Stream is !Send on some platforms, so we hold streams alive in a
    // dedicated parked thread rather than storing them in EngineHandle.
    // Use an AtomicBool shutdown flag so the thread exits cleanly on drop.
    let stream_shutdown = Arc::new(AtomicBool::new(false));
    let stream_shutdown_clone = Arc::clone(&stream_shutdown);

    let stream_holder_handle = std::thread::Builder::new()
        .name("kazoo-streams".into())
        .spawn(move || {
            let _keep_alive = streams;
            while !stream_shutdown_clone.load(Ordering::Acquire) {
                std::thread::park();
            }
        })
        .map_err(|e| crate::Error::Stream(format!("failed to spawn streams holder: {e}")))?;

    // -----------------------------------------------------------------------
    // 4. Spawn processing thread
    // -----------------------------------------------------------------------
    let proc_sample_rate = sample_rate;
    let proc_buffer_size = buffer_size;
    let proc_disk_cmd_tx = disk_cmd_tx.clone();

    let processing_handle = std::thread::Builder::new()
        .name("kazoo-processing".into())
        .spawn(move || {
            processing::run(
                command_rx,
                mic_cons,
                output_prod,
                display_prod,
                analysis_in_prod,
                disk_prod,
                pitch_cons,
                spectrum_cons,
                formant_cons,
                proc_disk_cmd_tx,
                proc_sample_rate,
                proc_buffer_size,
            );
            // When processing exits, signal the disk thread to shut down.
            let _ = disk_cmd_tx.send(DiskCommand::Shutdown);
        })
        .map_err(|e| crate::Error::Stream(format!("failed to spawn processing thread: {e}")))?;

    // -----------------------------------------------------------------------
    // 5. Spawn analysis thread
    // -----------------------------------------------------------------------
    let analysis_config = AnalysisConfig {
        pitch: config.pitch,
        spectrum_fft_size: config.spectrum_fft_size,
        spectrum_smoothing: config.spectrum_smoothing,
        onset_fft_size: config.onset_fft_size,
        onset_threshold: config.onset_threshold,
        sample_rate,
        buffer_size,
    };

    let analysis_handle = std::thread::Builder::new()
        .name("kazoo-analysis".into())
        .spawn(move || {
            analysis_thread::run(
                analysis_in_cons,
                pitch_prod,
                spectrum_prod,
                formant_prod,
                analysis_config,
            );
        })
        .map_err(|e| crate::Error::Stream(format!("failed to spawn analysis thread: {e}")))?;

    // -----------------------------------------------------------------------
    // 6. Spawn disk I/O thread
    // -----------------------------------------------------------------------
    let disk_handle = std::thread::Builder::new()
        .name("kazoo-disk-io".into())
        .spawn(move || {
            disk::run(disk_cons, disk_cmd_rx, sample_rate);
        })
        .map_err(|e| crate::Error::Stream(format!("failed to spawn disk I/O thread: {e}")))?;

    // -----------------------------------------------------------------------
    // 7. Build and return the engine handle
    // -----------------------------------------------------------------------
    let mut handle = EngineHandle::new(command_tx, display_cons, sample_rate, buffer_size);

    handle.set_thread_handles(ThreadHandles {
        processing: processing_handle,
        analysis: analysis_handle,
        disk: disk_handle,
        stream_holder: stream_holder_handle,
        stream_shutdown,
    });

    Ok(handle)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_config_default_values() {
        let config = EngineConfig::default();
        assert_eq!(config.spectrum_fft_size, SPECTRUM_FFT_SIZE);
        assert!((config.spectrum_smoothing - 0.8).abs() < f32::EPSILON);
        assert_eq!(config.onset_fft_size, 1024);
        assert!((config.onset_threshold - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    fn engine_config_debug_format() {
        let config = EngineConfig::default();
        let dbg = format!("{config:?}");
        assert!(dbg.contains("EngineConfig"));
    }

    #[test]
    fn engine_config_clone() {
        let config = EngineConfig::default();
        let cloned = config.clone();
        assert_eq!(cloned.spectrum_fft_size, config.spectrum_fft_size);
    }

    #[test]
    fn ring_buffer_capacities_default_buffer_size() {
        let caps = RingBufferCapacities::from_buffer_size(256);
        assert_eq!(caps.mic_and_output, 256 * 16);
        assert_eq!(caps.display, 4);
        assert_eq!(caps.analysis_input, 256 * 16);
        assert_eq!(caps.analysis_results, 32);
        assert_eq!(caps.disk, 256 * 32);
    }

    #[test]
    fn ring_buffer_capacities_large_buffer_size() {
        let caps = RingBufferCapacities::from_buffer_size(1024);
        assert_eq!(caps.mic_and_output, 1024 * 16);
        assert_eq!(caps.analysis_input, 1024 * 16);
        assert_eq!(caps.disk, 1024 * 32);
    }

    #[test]
    fn ring_buffer_capacities_zero_buffer_size_uses_one() {
        let caps = RingBufferCapacities::from_buffer_size(0);
        assert_eq!(caps.mic_and_output, 16);
        assert_eq!(caps.analysis_input, 16);
        assert_eq!(caps.disk, 32);
    }

    #[test]
    fn display_state_initial_construction() {
        let state = DisplayState::initial(44_100);
        assert!(state.spectrum_magnitudes.is_empty());
        assert!(state.waveform.is_empty());
        assert!(!state.is_recording);
    }

    #[test]
    fn engine_config_custom_values() {
        let config = EngineConfig {
            stream: StreamConfig {
                sample_rate: Some(48_000),
                buffer_size: Some(512),
                ..StreamConfig::default()
            },
            pitch: PitchDetectorConfig {
                min_frequency: 80.0,
                max_frequency: 800.0,
                ..PitchDetectorConfig::default()
            },
            spectrum_fft_size: 4096,
            spectrum_smoothing: 0.9,
            onset_fft_size: 2048,
            onset_threshold: 0.5,
        };

        assert_eq!(config.spectrum_fft_size, 4096);
        assert!((config.spectrum_smoothing - 0.9).abs() < f32::EPSILON);
        assert_eq!(config.onset_fft_size, 2048);
        assert!((config.onset_threshold - 0.5).abs() < f32::EPSILON);
        assert_eq!(config.stream.sample_rate, Some(48_000));
        assert_eq!(config.stream.buffer_size, Some(512));
        assert!((config.pitch.min_frequency - 80.0).abs() < f32::EPSILON);
        assert!((config.pitch.max_frequency - 800.0).abs() < f32::EPSILON);
    }
}
