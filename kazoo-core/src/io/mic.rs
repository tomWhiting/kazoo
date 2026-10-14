//! Native-rate microphone capture and recovery, feeding engine-rate mono.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use audioadapter_buffers::direct::SequentialSliceOfVecs;
use cpal::traits::DeviceTrait;
use crossbeam_channel::{Receiver, Sender, TrySendError};
use ringbuf::HeapProd;
use ringbuf::traits::Producer;
use rubato::{
    Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};

mod watch;

use super::channel_adapt::InputAdapter;
use crate::host::audio::{DeviceChoice, InputNotices, OpenedInput, OutputRequest, RateChoice};
use crate::{Error, Result, sanitize_sample};

/// What to open: which device, and which of its channels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicRequest {
    pub device: DeviceChoice,
    /// `None`: the mean of every channel. `Some(n)`: channel n, 1-based.
    pub channel: Option<u16>,
}

/// The engine side the mic feeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MicTarget {
    pub rate: u32,
    pub frames: usize,
}

pub const MIC_MIN_RATE: u32 = 8_000;
pub const MIC_MAX_RATE: u32 = 192_000;
pub const SINC_LEN: usize = 64;
pub const OVERSAMPLING: usize = 256;
pub const HEALTH_EVERY: Duration = Duration::from_millis(250);
pub const FACTS_EVERY: Duration = Duration::from_secs(1);
/// The longest wait between asking a device whose frames stay off its rate
/// for facts that never change: asking creates host audio units.
pub const RECHECK_MOST: Duration = Duration::from_secs(32);
pub const RELEASE_WAIT: Duration = Duration::from_secs(1);
/// Device names are cut to this many bytes (at a char boundary) in the status.
pub const NAME_BYTES: usize = 48;
const CAPTURE_FRAMES: usize = 4_096;
const LIST_HINT: &str = " (kazoo mouth --list-inputs lists the inputs)";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MicState {
    /// No mic was opened (including engines constructed by tests).
    #[default]
    Off,
    /// Running.
    Live,
    /// The device went away or failed; searches back off up to 32 seconds.
    Lost,
    /// A dropped stream never gave the ring back; no further reopening.
    Stuck,
}

/// A device name held in a fixed array, so the status is `Copy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MicName {
    bytes: [u8; NAME_BYTES],
    len: u8,
}

impl Default for MicName {
    fn default() -> Self {
        Self {
            bytes: [0; NAME_BYTES],
            len: 0,
        }
    }
}

