//! Selected interface channels, bounded history and measured software latency.

mod ring;
mod round_trip;
#[cfg(test)]
mod tests;

use super::audio::{DeviceChoice, OpenedInput, OutputRequest, RateChoice};
use crate::{Error, Result};
use cpal::traits::DeviceTrait;

pub use ring::{LiveInput, LiveStats, LiveWriter, ReadReport, live_pair};
pub use round_trip::{RoundTrip, RoundTripError, RoundTripResult};

/// 170 ms at 48 kHz and 43 ms at 192 kHz: more than the largest slack (one
/// 256-frame buffer) plus the drift window's excess plus the crossfade, at
/// every rate; a power of two so every history index is masked.
pub const LIVE_RING_FRAMES: usize = 8192;
/// The reader never goes within 1024 frames of being overwritten, so a
/// stalled reader is caught as an overrun, not read torn.
pub const GUARD: usize = 1024;
/// An underrun adds 16 frames (0.33 ms at 48 kHz) of slack: small enough
/// that one bad callback costs little latency, large enough that a machine
/// with real jitter settles within a few steps. The slack is capped at the
/// output buffer: more would hide a buffer size the machine cannot keep.
pub const SLACK_STEP: usize = 16;
/// Every jump crossfades over 1 ms (48 frames at 48 kHz): shorter than a
/// pick attack, long enough that a jump is not a click.
pub const FADE_MS: f32 = 1.0;
/// The reader jumps forward when its lag has stayed more than half a buffer
/// over target for 250 ms (drift), or at once when it is more than 4 buffers
/// over (a stall or a burst).
pub const DRIFT_WINDOW_MS: u64 = 250;
/// The desk's `CLIP_THRESHOLD` (`kazoo-mix/src/engine.rs`): a raw sample at
/// or above it, before any gain, means the converter has clipped.
pub const CLIP_RAW: f32 = 0.999;
/// One interface's inputs; eight strips are the most anyone will tune by
/// ear at once.
pub const MAX_LIVE_CHANNELS: usize = 8;

/// Labels and fundamental ranges for input tracking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputProfile {
    #[default]
    Line,
    Voice,
    Guitar,
    Bass,
}

