use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::{Receiver, TryRecvError};

#[cfg(test)]
use super::audio_callback::OutputCallback;
use crate::io::device::{
    device_name, native_output_rate, resolve_input_device, resolve_output_device,
};
use crate::{Error, Result};

pub use super::audio_open::{open_input, open_output};

mod input_pick;
pub(crate) use input_pick::resolve_named_input;
pub use input_pick::{
    InputNotices, InputPick, choose_input, explicit_input, input_for, input_for_names,
};

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
    /// Channel count the instrument wants to write. The device may open
    /// wider or narrower; callbacks receive the actual count in
    /// [`OutputInfo::channels`].
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
    pub fn prepare_output(&self) -> Result<PreparedOutput> {
        self.prepare_output_with_frames(crate::DEFAULT_BUFFER_SIZE)
    }

    pub fn prepare_output_with_frames(&self, frames: usize) -> Result<PreparedOutput> {
        self.prepare_output_using(|request| prepare(request, false, frames))
    }

    fn prepare_output_using<D>(
        &self,
        probe: impl FnOnce(&Self) -> Result<(D, Chosen, String)>,
    ) -> Result<PreparedOutput<D>> {
        let (device, chosen, context) = probe(self)?;
        Ok(PreparedOutput {
            device,
            chosen,
            context,
        })
    }

    pub fn prepare_input(&self) -> Result<PreparedInput> {
        self.prepare_input_with_frames(crate::DEFAULT_BUFFER_SIZE)
    }

    pub fn prepare_input_with_frames(&self, frames: usize) -> Result<PreparedInput> {
        let (device, chosen, context) = prepare(self, true, frames)?;
        Ok(PreparedInput {
            device,
            chosen,
            context,
        })
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
    /// Actual device channel count supplied to the callback.
    pub channels: u16,
    pub frames: usize,
}

pub type InputRequest = OutputRequest;
pub type InputInfo = OutputInfo;
pub type OpenedInput = OpenedOutput;

pub struct PreparedOutput<D = cpal::Device> {
    pub(super) device: D,
    pub(super) chosen: Chosen,
    pub(super) context: String,
}

impl<D> PreparedOutput<D> {
    #[must_use]
    pub const fn info(&self) -> OutputInfo {
        self.chosen.info
    }

    pub(super) fn open_using<S>(
        self,
        build: impl FnOnce(D, Chosen, String) -> Result<S>,
    ) -> Result<S> {
        build(self.device, self.chosen, self.context)
    }
}

impl PreparedOutput {
    /// The input to open beside this output. An input device whose name
    /// cannot be read is left out, and `notices` says so for the status line.
    ///
    /// # Errors
    ///
    /// [`Error::Device`] when the output or the input list cannot be read, or
    /// when nothing was asked for and no nameable input device is left.
    pub fn input_pick(&self, explicit: Option<&str>, notices: &InputNotices) -> Result<InputPick> {
        let output = self
            .device
            .description()
            .map_err(|error| Error::Device(format!("cannot name output device: {error}")))?;
        if let Some(name) = explicit {
            return input_for(Some(name), None, output.name(), &[]);
        }
        let host = cpal::default_host();
        let default = host
            .default_input_device()
            .map(|device| {
                device
                    .description()
                    .map(|description| description.name().to_owned())
            })
            .map(|result| result.map_err(|error| Error::Device(error.to_string())));
        let inputs = host
            .input_devices()
            .map_err(|error| Error::Device(format!("cannot list inputs: {error}")))?
            .map(|device| {
                device
                    .description()
                    .map(|description| description.name().to_owned())
                    .map_err(|error| Error::Device(error.to_string()))
            })
            .collect::<Vec<_>>();
        input_for_names(None, default, output.name(), inputs, |message| {
            notices.report(message);
        })
    }
}