impl MicName {
    #[must_use]
    pub fn new(name: &str) -> Self {
        let mut end = name.len().min(NAME_BYTES);
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        let mut result = Self::default();
        for (out, byte) in result.bytes.iter_mut().zip(&name.as_bytes()[..end]) {
            *out = *byte;
            result.len += 1;
        }
        result
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        // Only new() writes these bytes, and it copies whole UTF-8 characters.
        std::str::from_utf8(&self.bytes[..usize::from(self.len)]).unwrap_or("<invalid mic name>")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MicStatus {
    pub state: MicState,
    pub device: MicName,
    pub device_rate: u32,
    pub device_channels: u16,
    pub channel: Option<u16>,
    pub engine_rate: u32,
    /// Times the mic was opened again after the first.
    pub reopens: u32,
    /// `BufferUnderrun` reports from the host, not losses.
    pub glitches: u32,
    /// Resampler errors: the chunk is dropped and the resampler reset.
    pub resample_errors: u32,
}

/// Status shared between the supervisor and UI; callbacks touch only atomics.
#[derive(Debug, Default)]
pub struct MicShared {
    status: Mutex<MicStatus>,
    input_health: Mutex<Option<crate::host::audio::StreamHealth>>,
    resampler_delay: AtomicU64,
    glitches: AtomicU32,
    resample_errors: AtomicU32,
    /// Device frames the callback has been handed, for the supervisor to
    /// hold against the rate the mic was opened at.
    frames: AtomicU64,
}

impl MicShared {
    #[must_use]
    pub fn status(&self) -> MicStatus {
        let mut status = *self
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        status.glitches = self.glitches.load(Ordering::Relaxed);
        status.resample_errors = self.resample_errors.load(Ordering::Relaxed);
        status
    }

    fn publish(&self, status: MicStatus) {
        *self
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = status;
    }
}

/// Called after each push with (offered, accepted) engine-rate samples.
pub type PushCount = Arc<dyn Fn(usize, usize) + Send + Sync>;

/// The running mic: dropping it stops and joins its supervisor thread.
pub struct Mic {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    shared: Arc<MicShared>,
    notices: InputNotices,
}

impl std::fmt::Debug for Mic {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Mic")
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

impl Mic {
    #[must_use]
    pub fn input_latency_ms(&self) -> Option<f64> {
        if self.status().state != MicState::Live {
            return None;
        }
        self.shared
            .input_health
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(crate::host::audio::StreamHealth::latency_ms)
    }

    #[must_use]
    pub fn resampler_delay_frames(&self) -> Option<usize> {
        (self.status().state == MicState::Live)
            .then(|| self.shared.resampler_delay.load(Ordering::Relaxed) as usize)
    }

    #[must_use]
    pub const fn input_notices(&self) -> &InputNotices {
        &self.notices
    }

    #[must_use]
    pub fn shared(&self) -> Arc<MicShared> {
        Arc::clone(&self.shared)
    }

    #[must_use]
    pub fn status(&self) -> MicStatus {
        self.shared.status()
    }
}

impl Drop for Mic {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            if let Err(panic) = thread.join() {
                let mut status = self.shared.status();
                status.state = MicState::Stuck;
                self.shared.publish(status);
                drop(panic);
            }
        }
    }
}

/// Open at the device's own rate, returning the first error before startup.
pub fn open_mic(
    request: MicRequest,
    target: MicTarget,
    sink: HeapProd<f32>,
    counted: PushCount,
) -> Result<Mic> {
    open_mic_with_notices(request, target, sink, counted, InputNotices::default())
}

pub fn open_mic_with_notices(
    request: MicRequest,
    target: MicTarget,
    sink: HeapProd<f32>,
    counted: PushCount,
    notices: InputNotices,
) -> Result<Mic> {
    if target.rate == 0 || target.frames == 0 {
        return Err(Error::Config(
            "mic target rate and frames must be non-zero".into(),
        ));
    }
    let shared = Arc::new(MicShared::default());
    let stop = Arc::new(AtomicBool::new(false));
    let thread_shared = Arc::clone(&shared);
    let thread_stop = Arc::clone(&stop);
    let (started, ready) = crossbeam_channel::bounded(1);
    let backend = CpalMic {
        notices: notices.clone(),
        ..CpalMic::default()
    };
    let thread = std::thread::Builder::new()
        .name("kazoo-mic".into())
        .spawn(move || {
            // Stream creation and destruction stay on the owning thread.
            let mut supervisor =
                Supervisor::new(backend, request, target, sink, counted, thread_shared);
            let result = supervisor.start();
            let running = result.is_ok();
            if let Err(error) = started.send(result) {
                drop(error);
                return;
            }
            if !running {
                return;
            }
            while !thread_stop.load(Ordering::Acquire) {
                supervisor.tick(Instant::now());
                std::thread::park_timeout(HEALTH_EVERY);
            }
        })
        .map_err(|error| Error::Stream(format!("failed to spawn mic supervisor: {error}")))?;
    let mic = Mic {
        stop,
        thread: Some(thread),
        shared,
        notices,
    };
    ready.recv().map_err(|error| {
        Error::Stream(format!("mic supervisor stopped during startup: {error}"))
    })??;
    Ok(mic)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MicFacts {
    pub name: String,
    pub rate: u32,
    pub channels: u16,
}

#[derive(Debug)]
pub(crate) enum MicFault {
    Lost(String),
    Glitch,
}

pub(crate) trait MicBackend: Send + 'static {
    type Stream;
    /// The device's facts. A backend may answer from what it read before,
    /// for as long as the device keeps its identity.
    fn facts(&mut self, device: &DeviceChoice) -> Result<MicFacts>;
    /// Drop what `facts` remembers, so its next answer asks the device.
    fn forget(&mut self);
    fn open(
        &mut self,
        device: &DeviceChoice,
        facts: &MicFacts,
        frames: usize,
        capture: MicCapture,
    ) -> Result<Self::Stream>;
    fn fault(&mut self, stream: &Self::Stream) -> Option<MicFault>;
}

#[derive(Default)]
struct CpalMic {
    facts: watch::FactsCache<cpal::DeviceId>,
    named: Option<cpal::Device>,
    notices: InputNotices,
}

impl std::fmt::Debug for CpalMic {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CpalMic")
            .field("facts", &self.facts)
            .finish_non_exhaustive()
    }
}

