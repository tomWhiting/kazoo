//! Audio device enumeration and stream construction via `cpal`.

use cpal::traits::{DeviceTrait, HostTrait};

use super::channel_adapt::{InputAdapter, OutputAdapter};
use crate::{DEFAULT_BUFFER_SIZE, Error, Result};

/// Get the human-readable name of a device via `description()`.
pub(crate) fn device_name(device: &cpal::Device) -> String {
    device
        .description()
        .map_or_else(|_| "<unknown>".to_owned(), |d| d.name().to_owned())
}

// ---------------------------------------------------------------------------
// Configuration types
// ---------------------------------------------------------------------------

/// Parameters for building audio input/output streams.
#[derive(Debug, Clone)]
pub struct StreamConfig {
    /// Desired sample rate in Hz.  `None` uses the device default.
    pub sample_rate: Option<u32>,
    /// Desired buffer size in samples.  `None` uses the device default.
    pub buffer_size: Option<usize>,
    /// Input device name.  `None` selects the system default.
    pub input_device: Option<String>,
    /// Output device name.  `None` selects the system default.
    pub output_device: Option<String>,
    /// Number of input channels (default 1 -- mono microphone).
    pub input_channels: u16,
    /// Requested device output channels (default 2). The engine renders
    /// stereo, which is adapted to the opened device's channel count.
    pub output_channels: u16,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            sample_rate: None,
            buffer_size: Some(DEFAULT_BUFFER_SIZE),
            input_device: None,
            output_device: None,
            input_channels: 1,
            output_channels: 2,
        }
    }
}

// ---------------------------------------------------------------------------
// AudioStreams
// ---------------------------------------------------------------------------

/// A pair of running audio I/O streams returned by [`build_streams`].
///
/// Both streams are already in the *playing* state.  Dropping this struct
/// stops both streams automatically (cpal `Stream` implements `Drop`).
pub struct AudioStreams {
    /// The input (capture) stream.
    pub input: crate::host::audio::OpenedInput,
    /// The output (playback) stream.
    pub output: crate::host::audio::OpenedOutput,
    /// Actual sample rate negotiated with the devices.
    pub sample_rate: u32,
    /// Actual buffer size negotiated with the devices.
    pub buffer_size: usize,
}

// cpal::Stream is !Debug, so implement manually.
impl std::fmt::Debug for AudioStreams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioStreams")
            .field("sample_rate", &self.sample_rate)
            .field("buffer_size", &self.buffer_size)
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// DeviceInfo
// ---------------------------------------------------------------------------

/// Metadata about a single audio device.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    /// Human-readable device name.
    pub name: String,
    /// Whether this is the system default for its direction.
    pub is_default: bool,
    /// Maximum number of input channels the device supports.
    pub max_input_channels: u16,
    /// Maximum number of output channels the device supports.
    pub max_output_channels: u16,
    /// Standard sample rates the device supports.
    pub supported_sample_rates: Vec<u32>,
}

// ---------------------------------------------------------------------------
// Device enumeration
// ---------------------------------------------------------------------------

/// Standard sample rates checked during enumeration.
const STANDARD_RATES: &[u32] = &[8000, 11_025, 16_000, 22_050, 44_100, 48_000, 88_200, 96_000];

/// Enumerate all available audio input (capture) devices.
pub fn enumerate_input_devices() -> Result<Vec<DeviceInfo>> {
    let host = cpal::default_host();
    let default_name = host.default_input_device().map(|d| device_name(&d));

    let devices = host
        .input_devices()
        .map_err(|e| Error::AudioDevice(format!("failed to enumerate input devices: {e}")))?;

    let mut result = Vec::new();
    for device in devices {
        let name = device_name(&device);
        let is_default = default_name.as_deref() == Some(name.as_str());

        let (max_in, rates_in) = input_caps(&device);
        let (max_out, _) = output_caps(&device);

        result.push(DeviceInfo {
            name,
            is_default,
            max_input_channels: max_in,
            max_output_channels: max_out,
            supported_sample_rates: rates_in,
        });
    }
    Ok(result)
}

/// Enumerate all available audio output (playback) devices.
pub fn enumerate_output_devices() -> Result<Vec<DeviceInfo>> {
    let host = cpal::default_host();
    let default_name = host.default_output_device().map(|d| device_name(&d));

    let devices = host
        .output_devices()
        .map_err(|e| Error::AudioDevice(format!("failed to enumerate output devices: {e}")))?;

    let mut result = Vec::new();
    for device in devices {
        let name = device_name(&device);
        let is_default = default_name.as_deref() == Some(name.as_str());

        let (max_in, _) = input_caps(&device);
        let (max_out, rates_out) = output_caps(&device);

        result.push(DeviceInfo {
            name,
            is_default,
            max_input_channels: max_in,
            max_output_channels: max_out,
            supported_sample_rates: rates_out,
        });
    }
    Ok(result)
}

