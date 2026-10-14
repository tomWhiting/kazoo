//! Audio engine: real-time audio graph, buffer management, thread coordination.
//!
//! The engine orchestrates six threads:
//!
//! 1. **cpal input callback** (OS-managed) -- writes mic samples to ring buffer.
//! 2. **cpal output callback** (OS-managed) -- the main audio workhorse.
//!    Drains commands, reads mic from ring buffer, runs the mixer (synth +
//!    effects), applies the soft limiter, and writes directly to the output
//!    buffer. All processing state is owned by this callback's closure, and
//!    it never allocates, frees, locks or does I/O.
//! 3. **Analysis thread** (`kazoo-analysis`) -- runs spectrum and formants.
//! 4. **Disk I/O thread** (`kazoo-disk-io`) -- writes recorded audio to WAV files.
//! 5. **Reclaim thread** (`kazoo-reclaim`) -- frees everything the output
//!    callback lets go of, turns finished takes into clips and builds the
//!    timeline snapshots (see [`reclaim`]).
//! 6. **Judge thread** (`kazoo-judge`) -- runs pYIN and onset detection.
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
pub mod midi;
pub mod processing;
pub mod reclaim;
pub mod stats;

pub use command::EngineCommand;
pub use disk::DiskCommand;
pub use display::{
    ClipSnapshot, DisplayState, RecordingSpan, TimelineSnapshot, TrackClipSnapshot, TrackRecording,
};
pub use handle::{DeskLink, EngineHandle};
pub use processing::{create_synth, prepared_synth};
pub use reclaim::Parcel;
pub use stats::{EngineStats, EngineStatsSnapshot};

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use ringbuf::traits::{Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};

use crate::analysis::{FormantData, PitchDetectorConfig, VOICE_MAX_HZ, VOICE_MIN_HZ};
use crate::io::StreamConfig;
use crate::voice::judge::{self, OnsetConfig};
use crate::{Result, SPECTRUM_FFT_SIZE};

use analysis_thread::AnalysisConfig;
use handle::{DisplayLink, ThreadHandles};
use reclaim::{Inbound, Outbound, Reclaimer, TimelineMailbox};

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

    /// Voice judge configuration: 50 to 4000 Hz, 0.34-semitone bins and a
    /// 0.3 voicing threshold by default; rate, frame and hop follow the device.
    pub pitch: PitchDetectorConfig,

    /// FFT size for spectrum analysis (default 2048).
    pub spectrum_fft_size: usize,

    /// EMA smoothing factor for spectrum display (default 0.8).
    pub spectrum_smoothing: f32,

    /// FFT size for onset detection (default 1024).
    pub onset_fft_size: usize,

    /// Threshold factor for onset detection (default 0.3).
    pub onset_threshold: f32,

    /// Plug into the kazoo-mix desk as an instrument with this name
    /// (default `None`: standalone only).
    ///
    /// The engine's master output is then sent to the desk whenever the desk
    /// is running, and silenced locally while the desk plays it; the engine
    /// follows the desk's transport, and asks the desk to play, stop, pause
    /// or change tempo instead of doing so alone. With no desk running the
    /// engine plays locally as usual, and plugs in as soon as a desk appears.
    pub desk_instrument_name: Option<String>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            stream: StreamConfig::default(),
            pitch: PitchDetectorConfig::for_rate(44_100),
            spectrum_fft_size: SPECTRUM_FFT_SIZE,
            spectrum_smoothing: 0.8,
            onset_fft_size: 1024,
            onset_threshold: 0.3,
            desk_instrument_name: None,
        }
    }
}

/// Preserve the requested range and voicing threshold at the device's rate.
///
/// Use the voice judge's window, hop and resolution at the device's rate.
/// The cycle tracker supplies Hz, so the judge only rules on octave and
/// voicing: 0.34-semitone bins suffice, and a 512-sample hop at 48 kHz
/// brings a fresh verdict every 10.7 ms. Halve the hop only where pYIN
/// cannot hold it.
#[must_use]
pub fn judge_config(asked: &PitchDetectorConfig, rate: u32) -> PitchDetectorConfig {
    PitchDetectorConfig {
        min_frequency: asked.min_frequency,
        max_frequency: asked.max_frequency,
        voiced_threshold: asked.voiced_threshold,
        ..PitchDetectorConfig::for_rate(rate)
    }
    .with_hop_pyin_holds()
}