impl MicBackend for CpalMic {
    type Stream = OpenedInput;

    fn facts(&mut self, device: &DeviceChoice) -> Result<MicFacts> {
        let name = match device {
            DeviceChoice::Default => None,
            DeviceChoice::Named(name) => Some(name.as_str()),
        };
        let device = if let Some(name) = name {
            if let Some(device) = &self.named {
                device.clone()
            } else {
                let device = crate::host::audio::resolve_named_input(
                    &cpal::default_host(),
                    name,
                    &self.notices,
                )?;
                self.named = Some(device.clone());
                device
            }
        } else {
            super::device::resolve_input_device(&cpal::default_host(), None)?
        };
        let identity = device
            .id()
            .map_err(|error| Error::Device(format!("cannot identify input: {error}")))?;
        // Configuration and description probes create host audio units; only probe a changed device.
        self.facts.read(identity, || {
            // The picked mic itself is not left out: its failure is a loss.
            let description = device
                .description()
                .map_err(|error| Error::Device(format!("cannot name input: {error}")))?;
            let config = device.default_input_config().map_err(|error| {
                Error::Device(format!(
                    "input device '{}': cannot read native configuration: {error}",
                    description.name()
                ))
            })?;
            Ok(MicFacts {
                name: description.name().to_owned(),
                rate: config.sample_rate(),
                channels: config.channels(),
            })
        })
    }

    fn forget(&mut self) {
        self.facts.invalidate();
        self.named = None;
    }

    fn open(
        &mut self,
        device: &DeviceChoice,
        facts: &MicFacts,
        frames: usize,
        mut capture: MicCapture,
    ) -> Result<Self::Stream> {
        let prepared = OutputRequest {
            device: device.clone(),
            channels: facts.channels,
            rate: RateChoice::Native,
        }
        .prepare_input_with_frames(frames)?;
        let info = prepared.info();
        self.facts.invalidate();
        if info.rate != facts.rate
            || info.channels != facts.channels
            || self.facts(device)? != *facts
        {
            return Err(Error::Device(format!(
                "input device '{}': native configuration changed while opening ({} Hz, {} channels)",
                facts.name, info.rate, info.channels
            )));
        }
        let shared = Arc::clone(&capture.shared);
        let stream = prepared.open(move |data, info| capture.process(data, info.channels))?;
        *shared
            .input_health
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(stream.health());
        Ok(stream)
    }

    fn fault(&mut self, stream: &Self::Stream) -> Option<MicFault> {
        let mut fault = None;
        let outcome = match stream.check_error_with_handler(|error| {
            fault = Some(match error {
                cpal::StreamError::BufferUnderrun => MicFault::Glitch,
                other => MicFault::Lost(other.to_string()),
            });
        }) {
            Ok(()) => None,
            Err(error) => Some(fault.unwrap_or_else(|| MicFault::Lost(error.to_string()))),
        };
        // A loss reaches the supervisor, whose `lose` makes this backend forget.
        outcome
    }
}

/// Owns the ring producer until the stream releases its callback.
pub(crate) struct MicCapture {
    sink: Option<HeapProd<f32>>,
    home: Sender<HeapProd<f32>>,
    counted: PushCount,
    channel: Option<u16>,
    adapter: InputAdapter,
    pick: Vec<f32>,
    resampler: Option<MicResampler>,
    shared: Arc<MicShared>,
}