/// Extract the maximum input channel count and supported sample rates for a
/// device's input configurations.
fn input_caps(device: &cpal::Device) -> (u16, Vec<u32>) {
    let Ok(configs) = device.supported_input_configs() else {
        return (0, Vec::new());
    };

    let mut max_channels: u16 = 0;
    let mut rates = Vec::new();

    for cfg in configs {
        max_channels = max_channels.max(cfg.channels());
        let lo = cfg.min_sample_rate();
        let hi = cfg.max_sample_rate();
        for &rate in STANDARD_RATES {
            if rate >= lo && rate <= hi && !rates.contains(&rate) {
                rates.push(rate);
            }
        }
    }

    rates.sort_unstable();
    (max_channels, rates)
}

/// Extract the maximum output channel count and supported sample rates for a
/// device's output configurations.
fn output_caps(device: &cpal::Device) -> (u16, Vec<u32>) {
    let Ok(configs) = device.supported_output_configs() else {
        return (0, Vec::new());
    };

    let mut max_channels: u16 = 0;
    let mut rates = Vec::new();

    for cfg in configs {
        max_channels = max_channels.max(cfg.channels());
        let lo = cfg.min_sample_rate();
        let hi = cfg.max_sample_rate();
        for &rate in STANDARD_RATES {
            if rate >= lo && rate <= hi && !rates.contains(&rate) {
                rates.push(rate);
            }
        }
    }

    rates.sort_unstable();
    (max_channels, rates)
}

// ---------------------------------------------------------------------------
// Stream construction
// ---------------------------------------------------------------------------

fn output_request(config: &StreamConfig) -> crate::host::audio::OutputRequest {
    use crate::host::audio::{DeviceChoice, OutputRequest, RateChoice};
    OutputRequest {
        device: config
            .output_device
            .clone()
            .map_or(DeviceChoice::Default, DeviceChoice::Named),
        channels: config.output_channels,
        rate: config
            .sample_rate
            .map_or(RateChoice::Native, RateChoice::Hz),
    }
}

pub(crate) fn prepare_output(config: &StreamConfig) -> Result<crate::host::audio::PreparedOutput> {
    output_request(config)
        .prepare_output_with_frames(config.buffer_size.unwrap_or(DEFAULT_BUFFER_SIZE))
}

/// The output device's native sample rate. Running at it avoids Core Audio
/// inserting an internal resampler (which adds 11-23 ms of latency) when the
/// device runs at e.g. 48000 Hz but 44100 Hz is requested.
pub(crate) fn native_output_rate(device: &cpal::Device) -> Result<u32> {
    device
        .default_output_config()
        .map(|config| config.sample_rate())
        .map_err(|error| Error::Device(format!("cannot read native output rate: {error}")))
}

/// Build an input + output stream pair from the given configuration.
///
/// Both streams are started (playing) before being returned.  The caller
/// supplies two callbacks:
///
/// - `input_callback` receives captured audio data as `&[f32]`.
/// - `output_callback` fills an interleaved stereo buffer via `&mut [f32]`.
///
/// # Errors
///
/// Returns [`Error::Device`] if the requested (or default) device
/// cannot be found, the configuration is unsupported, or stream creation
/// fails.
pub fn build_streams(
    config: &StreamConfig,
    input_callback: impl FnMut(&[f32]) + Send + 'static,
    output_callback: impl FnMut(&mut [f32]) + Send + 'static,
) -> Result<AudioStreams> {
    let prepared = prepare_output(config)?;
    build_streams_with_output(config, prepared, input_callback, output_callback)
}