impl InputProfile {
    pub const ALL: [Self; 4] = [Self::Line, Self::Voice, Self::Guitar, Self::Bass];

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "line" => Some(Self::Line),
            "voice" => Some(Self::Voice),
            "guitar" => Some(Self::Guitar),
            "bass" => Some(Self::Bass),
            _ => None,
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Line => "line",
            Self::Voice => "voice",
            Self::Guitar => "guitar",
            Self::Bass => "bass",
        }
    }

    /// Fundamental ranges in Hz. Voice is the voice tracker's own range
    /// (`VOICE_MIN_HZ`, `VOICE_MAX_HZ`).
    /// Guitar covers B1 (61.7 Hz), drop C (65.4 Hz), and top E's 24th fret
    /// (1318.5 Hz) with bends. Bass covers detuned B0 (29.1 Hz) and G's
    /// 24th fret (392 Hz) with bends. Line covers the tracker's widest range
    /// for synths, drum machines and other sources.
    #[must_use]
    pub const fn range_hz(self) -> (f32, f32) {
        match self {
            Self::Line => (30.0, 4000.0),
            Self::Voice => (crate::analysis::VOICE_MIN_HZ, crate::analysis::VOICE_MAX_HZ),
            Self::Guitar => (60.0, 1400.0),
            Self::Bass => (29.0, 500.0),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LiveInputRequest {
    pub device: DeviceChoice,
    /// Distinct 1-based channel numbers as printed on the interface.
    pub channels: Vec<u16>,
    /// Desk sample rate; live inputs never resample.
    pub rate: u32,
    /// Fixed buffer of 32, 64, 128 or 256 frames.
    pub frames: usize,
}

#[derive(Debug)]
pub struct LiveInputStream {
    stream: OpenedInput,
    name: String,
}

impl LiveInputStream {
    #[must_use]
    pub fn device_name(&self) -> &str {
        &self.name
    }
    #[must_use]
    pub const fn device_channels(&self) -> u16 {
        self.stream.channels()
    }
    /// False when the device runs at its own buffer size.
    #[must_use]
    pub const fn fixed_buffer(&self) -> bool {
        self.stream.fixed_buffer()
    }
    pub fn check_error(&self) -> Result<()> {
        self.stream.check_error()
    }
}

fn validate(request: &LiveInputRequest) -> Result<u16> {
    if request.rate == 0 || !matches!(request.frames, 32 | 64 | 128 | 256) {
        return Err(Error::Device(
            "live input requires a nonzero rate and 32, 64, 128 or 256 frames".into(),
        ));
    }
    if request.channels.is_empty() || request.channels.len() > MAX_LIVE_CHANNELS {
        return Err(Error::Device(
            "live input requires 1 to 8 distinct channels".into(),
        ));
    }
    let mut maximum = 0;
    for (index, &channel) in request.channels.iter().enumerate() {
        if channel == 0 || request.channels[..index].contains(&channel) {
            return Err(Error::Device(
                "live input channels must be distinct and 1-based".into(),
            ));
        }
        maximum = maximum.max(channel);
    }
    Ok(maximum)
}

fn check_channels(name: &str, count: u16, channels: &[u16]) -> Result<()> {
    for &channel in channels {
        if channel > count {
            return Err(Error::Device(format!(
                "input device '{name}' has {count} channels; channel {channel} was asked for"
            )));
        }
    }
    Ok(())
}

/// Open the asked channels of an input device at the desk's rate and a
/// fixed buffer, one [`LiveInput`] per channel in the order asked, each to be
/// moved into the output callback. Fails on a request out of range, a device
/// that cannot run at the rate, or a channel the device does not have.
pub fn open_live_input(request: &LiveInputRequest) -> Result<(LiveInputStream, Vec<LiveInput>)> {
    let channels = validate(request)?;
    let prepared = OutputRequest {
        device: request.device.clone(),
        channels,
        rate: RateChoice::Hz(request.rate),
    }
    .prepare_input_with_frames(request.frames)?;
    let name = prepared
        .device
        .description()
        .map_err(|error| Error::Device(format!("input device description: {error}")))?
        .name()
        .to_owned();
    let count = prepared.info().channels;
    check_channels(&name, count, &request.channels)?;
    let (writers, readers) = request
        .channels
        .iter()
        .map(|_| live_pair(request.rate, request.frames))
        .unzip();
    let capture = InputCapture::new(request.channels.clone(), count, writers);
    let stream = prepared.open_live(capture)?;
    Ok((LiveInputStream { stream, name }, readers))
}

/// Raw conversion precedes sanitising so converter clipping remains observable.
#[derive(Debug)]
pub(super) struct InputCapture {
    channels: Vec<u16>,
    count: usize,
    writers: Vec<LiveWriter>,
}

impl InputCapture {
    fn new(channels: Vec<u16>, count: u16, writers: Vec<LiveWriter>) -> Self {
        Self {
            channels,
            count: usize::from(count),
            writers,
        }
    }

    pub(super) fn process<T>(&mut self, data: &[T])
    where
        T: cpal::Sample,
        f32: cpal::FromSample<T>,
    {
        let mut scratch = [0.0_f32; 4096];
        for (&channel, writer) in self.channels.iter().zip(&mut self.writers) {
            writer.observe_callback(data.len() / self.count);
            for chunk in data.chunks(4096 * self.count) {
                let frames = chunk.len() / self.count;
                for (target, frame) in scratch.iter_mut().zip(chunk.chunks_exact(self.count)) {
                    *target = frame[usize::from(channel - 1)].to_sample::<f32>();
                }
                writer.write(&scratch[..frames]);
            }
        }
    }
}