impl MicCapture {
    fn process(&mut self, data: &[f32], device_channels: u16) {
        let channels = usize::from(device_channels.max(1));
        let Self {
            sink,
            counted,
            channel,
            adapter,
            pick,
            resampler,
            shared,
            ..
        } = self;
        shared.frames.fetch_add(
            u64::try_from(data.len() / channels).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        let Some(sink) = sink.as_mut() else {
            shared.resample_errors.fetch_add(1, Ordering::Relaxed);
            return;
        };
        if let Some(channel) = channel {
            // The channel was checked at open; guard malformed callback widths.
            let Some(index) = channel
                .checked_sub(1)
                .map(usize::from)
                .filter(|&n| n < channels)
            else {
                shared.resample_errors.fetch_add(1, Ordering::Relaxed);
                return;
            };
            for pass in data.chunks(CAPTURE_FRAMES * channels) {
                let frames = pass.len() / channels;
                for (out, frame) in pick[..frames].iter_mut().zip(pass.chunks_exact(channels)) {
                    *out = capture_sample(frame[index]);
                }
                push_mono(sink, counted, resampler.as_mut(), &pick[..frames]);
            }
        } else {
            let mut consume = |mono: &[f32]| {
                for pass in mono.chunks(CAPTURE_FRAMES) {
                    let scratch = &mut pick[..pass.len()];
                    for (out, sample) in scratch.iter_mut().zip(pass) {
                        *out = capture_sample(*sample);
                    }
                    push_mono(sink, counted, resampler.as_mut(), scratch);
                }
            };
            adapter.process(data, device_channels, &mut consume);
        }
    }
}

/// Reject impossible magnitudes before they contaminate the filter's history.
const fn capture_sample(sample: f32) -> f32 {
    let sample = sanitize_sample(sample);
    if sample.abs() > 1.0e20 { 0.0 } else { sample }
}

fn push_mono(
    sink: &mut HeapProd<f32>,
    counted: &PushCount,
    resampler: Option<&mut MicResampler>,
    mono: &[f32],
) {
    let mut push = |samples: &[f32]| {
        let accepted = sink.push_slice(samples);
        counted(samples.len(), accepted);
    };
    if let Some(resampler) = resampler {
        resampler.push(mono, push);
    } else {
        push(mono);
    }
}

impl Drop for MicCapture {
    fn drop(&mut self) {
        if let Some(sink) = self.sink.take() {
            match self.home.try_send(sink) {
                Ok(()) => {}
                Err(TrySendError::Full(sink) | TrySendError::Disconnected(sink)) => drop(sink),
            }
        }
    }
}

struct MicResampler {
    resampler: Async<f32>,
    input: Vec<Vec<f32>>,
    filled: usize,
    output: Vec<Vec<f32>>,
    shared: Arc<MicShared>,
}

impl std::fmt::Debug for MicResampler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MicResampler")
            .field("delay_frames", &self.delay_frames())
            .field("filled", &self.filled)
            .finish_non_exhaustive()
    }
}

impl MicResampler {
    fn new(mic_rate: u32, engine_rate: u32, shared: Arc<MicShared>) -> Result<Self> {
        let chunk = rounded_frames(1, mic_rate, 1_000, 32, 256)?;
        let params = SincInterpolationParameters {
            sinc_len: SINC_LEN,
            f_cutoff: rubato::calculate_cutoff(SINC_LEN, WindowFunction::BlackmanHarris2),
            oversampling_factor: OVERSAMPLING,
            interpolation: SincInterpolationType::Cubic,
            window: WindowFunction::BlackmanHarris2,
        };
        let resampler = Async::<f32>::new_sinc(
            f64::from(engine_rate) / f64::from(mic_rate),
            1.0,
            &params,
            chunk,
            1,
            FixedAsync::Input,
        )
        .map_err(|error| Error::AudioFormat(format!("cannot build mic resampler: {error}")))?;
        let output_frames = resampler.output_frames_max();
        Ok(Self {
            resampler,
            input: vec![vec![0.0; chunk]],
            filled: 0,
            output: vec![vec![0.0; output_frames]],
            shared,
        })
    }