pub(crate) fn build_streams_with_output(
    config: &StreamConfig,
    prepared: crate::host::audio::PreparedOutput,
    mut input_callback: impl FnMut(&[f32]) + Send + 'static,
    mut output_callback: impl FnMut(&mut [f32]) + Send + 'static,
) -> Result<AudioStreams> {
    use crate::host::audio::{DeviceChoice, OutputRequest, RateChoice, open_input_with_frames};
    let info = prepared.info();
    let sample_rate = info.rate;
    let frames = info.frames;
    let input_request = OutputRequest {
        device: config
            .input_device
            .clone()
            .map_or(DeviceChoice::Default, DeviceChoice::Named),
        channels: config.input_channels,
        rate: RateChoice::Hz(sample_rate),
    };
    // Input follows the configured width; the engine always renders stereo.
    // Both are adapted to the actual device widths chosen by the host.
    let mut input_adapter = InputAdapter::new(config.input_channels, frames);
    let mut output_adapter = output_adapter(frames);
    let input = open_input_with_frames(&input_request, frames, move |data, info| {
        input_adapter.process(data, info.channels, &mut input_callback);
    })?;
    let output = prepared.open(move |data, info| {
        output_adapter.process(data, info.channels, &mut output_callback);
    })?;
    let buffer_size = output.buffer_frames();
    Ok(AudioStreams {
        input,
        output,
        sample_rate,
        buffer_size,
    })
}

fn output_adapter(frames: usize) -> OutputAdapter {
    OutputAdapter::new(2, frames)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Look up an input device by name, falling back to the system default.
pub(crate) fn resolve_input_device(host: &cpal::Host, name: Option<&str>) -> Result<cpal::Device> {
    if let Some(target) = name {
        let devices = host
            .input_devices()
            .map_err(|e| Error::AudioDevice(format!("cannot list input devices: {e}")))?;
        for device in devices {
            if device_name(&device) == target {
                return Ok(device);
            }
        }
        return Err(Error::AudioDevice(format!(
            "input device not found: {target}"
        )));
    }

    host.default_input_device()
        .ok_or_else(|| Error::AudioDevice("no default input device available".into()))
}

/// Look up an output device by name, falling back to the system default.
pub(crate) fn resolve_output_device(host: &cpal::Host, name: Option<&str>) -> Result<cpal::Device> {
    if let Some(target) = name {
        let devices = host
            .output_devices()
            .map_err(|e| Error::AudioDevice(format!("cannot list output devices: {e}")))?;
        for device in devices {
            if device_name(&device) == target {
                return Ok(device);
            }
        }
        return Err(Error::AudioDevice(format!(
            "output device not found: {target}"
        )));
    }

    host.default_output_device()
        .ok_or_else(|| Error::AudioDevice("no default output device available".into()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mono_output_config_gets_a_mono_mix_of_the_stereo_render() {
        let config = StreamConfig {
            output_channels: 1,
            ..StreamConfig::default()
        };
        let mut adapter = output_adapter(2);
        let mut data = [0.0; 5];
        adapter.process(&mut data, config.output_channels, &mut |stereo| {
            for frame in stereo.chunks_exact_mut(2) {
                frame[0] = 0.75;
                frame[1] = -0.25;
            }
        });
        assert!(
            data.iter()
                .all(|sample| (*sample - 0.25).abs() < f32::EPSILON)
        );
    }

    #[test]
    fn stream_config_default_values() {
        let cfg = StreamConfig::default();
        assert!(cfg.sample_rate.is_none());
        assert_eq!(cfg.buffer_size, Some(crate::DEFAULT_BUFFER_SIZE));
        assert!(cfg.input_device.is_none());
        assert!(cfg.output_device.is_none());
        assert_eq!(cfg.input_channels, 1);
        assert_eq!(cfg.output_channels, 2);
    }

    #[test]
    fn device_info_debug() {
        let info = DeviceInfo {
            name: "Test".to_owned(),
            is_default: true,
            max_input_channels: 2,
            max_output_channels: 2,
            supported_sample_rates: vec![44_100, 48_000],
        };
        let dbg = format!("{info:?}");
        assert!(dbg.contains("Test"));
    }

    #[test]
    fn enumerate_input_does_not_panic() {
        // On CI there may be zero devices (or no audio host at all); either
        // outcome must be well-formed.
        match enumerate_input_devices() {
            Ok(devices) => assert!(devices.iter().all(|d| !d.name.is_empty())),
            Err(e) => assert!(matches!(e, crate::Error::AudioDevice(_)), "{e}"),
        }
    }

    #[test]
    fn enumerate_output_does_not_panic() {
        match enumerate_output_devices() {
            Ok(devices) => assert!(devices.iter().all(|d| !d.name.is_empty())),
            Err(e) => assert!(matches!(e, crate::Error::AudioDevice(_)), "{e}"),
        }
    }

    #[test]
    fn resolve_input_device_nonexistent() {
        let host = cpal::default_host();
        let result = resolve_input_device(&host, Some("__nonexistent_device__"));
        assert!(result.is_err());
    }

    #[test]
    fn resolve_output_device_nonexistent() {
        let host = cpal::default_host();
        let result = resolve_output_device(&host, Some("__nonexistent_device__"));
        assert!(result.is_err());
    }
}