/// Refuse a judge that cannot hear every pitch the tracker can report.
/// Folding trusts the judge's octave, so its range must cover the tracker.
pub fn check_judge_range(asked: &PitchDetectorConfig) -> crate::Result<()> {
    if !(asked.min_frequency <= VOICE_MIN_HZ && asked.max_frequency >= VOICE_MAX_HZ) {
        return Err(crate::Error::Config(format!(
            "the judge's range {} to {} Hz must cover the tracker's 50 to 4000 Hz: \
             a narrower judge folds every note above its top down an octave",
            asked.min_frequency, asked.max_frequency
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Ring buffer capacity helpers
// ---------------------------------------------------------------------------

/// Compute capacities for all ring buffers based on buffer size.
struct RingBufferCapacities {
    /// Mic input: `buffer_size` * 4.
    mic: usize,
    /// Display state: 4 slots.
    display: usize,
    /// Display frames in circulation: one for every display slot, one held
    /// by the handle, one held by the callback, and one spare. The recycle
    /// ring holds them all.
    display_frames: usize,
    /// Analysis input: `buffer_size` * 4.
    analysis_input: usize,
    /// Analysis results (spectrum, formant): 32 slots each.
    analysis_results: usize,
    /// Disk recording: `buffer_size` * 32.
    disk: usize,
}

impl RingBufferCapacities {
    fn from_buffer_size(buffer_size: usize) -> Self {
        let bs = buffer_size.max(1);
        Self {
            // Mic and analysis use ×4: enough headroom for scheduling jitter
            // while keeping latency low (at 128 samples / 44.1 kHz ≈ 12 ms
            // maximum backlog). Disk stays at ×32 because writes are bursty
            // and latency-insensitive.
            mic: bs.saturating_mul(4),
            display: 4,
            display_frames: 4 + 3,
            analysis_input: bs.saturating_mul(4),
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
/// 1. Creates all inter-thread ring buffers.
/// 2. Builds audio I/O streams via `cpal`, moving all processing state into
///    the output callback closure.
/// 3. Spawns the analysis, judge and disk I/O threads.
/// 4. Returns an [`EngineHandle`] for the caller to send commands and poll
///    display state.
///
/// # Errors
///
/// Returns [`crate::Error::AudioDevice`] if audio streams cannot be created,
/// or [`crate::Error::Config`] for invalid configuration values (including
/// an invalid judge pitch range or onset setting).
pub fn start(config: EngineConfig) -> Result<EngineHandle> {
    check_judge_range(&config.pitch)?;
    // Keep the selected device and configuration until the streams open, so
    // processing and the device use the same rate and buffer size.
    let prepared = crate::io::device::prepare_output(&config.stream)?;
    let output_info = prepared.info();
    let sample_rate = output_info.rate;
    let stream_config = &config.stream;
    let buffer_size = output_info.frames;

    // Validate the judge's configuration up front: an invalid config is a
    // caller error, not something to discover later on the judge thread.
    let analysis = analysis_config(&config, sample_rate, buffer_size);
    let (judge_feed, judge_results, judge) = judge::spawn(
        judge_config(&config.pitch, sample_rate),
        buffer_size,
        onset_config(&config),
    )?;
    let stats = Arc::new(EngineStats::new());

    // The desk link is created before the streams so its audio half can be
    // moved into the output callback with the rest of the processing state.
    let desk_instrument_name = config.desk_instrument_name;
    let (desk, desk_audio) =
        start_desk_link(desk_instrument_name.as_deref(), sample_rate, buffer_size)?;

    // -----------------------------------------------------------------------
    // 1. Create ring buffers
    // -----------------------------------------------------------------------
    let caps = RingBufferCapacities::from_buffer_size(buffer_size);

    // Mic input: cpal input callback -> output callback.
    let (mic_prod, mic_cons) = HeapRb::<f32>::new(caps.mic).split();
    // Display frames: output callback -> UI thread, and back to be reused.
    let spectrum_len = config.spectrum_fft_size.max(2);
    let display = DisplayRings::new(&caps, sample_rate, spectrum_len)?;
    // Analysis input: output callback -> analysis thread.
    let (analysis_in_prod, analysis_in_cons) = HeapRb::<f32>::new(caps.analysis_input).split();
    // Analysis results: analysis thread -> output callback.
    let (spectrum_prod, spectrum_cons) = HeapRb::<Vec<f32>>::new(caps.analysis_results).split();
    let (formant_prod, formant_cons) =
        HeapRb::<Option<FormantData>>::new(caps.analysis_results).split();
    // Disk recording: output callback -> disk I/O thread.
    let (disk_prod, disk_cons) = HeapRb::<f32>::new(caps.disk).split();
    // Reclaim: output callback <-> reclaim thread, and timelines to the UI.
    // Started before the streams, so it is ready for the first block.
    let reclaim = ReclaimEnds::start()?;

    // -----------------------------------------------------------------------
    // 2. Create command channels (bounded per CLAUDE.md)
    // -----------------------------------------------------------------------
    let (command_tx, command_rx) = crossbeam_channel::bounded::<EngineCommand>(256);
    let (disk_cmd_tx, disk_cmd_rx) = crossbeam_channel::bounded::<DiskCommand>(64);

    // -----------------------------------------------------------------------
    // 3. Build audio streams
    // -----------------------------------------------------------------------
    // The input callback pushes mic samples to the ring buffer. The output
    // callback owns all processing state and does the actual audio work:
    // draining commands, running the mixer, applying effects, and writing
    // directly to the cpal output buffer.
    let input_callback = make_input_callback(
        mic_prod,
        usize::from(stream_config.input_channels.max(1)),
        buffer_size,
        Arc::clone(&stats),
    );

    // Create processing state and I/O handles. These are moved into the
    // output callback closure — the callback owns all processing state.
    // The disk command sender is cloned: the original stays with the
    // EngineHandle to send DiskCommand::Shutdown on drop.
    let mut proc_state = processing::ProcessingState::new(
        sample_rate,
        buffer_size,
        spectrum_len,
        Arc::clone(&stats),
    );
    let mut proc_io = processing::ProcessingIO {
        mic_cons,
        display_prod: display.frames_prod,
        display_recycle: display.recycle_cons,
        analysis_prod: analysis_in_prod,
        disk_prod,
        judge: judge_feed,
        judge_results,
        spectrum_cons,
        formant_cons,
        command_rx,
        disk_cmd_tx: disk_cmd_tx.clone(),
        reclaim: processing::ReclaimLink::new(reclaim.outbound, Arc::clone(&stats)),
        inbound: reclaim.inbound,
        desk: desk_audio,
    };

    // -----------------------------------------------------------------------
    // 4. Spawn analysis and disk I/O threads, before the streams start: if
    //    anything fails after this point, dropping the processing state ends
    //    them (their rings and channels disconnect), so nothing is left
    //    running.
    // -----------------------------------------------------------------------
    let analysis_handle = spawn_analysis_thread(
        analysis_thread::AnalysisIo {
            input_cons: analysis_in_cons,
            spectrum_prod,
            formant_prod,
            stats: Arc::clone(&stats),
        },
        analysis,
    )?;
    let disk_handle = spawn_disk_thread(disk_cons, disk_cmd_rx, sample_rate, Arc::clone(&stats))?;

    let streams = crate::io::device::build_streams_with_output(
        stream_config,
        prepared,
        input_callback,
        move |data| {
            // Output callback: run the entire audio processing pipeline.
            // ProcessingState and ProcessingIO are owned by this closure.
            processing::process_block(&mut proc_state, &mut proc_io, data);
        },
    )?;
    let (stream_holder_handle, stream_shutdown) = hold_streams(streams, &stats)?;

    // -----------------------------------------------------------------------
    // 5. Connect MIDI input (auto-discover first available device)
    // -----------------------------------------------------------------------
    let midi_handle = connect_midi(&command_tx, &stats);

    // -----------------------------------------------------------------------
    // 6. Build and return the engine handle
    // -----------------------------------------------------------------------
    let mut handle = EngineHandle::new(
        command_tx,
        DisplayLink {
            frames: display.frames_cons,
            recycle: display.recycle_prod,
            timelines: reclaim.timelines,
        },
        DisplayState::with_capacity(sample_rate, spectrum_len),
        sample_rate,
        buffer_size,
    );
    handle.set_stats(stats);
    handle.set_midi_handle(midi_handle);

    handle.set_thread_handles(ThreadHandles {
        analysis: analysis_handle,
        judge,
        disk: disk_handle,
        reclaim: reclaim.thread,
        stream_holder: stream_holder_handle,
        stream_shutdown,
        disk_cmd_tx,
    });

    handle.set_desk_link(desk);

    Ok(handle)
}

/// Keep the audio streams alive on a dedicated parked thread until the
/// returned flag is set (and the thread unparked).
///
/// `cpal::Stream` is `!Send` on some platforms, so the streams cannot be
/// stored in the [`EngineHandle`]; the flag lets the thread exit cleanly
/// when the handle is dropped.
fn hold_streams(
    streams: crate::io::AudioStreams,
    stats: &EngineStats,
) -> Result<(JoinHandle<()>, Arc<AtomicBool>)> {
    stats.set_audio_health([streams.input.health(), streams.output.health()])?;
    let stream_shutdown = Arc::new(AtomicBool::new(false));
    let shutdown = Arc::clone(&stream_shutdown);
    let holder = std::thread::Builder::new()
        .name("kazoo-streams".into())
        .spawn(move || {
            let _keep_alive = streams;
            while !shutdown.load(Ordering::Acquire) {
                std::thread::park();
            }
        })
        .map_err(|e| crate::Error::Stream(format!("failed to spawn streams holder: {e}")))?;
    Ok((holder, stream_shutdown))
}

/// Start the link to the kazoo-mix desk when `name` asks for one.
///
/// The engine's output is interleaved stereo, so the link is stereo; its
/// block size is the engine's processing block, which is the largest block
/// the output callback hands it. A link thread that cannot be started leaves
/// the engine standalone, with the reason kept for the UI.
///
/// # Errors
///
/// Returns [`crate::Error::Config`] if `buffer_size` does not fit the
/// link's `u32` frame count.
fn start_desk_link(
    name: Option<&str>,
    sample_rate: u32,
    buffer_size: usize,
) -> Result<(handle::DeskLink, Option<crate::ipc::link::HubLinkAudio>)> {
    let Some(name) = name else {
        return Ok((handle::DeskLink::Disabled, None));
    };
    let frames = u32::try_from(buffer_size)
        .map_err(|_| crate::Error::Config("buffer size too large for the desk link".into()))?;
    let link_config = crate::ipc::link::LinkConfig::new(name, 2, sample_rate, frames);
    Ok(start_desk_link_config(link_config))
}

fn start_desk_link_config(
    link_config: crate::ipc::link::LinkConfig,
) -> (handle::DeskLink, Option<crate::ipc::link::HubLinkAudio>) {
    match crate::host::link::connect_config_or_direct(link_config) {
        Ok((link, audio)) => (handle::DeskLink::Running(link), Some(audio)),
        Err(err) => (
            handle::DeskLink::Failed(format!("could not start the desk link: {err}")),
            None,
        ),
    }
}

/// Build the cpal input callback: push mic samples into the ring buffer as
/// mono, averaging across channels when the device delivers multi-channel
/// data.
///
/// The downmix scratch buffer is pre-allocated here and moved into the
/// closure, and the channel reciprocal is pre-computed — no allocation or
/// division in the hot path. Samples that do not fit in the ring buffer are
/// counted in [`EngineStats`].
fn make_input_callback(
    mut mic_prod: HeapProd<f32>,
    input_channels: usize,
    buffer_size: usize,
    stats: Arc<EngineStats>,
) -> impl FnMut(&[f32]) + Send + 'static {
    let input_channels = input_channels.max(1);
    let inv_ch = 1.0_f32 / input_channels as f32;
    let mut downmix_scratch = vec![0.0f32; buffer_size];

    move |data: &[f32]| {
        if input_channels <= 1 {
            let pushed = mic_prod.push_slice(data);
            stats.record_mic_push(data.len(), pushed);
            return;
        }
        let available_frames = data.len() / input_channels;
        let frames = available_frames.min(downmix_scratch.len());
        for (out, frame) in downmix_scratch
            .iter_mut()
            .zip(data.chunks_exact(input_channels))
        {
            *out = frame.iter().sum::<f32>() * inv_ch;
        }
        let pushed = mic_prod.push_slice(&downmix_scratch[..frames]);
        // Frames beyond the scratch capacity are lost too.
        stats.record_mic_push(available_frames, pushed);
    }
}

/// Spawn the analysis thread (`kazoo-analysis`).
fn spawn_analysis_thread(
    io: analysis_thread::AnalysisIo,
    config: AnalysisConfig,
) -> Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("kazoo-analysis".into())
        .spawn(move || {
            analysis_thread::run(io, &config);
        })
        .map_err(|e| crate::Error::Stream(format!("failed to spawn analysis thread: {e}")))
}

/// The caller's onset settings, as the judge takes them.
const fn onset_config(config: &EngineConfig) -> OnsetConfig {
    OnsetConfig {
        onset_fft_size: config.onset_fft_size,
        onset_threshold: config.onset_threshold,
    }
}

/// The analysis pipeline's configuration for an engine running at
/// `sample_rate` with `buffer_size`-sample blocks.
const fn analysis_config(
    config: &EngineConfig,
    sample_rate: u32,
    buffer_size: usize,
) -> AnalysisConfig {
    AnalysisConfig {
        spectrum_fft_size: config.spectrum_fft_size,
        spectrum_smoothing: config.spectrum_smoothing,
        sample_rate,
        buffer_size,
    }
}

/// Connect the first available MIDI input, if any, sending its events as
/// engine commands.
fn connect_midi(
    command_tx: &crossbeam_channel::Sender<EngineCommand>,
    stats: &Arc<EngineStats>,
) -> Option<midi::MidiHandle> {
    let midi_handle = midi::connect_first_port(command_tx.clone(), Arc::clone(stats));
    if let Some(ref mh) = midi_handle {
        eprintln!("MIDI connected: {}", mh.port_name());
    }
    midi_handle
}

/// The display path's rings: frames from the output callback to the UI, and
/// finished frames back to be written again.
struct DisplayRings {
    frames_prod: HeapProd<DisplayState>,
    frames_cons: HeapCons<DisplayState>,
    recycle_prod: HeapProd<DisplayState>,
    recycle_cons: HeapCons<DisplayState>,
}

impl DisplayRings {
    /// Create the rings and every display frame in circulation, each sized
    /// for everything the callback writes (`spectrum_len` spectrum bins).
    /// All but the one the handle starts with wait in the recycle ring.
    fn new(caps: &RingBufferCapacities, sample_rate: u32, spectrum_len: usize) -> Result<Self> {
        let (frames_prod, frames_cons) = HeapRb::<DisplayState>::new(caps.display).split();
        let (mut recycle_prod, recycle_cons) =
            HeapRb::<DisplayState>::new(caps.display_frames).split();
        for _ in 1..caps.display_frames {
            if recycle_prod
                .try_push(DisplayState::with_capacity(sample_rate, spectrum_len))
                .is_err()
            {
                return Err(crate::Error::Config(
                    "display recycle ring is smaller than the frames in circulation".into(),
                ));
            }
        }
        Ok(Self {
            frames_prod,
            frames_cons,
            recycle_prod,
            recycle_cons,
        })
    }
}

/// The output callback's ends of the reclaim rings, the handle's timeline
/// receiver, and the running reclaim thread (`kazoo-reclaim`).
struct ReclaimEnds {
    outbound: HeapProd<Outbound>,
    inbound: HeapCons<Inbound>,
    timelines: TimelineMailbox,
    thread: JoinHandle<()>,
}

impl ReclaimEnds {
    /// Create the rings and start the reclaim thread. It exits once the
    /// output callback (and with it the outbound ring's producer) is gone.
    fn start() -> Result<Self> {
        let (outbound, outbound_cons) = HeapRb::<Outbound>::new(reclaim::OUTBOUND_CAPACITY).split();
        let (inbound_prod, inbound) = HeapRb::<Inbound>::new(reclaim::INBOUND_CAPACITY).split();
        let timelines = TimelineMailbox::default();
        let reclaimer = Reclaimer::new(outbound_cons, inbound_prod, timelines.clone());
        let thread = std::thread::Builder::new()
            .name("kazoo-reclaim".into())
            .spawn(move || reclaimer.run())
            .map_err(|e| crate::Error::Stream(format!("failed to spawn reclaim thread: {e}")))?;
        Ok(Self {
            outbound,
            inbound,
            timelines,
            thread,
        })
    }
}

/// Spawn the disk I/O thread (`kazoo-disk-io`).
fn spawn_disk_thread(
    disk_cons: HeapCons<f32>,
    disk_cmd_rx: crossbeam_channel::Receiver<DiskCommand>,
    sample_rate: u32,
    stats: Arc<EngineStats>,
) -> Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("kazoo-disk-io".into())
        .spawn(move || {
            disk::run(disk_cons, &disk_cmd_rx, sample_rate, &stats);
        })
        .map_err(|e| crate::Error::Stream(format!("failed to spawn disk I/O thread: {e}")))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::judge::{JudgeResult, JudgeResults};
    use std::time::{Duration, Instant};

    /// The judge's next pitch verdict, skipping onsets; fails after five seconds.
    fn next_pitch(results: &mut JudgeResults) -> crate::analysis::PitchEstimate {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match results.pop() {
                Some(JudgeResult::Pitch(estimate)) => return estimate,
                Some(JudgeResult::Onset { .. }) => {}
                None => {
                    assert!(Instant::now() < deadline, "the judge gave no verdict");
                    std::thread::yield_now();
                }
            }
        }
    }

    /// A 48 kHz sine judged at the default 44.1 kHz rate used to read about 404 Hz.
    #[test]
    fn the_engine_judges_at_the_device_rate() {
        let config = judge_config(&EngineConfig::default().pitch, 48_000);
        assert_eq!(config.sample_rate, 48_000);
        assert_eq!(config.frame_length, 2048);
        assert_eq!(config.hop_length, 2048);
        let (mut feed, mut results, judge) =
            judge::spawn(config, 128, OnsetConfig::default()).expect("valid device judge");
        let samples: Vec<_> = (0..48_000_u32)
            .map(|index| (std::f64::consts::TAU * 440.0 * f64::from(index) / 48_000.0).sin() as f32)
            .collect();
        // One window at a time, each judged before the next is fed, so the
        // judge's input ring never overflows.
        let mut settled = 0;
        let mut estimates = 0;
        let mut windows = samples.chunks_exact(2048);
        for (index, window) in windows.by_ref().enumerate() {
            feed.push(window);
            let estimate = next_pitch(&mut results);
            estimates += 1;
            if (index + 1) * 2048 > 4096 {
                let frequency = estimate.frequency.expect("settled sine must be voiced");
                let cents = (1200.0 * (frequency / 440.0).log2()).abs();
                assert!(
                    cents <= 5.0,
                    "device-rate sine read {frequency} Hz ({cents} cents)"
                );
                settled += 1;
            }
        }
        feed.push(windows.remainder());
        drop(feed);
        let stats = judge.stats();
        judge.join().expect("the judge exits once its feed is gone");
        while let Some(result) = results.pop() {
            assert!(
                !matches!(result, JudgeResult::Pitch(_)),
                "a partial window was judged"
            );
        }
        assert_eq!(estimates, 23, "one estimate per device-rate window");
        assert!(settled > 0);
        assert_eq!(stats.samples_dropped, 0);
        assert_eq!(stats.results_dropped, 0);
    }

    /// Keep the voice judge's hop at every rate where pYIN holds it, and
    /// shorten it only when a low rate makes the transition band too wide.
    #[test]
    fn the_engine_builds_its_judge_at_every_device_rate() {
        for rate in [
            8000, 11_025, 12_000, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000, 88_200, 96_000,
            176_400, 192_000, 384_000,
        ] {
            let config = judge_config(&EngineConfig::default().pitch, rate);
            assert!((config.max_frequency - VOICE_MAX_HZ).abs() < f32::EPSILON);
            let shown = format!("{config:?}");
            let once_per_window = config.hop_length == config.frame_length;
            let (feed, _results, judge) =
                judge::spawn(config, 128, onset_config(&EngineConfig::default()))
                    .unwrap_or_else(|error| panic!("rate {rate} was refused: {shown} {error}"));
            drop(feed);
            judge.join().expect("the judge exits once its feed is gone");
            if rate >= 22_050 {
                assert!(once_per_window, "rate {rate} must run once per window");
            }
        }
    }

    #[test]
    fn engine_config_default_values() {
        let config = EngineConfig::default();
        assert!((config.pitch.min_frequency - 50.0).abs() < f32::EPSILON);
        assert!((config.pitch.max_frequency - 4000.0).abs() < f32::EPSILON);
        assert!(
            (config.pitch.resolution - PitchDetectorConfig::for_rate(44_100).resolution).abs()
                < f64::EPSILON
        );
        assert_eq!(config.spectrum_fft_size, SPECTRUM_FFT_SIZE);
        assert!((config.spectrum_smoothing - 0.8).abs() < f32::EPSILON);
        assert_eq!(config.onset_fft_size, 1024);
        assert!((config.onset_threshold - 0.3).abs() < f32::EPSILON);
        assert_eq!(config.desk_instrument_name, None);
    }

    #[test]
    fn desk_link_disabled_without_a_name() {
        let (desk, audio) = start_desk_link(None, 48_000, 128).unwrap();
        assert!(matches!(desk, handle::DeskLink::Disabled));
        assert!(audio.is_none());
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
        let caps = RingBufferCapacities::from_buffer_size(128);
        assert_eq!(caps.mic, 128 * 4);
        assert_eq!(caps.display, 4);
        assert!(caps.display_frames > caps.display + 1);
        assert_eq!(caps.analysis_input, 128 * 4);
        assert_eq!(caps.analysis_results, 32);
        assert_eq!(caps.disk, 128 * 32);
    }

    #[test]
    fn ring_buffer_capacities_large_buffer_size() {
        let caps = RingBufferCapacities::from_buffer_size(1024);
        assert_eq!(caps.mic, 1024 * 4);
        assert_eq!(caps.analysis_input, 1024 * 4);
        assert_eq!(caps.disk, 1024 * 32);
    }

    #[test]
    fn ring_buffer_capacities_zero_buffer_size_uses_one() {
        let caps = RingBufferCapacities::from_buffer_size(0);
        assert_eq!(caps.mic, 4);
        assert_eq!(caps.analysis_input, 4);
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
            desk_instrument_name: Some("kazoo-tui".into()),
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

#[cfg(test)]
mod desk_link_tests {
    #[test]
    fn engine_reports_no_desk_through_shared_setup() {
        let directory = tempfile::tempdir().unwrap();
        let config = crate::ipc::link::LinkConfig {
            address: crate::ipc::link::HubAddress::Socket(directory.path().join("missing.sock")),
            ..crate::ipc::link::LinkConfig::new("kazoo-tui", 2, 48_000, 256)
        };
        let (desk, audio) = super::start_desk_link_config(config);
        let super::handle::DeskLink::Running(link) = desk else {
            panic!("link must run direct");
        };
        assert_eq!(
            link.status().last_refusal.as_deref(),
            Some("no desk; playing direct")
        );
        drop(audio);
        drop(link);
    }
}