    fn push(&mut self, mut mono: &[f32], mut out: impl FnMut(&[f32])) {
        let chunk = self.input[0].len();
        while !mono.is_empty() {
            let take = mono.len().min(chunk - self.filled);
            self.input[0][self.filled..self.filled + take].copy_from_slice(&mono[..take]);
            self.filled += take;
            mono = &mono[take..];
            if self.filled == chunk {
                if let Some(written) = self.process_chunk() {
                    let samples = &mut self.output[0][..written];
                    for sample in samples.iter_mut() {
                        *sample = sanitize_sample(*sample);
                    }
                    out(samples);
                }
                self.filled = 0;
            }
        }
    }

    fn process_chunk(&mut self) -> Option<usize> {
        let result = (|| {
            let input =
                SequentialSliceOfVecs::new(&self.input, 1, self.input[0].len()).map_err(drop)?;
            let frames = self.output[0].len();
            let mut output =
                SequentialSliceOfVecs::new_mut(&mut self.output, 1, frames).map_err(drop)?;
            self.resampler
                .process_into_buffer(&input, &mut output, None)
                .map_err(drop)
        })();
        if let Ok((_, written)) = result {
            Some(written)
        } else {
            self.shared.resample_errors.fetch_add(1, Ordering::Relaxed);
            self.resampler.reset();
            None
        }
    }

    fn delay_frames(&self) -> usize {
        self.resampler.output_delay()
    }
}

/// Integer rounding avoids lossy conversions of frame counts on the callback path.
fn rounded_frames(
    frames: usize,
    numerator: u32,
    denominator: u32,
    min: usize,
    max: usize,
) -> Result<usize> {
    if denominator == 0 {
        return Err(Error::Config(
            "mic frame conversion needs a non-zero rate".into(),
        ));
    }
    let frames = u64::try_from(frames)
        .map_err(|error| Error::Config(format!("mic frame count too large: {error}")))?;
    let product = u128::from(frames) * u128::from(numerator);
    let denominator = u128::from(denominator);
    let rounded = (product + denominator / 2) / denominator;
    let min = u64::try_from(min)
        .map_err(|error| Error::Config(format!("mic minimum frame count too large: {error}")))?;
    let max = u64::try_from(max)
        .map_err(|error| Error::Config(format!("mic maximum frame count too large: {error}")))?;
    let bounded = rounded.clamp(u128::from(min), u128::from(max));
    usize::try_from(bounded)
        .map_err(|error| Error::Config(format!("mic frame count too large: {error}")))
}

struct Supervisor<B: MicBackend> {
    backend: B,
    request: MicRequest,
    target: MicTarget,
    stream: Option<B::Stream>,
    sink: Option<HeapProd<f32>>,
    home: Sender<HeapProd<f32>>,
    returned: Receiver<HeapProd<f32>>,
    counted: PushCount,
    shared: Arc<MicShared>,
    status: MicStatus,
    last_facts: Instant,
    ever_opened: bool,
    opened_facts: Option<MicFacts>,
    last_error: Option<String>,
    release_wait: Duration,
    /// Since when, and from what count, the frames are being measured.
    delivery: (Instant, u64),
    /// When frames off the rate may next send the supervisor to the device.
    recheck_at: Instant,
    recheck_wait: Duration,
    search_at: Instant,
    search_wait: Duration,
}

impl<B: MicBackend> Supervisor<B> {
    fn new(
        backend: B,
        request: MicRequest,
        target: MicTarget,
        sink: HeapProd<f32>,
        counted: PushCount,
        shared: Arc<MicShared>,
    ) -> Self {
        let (home, returned) = crossbeam_channel::bounded(1);
        let now = Instant::now();
        Self {
            backend,
            request,
            target,
            stream: None,
            sink: Some(sink),
            home,
            returned,
            counted,
            shared,
            status: MicStatus::default(),
            last_facts: now,
            ever_opened: false,
            opened_facts: None,
            last_error: None,
            release_wait: RELEASE_WAIT,
            delivery: (now, 0),
            recheck_at: now,
            recheck_wait: FACTS_EVERY,
            search_at: now + FACTS_EVERY,
            search_wait: FACTS_EVERY,
        }
    }