impl<D> std::fmt::Debug for PreparedOutput<D> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedOutput")
            .field("info", &self.info())
            .field("context", &self.context)
            .finish_non_exhaustive()
    }
}

pub struct PreparedInput {
    pub(super) device: cpal::Device,
    pub(super) chosen: Chosen,
    pub(super) context: String,
}

impl PreparedInput {
    #[must_use]
    pub const fn info(&self) -> InputInfo {
        self.chosen.info
    }
}

impl std::fmt::Debug for PreparedInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedInput")
            .field("info", &self.info())
            .field("context", &self.context)
            .finish_non_exhaustive()
    }
}

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
    pub(super) fixed: bool,
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
    /// Whether the fixed buffer configuration was accepted by the device.
    #[must_use]
    pub const fn fixed_buffer(&self) -> bool {
        self.fixed
    }

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
    let formats = [
        cpal::SampleFormat::F32,
        cpal::SampleFormat::I16,
        cpal::SampleFormat::U16,
    ];
    let chosen = ranges
        .iter()
        .filter_map(|range| {
            let channels = range.channels();
            let format_order = formats
                .iter()
                .position(|format| *format == range.sample_format())?;
            if channels == 0 || rate < range.min_sample_rate() || rate > range.max_sample_rate() {
                return None;
            }
            let frames = match *range.buffer_size() {
                cpal::SupportedBufferSize::Range { min, max } => {
                    if min == 0 || max < min {
                        return None;
                    }
                    frames.clamp(min, max)
                }
                cpal::SupportedBufferSize::Unknown => frames,
            };
            let (step, distance) = match channels.cmp(&request.channels) {
                std::cmp::Ordering::Equal => (0, 0),
                std::cmp::Ordering::Greater => (1, channels - request.channels),
                std::cmp::Ordering::Less => (2, request.channels - channels),
            };
            Some(((step, distance, format_order), range, frames))
        })
        .min_by_key(|(priority, _, _)| *priority);
    if let Some((_, range, frames)) = chosen {
        return Ok(Chosen {
            config: cpal::StreamConfig {
                channels: range.channels(),
                sample_rate: rate,
                buffer_size: cpal::BufferSize::Fixed(frames),
            },
            format: range.sample_format(),
            info: OutputInfo {
                rate,
                channels: range.channels(),
                frames: frames as usize,
            },
        });
    }
    let mut offered: Vec<_> = ranges
        .iter()
        .map(cpal::SupportedStreamConfigRange::channels)
        .collect();
    offered.sort_unstable();
    offered.dedup();
    Err(Error::Device(format!(
        "no supported f32, i16 or u16 configuration for {} channels at {rate} Hz; device offers channel counts {offered:?}",
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
    fn prepared_output_opens_the_configuration_it_reported() {
        let request = OutputRequest::default();
        let probes = AtomicUsize::new(0);
        let prepared = request
            .prepare_output_using(|request| {
                probes.fetch_add(1, Ordering::Relaxed);
                let chosen = configure(request, 48_000, &[channel_range(8)], 128)?;
                Ok(("original device", chosen, "fake output".to_owned()))
            })
            .unwrap();
        let reported = prepared.info();
        let replacement = configure(&request, 96_000, &[channel_range(1)], 512).unwrap();
        assert_ne!(reported, replacement.info);
        let opened = prepared
            .open_using(|device, chosen, context| {
                assert_eq!(device, "original device");
                assert_eq!(context, "fake output");
                Ok(chosen.config)
            })
            .unwrap();
        assert_eq!(opened.channels, reported.channels);
        assert_eq!(opened.sample_rate, reported.rate);
        assert_eq!(opened.buffer_size, cpal::BufferSize::Fixed(128));
        assert_eq!(reported.frames, 128);
        assert_eq!(probes.load(Ordering::Relaxed), 1);
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

    fn channel_range(channels: u16) -> cpal::SupportedStreamConfigRange {
        cpal::SupportedStreamConfigRange::new(
            channels,
            44_100,
            96_000,
            cpal::SupportedBufferSize::Range { min: 64, max: 1024 },
            cpal::SampleFormat::F32,
        )
    }

    #[test]
    fn an_eight_channel_device_opens_with_eight_channels() {
        let chosen =
            configure(&OutputRequest::default(), 48_000, &[channel_range(8)], 128).unwrap();
        assert_eq!(chosen.config.channels, 8);
        assert_eq!(chosen.info.channels, 8);
    }

    #[test]
    fn a_mono_device_opens_with_one_channel() {
        let chosen =
            configure(&OutputRequest::default(), 48_000, &[channel_range(1)], 128).unwrap();
        assert_eq!(chosen.config.channels, 1);
        assert_eq!(chosen.info.channels, 1);
    }

    #[test]
    fn an_exact_match_is_preferred_over_a_wider_range() {
        let chosen = configure(
            &OutputRequest::default(),
            48_000,
            &[channel_range(8), channel_range(2)],
            128,
        )
        .unwrap();
        assert_eq!(chosen.config.channels, 2);
        assert_eq!(chosen.info.channels, 2);
    }

    #[test]
    fn the_smallest_wider_range_is_preferred_over_a_narrower_range() {
        let chosen = configure(
            &OutputRequest::default(),
            48_000,
            &[channel_range(8), channel_range(1), channel_range(4)],
            128,
        )
        .unwrap();
        assert_eq!(chosen.info.channels, 4);
    }

    #[test]
    fn the_largest_narrower_range_is_chosen_when_none_is_wide_enough() {
        let request = OutputRequest {
            channels: 8,
            ..OutputRequest::default()
        };
        let chosen = configure(
            &request,
            48_000,
            &[channel_range(1), channel_range(4), channel_range(2)],
            128,
        )
        .unwrap();
        assert_eq!(chosen.info.channels, 4);
    }

    #[test]
    fn channel_preference_keeps_format_rate_and_buffer_checks() {
        let ranges = [
            cpal::SupportedStreamConfigRange::new(
                2,
                44_100,
                44_100,
                cpal::SupportedBufferSize::Unknown,
                cpal::SampleFormat::F32,
            ),
            cpal::SupportedStreamConfigRange::new(
                4,
                48_000,
                48_000,
                cpal::SupportedBufferSize::Range { min: 0, max: 1024 },
                cpal::SampleFormat::F32,
            ),
            cpal::SupportedStreamConfigRange::new(
                8,
                48_000,
                48_000,
                cpal::SupportedBufferSize::Range { min: 256, max: 512 },
                cpal::SampleFormat::I16,
            ),
            channel_range(8),
        ];
        let chosen = configure(&OutputRequest::default(), 48_000, &ranges, 128).unwrap();
        assert_eq!(chosen.info.channels, 8);
        assert_eq!(chosen.format, cpal::SampleFormat::F32);
        assert_eq!(chosen.info.frames, 128);
        let chosen = configure(&OutputRequest::default(), 48_000, &ranges[..3], 128).unwrap();
        assert_eq!(chosen.format, cpal::SampleFormat::I16);
        assert_eq!(chosen.info.frames, 256);
        assert_eq!(chosen.config.buffer_size, cpal::BufferSize::Fixed(256));
        assert_eq!(chosen.info.rate, 48_000);
    }

    #[test]
    fn an_unavailable_rate_reports_the_device_channel_counts() {
        let request = OutputRequest {
            rate: RateChoice::Hz(192_000),
            ..OutputRequest::default()
        };
        let message = match configure(
            &request,
            48_000,
            &[channel_range(8), channel_range(1), channel_range(8)],
            128,
        ) {
            Ok(_) => panic!("unsupported rate opened"),
            Err(error) => error.to_string(),
        };
        assert!(message.contains("channel counts [1, 8]"), "{message}");
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
