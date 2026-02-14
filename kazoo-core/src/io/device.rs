//! Audio device enumeration and stream construction via `cpal`.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::{DEFAULT_BUFFER_SIZE, DEFAULT_SAMPLE_RATE, Error, Result};

/// Get the human-readable name of a device via `description()`.
fn device_name(device: &cpal::Device) -> String {
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
    /// Number of output channels (default 2 -- stereo).
    pub output_channels: u16,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            sample_rate: None,
            buffer_size: None,
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
    pub input: cpal::Stream,
    /// The output (playback) stream.
    pub output: cpal::Stream,
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

/// Build an input + output stream pair from the given configuration.
///
/// Both streams are started (playing) before being returned.  The caller
/// supplies two callbacks:
///
/// - `input_callback` receives captured audio data as `&[f32]`.
/// - `output_callback` fills the playback buffer via `&mut [f32]`.
///
/// # Errors
///
/// Returns [`Error::AudioDevice`] if the requested (or default) device
/// cannot be found, the configuration is unsupported, or stream creation
/// fails.
pub fn build_streams(
    config: &StreamConfig,
    mut input_callback: impl FnMut(&[f32]) + Send + 'static,
    mut output_callback: impl FnMut(&mut [f32]) + Send + 'static,
) -> Result<AudioStreams> {
    let host = cpal::default_host();

    // -- resolve devices --------------------------------------------------
    let input_device = resolve_input_device(&host, config.input_device.as_deref())?;
    let output_device = resolve_output_device(&host, config.output_device.as_deref())?;

    // -- determine sample rate --------------------------------------------
    let sample_rate = config.sample_rate.unwrap_or(DEFAULT_SAMPLE_RATE);

    // -- determine buffer size --------------------------------------------
    let buffer_size = config.buffer_size.unwrap_or(DEFAULT_BUFFER_SIZE);

    // -- build cpal StreamConfig ------------------------------------------
    let cpal_buffer = cpal::BufferSize::Fixed(
        u32::try_from(buffer_size)
            .map_err(|_| Error::Config("buffer size too large for u32".into()))?,
    );

    let input_stream_cfg = cpal::StreamConfig {
        channels: config.input_channels,
        sample_rate,
        buffer_size: cpal_buffer,
    };

    let output_stream_cfg = cpal::StreamConfig {
        channels: config.output_channels,
        sample_rate,
        buffer_size: cpal_buffer,
    };

    // -- build streams ----------------------------------------------------
    let input_stream = input_device
        .build_input_stream(
            &input_stream_cfg,
            move |data: &[f32], _info: &cpal::InputCallbackInfo| {
                input_callback(data);
            },
            |err| {
                eprintln!("input stream error: {err}");
            },
            None,
        )
        .map_err(|e| Error::AudioDevice(format!("failed to build input stream: {e}")))?;

    let output_stream = output_device
        .build_output_stream(
            &output_stream_cfg,
            move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
                output_callback(data);
            },
            |err| {
                eprintln!("output stream error: {err}");
            },
            None,
        )
        .map_err(|e| Error::AudioDevice(format!("failed to build output stream: {e}")))?;

    // -- start playback ---------------------------------------------------
    input_stream
        .play()
        .map_err(|e| Error::Stream(format!("failed to start input stream: {e}")))?;
    output_stream
        .play()
        .map_err(|e| Error::Stream(format!("failed to start output stream: {e}")))?;

    Ok(AudioStreams {
        input: input_stream,
        output: output_stream,
        sample_rate,
        buffer_size,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Look up an input device by name, falling back to the system default.
fn resolve_input_device(host: &cpal::Host, name: Option<&str>) -> Result<cpal::Device> {
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
fn resolve_output_device(host: &cpal::Host, name: Option<&str>) -> Result<cpal::Device> {
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
    fn stream_config_default_values() {
        let cfg = StreamConfig::default();
        assert!(cfg.sample_rate.is_none());
        assert!(cfg.buffer_size.is_none());
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
        // On CI there may be zero devices; that is fine -- just ensure no panic.
        let _ = enumerate_input_devices();
    }

    #[test]
    fn enumerate_output_does_not_panic() {
        let _ = enumerate_output_devices();
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