    fn start(&mut self) -> Result<()> {
        let facts = self
            .backend
            .facts(&self.request.device)
            .map_err(|error| Error::Device(format!("{error}{LIST_HINT}")))?;
        let now = self.last_facts;
        self.open_facts(&facts, now).map_err(|error| match error {
            Error::Device(reason) => Error::Device(format!("{reason}{LIST_HINT}")),
            other => Error::Device(format!("input device '{}': {other}{LIST_HINT}", facts.name)),
        })?;
        self.shared.publish(self.status);
        Ok(())
    }

    fn open_facts(&mut self, facts: &MicFacts, now: Instant) -> Result<()> {
        if !(MIC_MIN_RATE..=MIC_MAX_RATE).contains(&facts.rate) {
            return Err(Error::Device(format!(
                "input device '{}' runs at {} Hz, which kazoo cannot resample (8000 to 192000)",
                facts.name, facts.rate
            )));
        }
        if facts.channels == 0 {
            return Err(Error::Device(format!(
                "input device '{}' has no input channels",
                facts.name
            )));
        }
        if let Some(channel) = self.request.channel {
            if channel == 0 || channel > facts.channels {
                return Err(Error::Device(format!(
                    "input device '{}' has {} channels; channel {channel} was asked for",
                    facts.name, facts.channels
                )));
            }
        }
        let frames = rounded_frames(self.target.frames, facts.rate, self.target.rate, 32, 1_024)?;
        let resampler = if facts.rate == self.target.rate {
            None
        } else {
            Some(MicResampler::new(
                facts.rate,
                self.target.rate,
                Arc::clone(&self.shared),
            )?)
        };
        self.shared.resampler_delay.store(
            resampler.as_ref().map_or(0, MicResampler::delay_frames) as u64,
            Ordering::Relaxed,
        );
        let sink = self
            .sink
            .take()
            .ok_or_else(|| Error::Stream("mic ring producer has not returned".into()))?;
        let capture = MicCapture {
            sink: Some(sink),
            home: self.home.clone(),
            counted: Arc::clone(&self.counted),
            channel: self.request.channel,
            adapter: InputAdapter::new(1, CAPTURE_FRAMES),
            pick: vec![0.0; CAPTURE_FRAMES],
            resampler,
            shared: Arc::clone(&self.shared),
        };
        match self
            .backend
            .open(&self.request.device, facts, frames, capture)
        {
            Ok(stream) => self.stream = Some(stream),
            Err(error) => {
                self.release();
                return Err(error);
            }
        }
        self.status = MicStatus {
            state: MicState::Live,
            device: MicName::new(&facts.name),
            device_rate: facts.rate,
            device_channels: facts.channels,
            channel: self.request.channel,
            engine_rate: self.target.rate,
            reopens: self
                .status
                .reopens
                .saturating_add(u32::from(self.ever_opened)),
            ..self.status
        };
        self.ever_opened = true;
        self.opened_facts = Some(facts.clone());
        self.last_error = None;
        self.delivery = (now, self.shared.frames.load(Ordering::Relaxed));
        self.recheck_at = now;
        self.recheck_wait = FACTS_EVERY;
        Ok(())
    }

    fn release(&mut self) {
        drop(self.stream.take());
        if self.sink.is_none() {
            match self.returned.recv_timeout(self.release_wait) {
                Ok(sink) => self.sink = Some(sink),
                Err(error) => {
                    self.status.state = MicState::Stuck;
                    self.last_error = Some(format!(
                        "mic stream did not release its ring producer: {error}"
                    ));
                }
            }
        }
    }

