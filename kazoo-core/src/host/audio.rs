use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use cpal::traits::{DeviceTrait, StreamTrait};
use crossbeam_channel::{Receiver, TryRecvError};

#[cfg(test)]
use super::audio_callback::OutputCallback;
use crate::io::device::{
    device_name, native_output_rate, resolve_input_device, resolve_output_device,
};
use crate::{Error, Result};

pub use super::audio_open::{open_input, open_output};
pub(crate) use super::audio_open::{open_input_with_frames, open_output_with_frames};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DeviceChoice {
    #[default]
    Default,
    Named(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RateChoice {
    #[default]
    Native,
    Hz(u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputRequest {
    pub device: DeviceChoice,
    pub channels: u16,
    pub rate: RateChoice,
}

impl Default for OutputRequest {
    fn default() -> Self {
        Self {
            device: DeviceChoice::Default,
            channels: 2,
            rate: RateChoice::Native,
        }
    }
}

impl OutputRequest {
    pub fn info(&self) -> Result<OutputInfo> {
        prepare(self, false, crate::DEFAULT_BUFFER_SIZE).map(|(_, chosen, _)| chosen.info)
    }

    fn context(&self, input: bool) -> String {
        let direction = if input { "input" } else { "output" };
        let device = match &self.device {
            DeviceChoice::Default => "default",
            DeviceChoice::Named(name) => name.as_str(),
        };
        match self.rate {
            RateChoice::Native => format!("{direction} device '{device}' at native rate"),
            RateChoice::Hz(rate) => format!("{direction} device '{device}' at {rate} Hz"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputInfo {
    pub rate: u32,
    pub channels: u16,
    pub frames: usize,
}

pub type InputRequest = OutputRequest;
pub type InputInfo = OutputInfo;
pub type OpenedInput = OpenedOutput;

#[derive(Debug, Clone)]
pub struct StreamHealth {
    pub(super) fault: Arc<AtomicUsize>,
    pub(super) receiver: Receiver<cpal::StreamError>,
    pub(super) context: String,
}

impl StreamHealth {
    pub fn check(&self) -> Result<()> {
        self.check_with_handler(drop)
    }

    fn check_with_handler(&self, mut handler: impl FnMut(cpal::StreamError)) -> Result<()> {
        match self.receiver.try_recv() {
            Ok(error) => {
                let report = Error::Device(format!("{}: {error}", self.context));
                handler(error);
                Err(report)
            }
            Err(TryRecvError::Disconnected) => Err(Error::Device(format!(
                "{}: stream error channel disconnected",
                self.context
            ))),
            Err(TryRecvError::Empty) => self.check_fault(),
        }
    }

    fn check_fault(&self) -> Result<()> {
        if self.fault.load(Ordering::Acquire) == 0 {
            Ok(())
        } else {
            Err(Error::Device(format!(
                "{}: stream error queue overflowed",
                self.context
            )))
        }
    }
}

pub struct OpenedOutput {
    pub(super) stream: cpal::Stream,
    pub(super) info: OutputInfo,
    pub(super) health: StreamHealth,
}

impl std::fmt::Debug for OpenedOutput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenedOutput")
            .field("info", &self.info)
            .finish_non_exhaustive()
    }
}

impl OpenedOutput {
    #[must_use]
    pub const fn rate(&self) -> u32 {
        self.info.rate
    }
    #[must_use]
    pub const fn channels(&self) -> u16 {
        self.info.channels
    }
    #[must_use]
    pub const fn buffer_frames(&self) -> usize {
        self.info.frames
    }
    #[must_use]
    pub fn health(&self) -> StreamHealth {
        self.health.clone()
    }
    pub fn check_error(&self) -> Result<()> {
        self.health.check()
    }
    pub fn check_error_with_handler(&self, handler: impl FnMut(cpal::StreamError)) -> Result<()> {
        self.health.check_with_handler(handler)
    }
    pub fn pause(&self) -> Result<()> {
        self.stream
            .pause()
            .map_err(|error| Error::Device(format!("{}: {error}", self.health.context)))
    }
}

pub(super) struct Chosen {
    pub(super) config: cpal::StreamConfig,
    pub(super) format: cpal::SampleFormat,
    pub(super) info: OutputInfo,
}

fn configure(
    request: &OutputRequest,
    native_rate: u32,
    ranges: &[cpal::SupportedStreamConfigRange],
    frames: usize,
) -> Result<Chosen> {
    let rate = match request.rate {
        RateChoice::Native => native_rate,
        RateChoice::Hz(rate) => rate,
    };
    if rate == 0 || request.channels == 0 || frames == 0 {
        return Err(Error::Device(
            "rate, channels and buffer frames must be nonzero".into(),
        ));
    }
    let frames =
        u32::try_from(frames).map_err(|error| Error::Device(format!("buffer size: {error}")))?;
    for format in [
        cpal::SampleFormat::F32,
        cpal::SampleFormat::I16,
        cpal::SampleFormat::U16,
    ] {
        for range in ranges {
            if range.sample_format() != format
                || range.channels() != request.channels
                || rate < range.min_sample_rate()
                || rate > range.max_sample_rate()
            {
                continue;
            }
            let frames = match *range.buffer_size() {
                cpal::SupportedBufferSize::Range { min, max } => {
                    if min == 0 || max < min {
                        continue;
                    }
                    frames.clamp(min, max)
                }
                cpal::SupportedBufferSize::Unknown => frames,
            };
            return Ok(Chosen {
                config: cpal::StreamConfig {
                    channels: request.channels,
                    sample_rate: rate,
                    buffer_size: cpal::BufferSize::Fixed(frames),
                },
                format,
                info: OutputInfo {
                    rate,
                    channels: request.channels,
                    frames: frames as usize,
                },
            });
        }
    }
    Err(Error::Device(format!(
        "no supported f32, i16 or u16 configuration for {} channels at {rate} Hz",
        request.channels
    )))
}

pub(super) fn prepare(
    request: &OutputRequest,
    input: bool,
    frames: usize,
) -> Result<(cpal::Device, Chosen, String)> {
    let context = request.context(input);
    let result = (|| {
        let host = cpal::default_host();
        let name = match &request.device {
            DeviceChoice::Default => None,
            DeviceChoice::Named(name) => Some(name.as_str()),
        };
        let device = if input {
            resolve_input_device(&host, name)?
        } else {
            resolve_output_device(&host, name)?
        };
        let native_rate = match request.rate {
            RateChoice::Hz(rate) => rate,
            RateChoice::Native if input => device
                .default_input_config()
                .map_err(|error| Error::Device(error.to_string()))?
                .sample_rate(),
            RateChoice::Native => native_output_rate(&device)?,
        };
        let ranges: Vec<_> = if input {
            device
                .supported_input_configs()
                .map_err(|error| Error::Device(error.to_string()))?
                .collect()
        } else {
            device
                .supported_output_configs()
                .map_err(|error| Error::Device(error.to_string()))?
                .collect()
        };
        let chosen = configure(request, native_rate, &ranges, frames)?;
        let actual = format!(
            "{context} ({}; {} Hz)",
            device_name(&device),
            chosen.info.rate
        );
        Ok((device, chosen, actual))
    })();
    result.map_err(|error: Error| Error::Device(format!("{context}: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn fake_ranges() -> [cpal::SupportedStreamConfigRange; 1] {
        [cpal::SupportedStreamConfigRange::new(
            2,
            44_100,
            96_000,
            cpal::SupportedBufferSize::Range { min: 64, max: 1024 },
            cpal::SampleFormat::F32,
        )]
    }

    #[test]
    fn request_defaults_to_native_rate_and_stereo() {
        let request = OutputRequest::default();
        assert_eq!(request.device, DeviceChoice::Default);
        assert_eq!(request.rate, RateChoice::Native);
        let chosen = configure(&request, 48_000, &fake_ranges(), 128).unwrap();
        assert_eq!(chosen.config.sample_rate, 48_000);
        assert_eq!(chosen.config.channels, 2);
        assert_eq!(chosen.info.frames, 128);
    }

    #[test]
    fn named_device_that_does_not_exist_is_an_error_naming_it() {
        let request = OutputRequest {
            device: DeviceChoice::Named("__kazoo_missing_audio_device__".into()),
            channels: 2,
            rate: RateChoice::Hz(48_000),
        };
        let error = open_output(&request, |_, _| {}).unwrap_err();
        assert!(matches!(error, Error::Device(_)), "{error}");
        let message = error.to_string();
        assert!(
            message.contains("__kazoo_missing_audio_device__"),
            "{message}"
        );
        assert!(message.contains("48000"), "{message}");
    }

    #[test]
    fn a_callback_of_another_size_still_plays() {
        let chosen = configure(&OutputRequest::default(), 48_000, &fake_ranges(), 128).unwrap();
        let mut output =
            OutputCallback::<f32, _>::new(chosen.info, |data: &mut [f32], _: &OutputInfo| {
                data.fill(0.5);
            });
        for frames in [64_usize, 128, 300, 1024] {
            let mut data = vec![0.0_f32; frames * 2];
            output.process(&mut data);
            assert!(
                data.iter()
                    .all(|sample| (*sample - 0.5).abs() < f32::EPSILON),
                "{frames} frames came out silent"
            );
        }
        let heard = std::sync::Arc::new(AtomicUsize::new(0));
        let counted = std::sync::Arc::clone(&heard);
        let mut input = super::super::audio_callback::InputCallback::new(
            chosen.info,
            move |data: &[f32], _: &OutputInfo| {
                counted.fetch_add(data.len(), Ordering::Relaxed);
            },
        );
        input.process(&[0.1_f32; 600]);
        assert_eq!(heard.load(Ordering::Relaxed), 600);
    }

    #[test]
    fn info_is_built_once_and_matches_the_stream() {
        let chosen = configure(&OutputRequest::default(), 48_000, &fake_ranges(), 128).unwrap();
        let calls = AtomicUsize::new(0);
        let address = AtomicUsize::new(0);
        let expected = chosen.info;
        let mut callback = OutputCallback::<f32, _>::new(chosen.info, |data, info: &OutputInfo| {
            assert_eq!(*info, expected);
            assert_eq!(data.len(), info.frames * usize::from(info.channels));
            let current = std::ptr::from_ref(info) as usize;
            if calls.fetch_add(1, Ordering::Relaxed) == 0 {
                address.store(current, Ordering::Relaxed);
            } else {
                assert_eq!(address.load(Ordering::Relaxed), current);
            }
            data.fill(0.25);
        });
        let mut data = [0.0; 256];
        for _ in 0..3 {
            callback.process(&mut data);
        }
        assert_eq!(calls.load(Ordering::Relaxed), 3);
        assert!(
            data.iter()
                .all(|sample| (*sample - 0.25_f32).abs() < f32::EPSILON)
        );
    }
}