    fn lose(&mut self, now: Instant, reason: String) {
        // A device that went may come back as another object: look it up afresh.
        self.backend.forget();
        self.last_error = Some(reason);
        self.status.state = MicState::Lost;
        self.last_facts = now;
        // The search goes on from the wait it reached unless the mic has
        // since delivered at its rate: a mic that drops as soon as it opens
        // would otherwise be reopened, host audio units and all, every second.
        self.search_at = now + self.search_wait;
        self.release();
    }

    fn tick(&mut self, now: Instant) {
        match self.status.state {
            MicState::Live => {
                while let Some(stream) = self.stream.as_ref() {
                    match self.backend.fault(stream) {
                        Some(MicFault::Glitch) => {
                            self.shared.glitches.fetch_add(1, Ordering::Relaxed);
                        }
                        Some(MicFault::Lost(reason)) => {
                            self.lose(now, reason);
                            break;
                        }
                        None => break,
                    }
                }
                if self.status.state == MicState::Live
                    && now.duration_since(self.last_facts) >= FACTS_EVERY
                {
                    self.last_facts = now;
                    if self.delivery_strays(now) {
                        // The device's own facts are remembered while it keeps
                        // its identity; a headset changing mode keeps it, and
                        // only its frames say so. Ask it again.
                        self.backend.forget();
                    }
                    match self.backend.facts(&self.request.device) {
                        Ok(facts) => {
                            let moved = self.request.device == DeviceChoice::Default
                                && self
                                    .opened_facts
                                    .as_ref()
                                    .is_some_and(|opened| opened.name != facts.name);
                            if moved
                                || facts.rate != self.status.device_rate
                                || facts.channels != self.status.device_channels
                            {
                                self.lose(now, "native input configuration changed".into());
                                if self.status.state != MicState::Stuck {
                                    self.reopen(&facts, now);
                                }
                            }
                        }
                        Err(error) => self.lose(now, error.to_string()),
                    }
                }
            }
            MicState::Lost if now >= self.search_at => {
                self.last_facts = now;
                self.search_wait = self.search_wait.saturating_mul(2).min(RECHECK_MOST);
                self.search_at = now + self.search_wait;
                match self.backend.facts(&self.request.device) {
                    Ok(facts) => self.reopen(&facts, now),
                    Err(error) => {
                        self.backend.forget();
                        self.last_error = Some(error.to_string());
                    }
                }
            }
            MicState::Lost | MicState::Stuck | MicState::Off => {}
        }
        self.shared.publish(self.status);
    }

    /// Whether the frames since the last look stray more than a quarter
    /// from the rate the mic was opened at, and the device is due to be
    /// asked again. Each look in a row that finds nothing changed doubles the
    /// wait, up to [`RECHECK_MOST`]; frames at the rate start it over, and
    /// start over the wait between searches for the mic should it go.
    fn delivery_strays(&mut self, now: Instant) -> bool {
        let frames = self.shared.frames.load(Ordering::Relaxed);
        let (since, from) = self.delivery;
        self.delivery = (now, frames);
        let delivered = u128::from(frames.wrapping_sub(from));
        let expected = u128::from(self.status.device_rate)
            * now.saturating_duration_since(since).as_nanos()
            / 1_000_000_000;
        if delivered * 4 >= expected * 3 && delivered * 3 <= expected * 4 {
            self.recheck_at = now;
            self.recheck_wait = FACTS_EVERY;
            self.search_wait = FACTS_EVERY;
            return false;
        }
        if now < self.recheck_at {
            return false;
        }
        self.recheck_at = now + self.recheck_wait;
        self.recheck_wait = self.recheck_wait.saturating_mul(2).min(RECHECK_MOST);
        true
    }

    fn reopen(&mut self, facts: &MicFacts, now: Instant) {
        if let Err(error) = self.open_facts(facts, now) {
            if self.status.state != MicState::Stuck {
                self.status.state = MicState::Lost;
            }
            self.last_error = Some(error.to_string());
        }
    }
}

impl<B: MicBackend> Drop for Supervisor<B> {
    fn drop(&mut self) {
        drop(self.stream.take());
    }
}

#[cfg(test)]
mod tests;
