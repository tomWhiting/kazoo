use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use cpal::traits::{DeviceTrait, StreamTrait};
use crossbeam_channel::Sender;

use super::audio::{Chosen, OpenedOutput, OutputInfo, OutputRequest, StreamHealth, prepare};
use super::audio_callback::{InputCallback, OutputCallback};
use crate::{Error, Result};

pub fn open_output<F>(request: &OutputRequest, callback: F) -> Result<OpenedOutput>
where
    F: FnMut(&mut [f32], &OutputInfo) + Send + 'static,
{
    open_output_with_frames(request, crate::DEFAULT_BUFFER_SIZE, callback)
}

pub fn open_input<F>(request: &OutputRequest, callback: F) -> Result<OpenedOutput>
where
    F: FnMut(&[f32], &OutputInfo) + Send + 'static,
{
    open_input_with_frames(request, crate::DEFAULT_BUFFER_SIZE, callback)
}

pub fn open_output_with_frames<F>(
    request: &OutputRequest,
    frames: usize,
    callback: F,
) -> Result<OpenedOutput>
where
    F: FnMut(&mut [f32], &OutputInfo) + Send + 'static,
{
    let (device, mut chosen, context) = prepare(request, false, frames)?;
    fall_back_when_fixed_is_refused(&device, &mut chosen, false);
    let (errors, receiver) = crossbeam_channel::bounded(16);
    let fault = Arc::new(AtomicUsize::new(0));
    let stream = match chosen.format {
        cpal::SampleFormat::F32 => output_stream::<f32, _>(
            &device,
            &chosen.config,
            chosen.info,
            callback,
            errors,
            Arc::clone(&fault),
        ),
        cpal::SampleFormat::I16 => output_stream::<i16, _>(
            &device,
            &chosen.config,
            chosen.info,
            callback,
            errors,
            Arc::clone(&fault),
        ),
        cpal::SampleFormat::U16 => output_stream::<u16, _>(
            &device,
            &chosen.config,
            chosen.info,
            callback,
            errors,
            Arc::clone(&fault),
        ),
        other => {
            return Err(Error::Device(format!(
                "{context}: unsupported sample format {other:?}"
            )));
        }
    }
    .map_err(|error| Error::Device(format!("{context}: {error}")))?;
    stream
        .play()
        .map_err(|error| Error::Device(format!("{context}: {error}")))?;
    Ok(OpenedOutput {
        stream,
        info: chosen.info,
        health: StreamHealth {
            fault,
            receiver,
            context,
        },
    })
}

pub fn open_input_with_frames<F>(
    request: &OutputRequest,
    frames: usize,
    callback: F,
) -> Result<OpenedOutput>
where
    F: FnMut(&[f32], &OutputInfo) + Send + 'static,
{
    let (device, mut chosen, context) = prepare(request, true, frames)?;
    fall_back_when_fixed_is_refused(&device, &mut chosen, true);
    let (errors, receiver) = crossbeam_channel::bounded(16);
    let fault = Arc::new(AtomicUsize::new(0));
    let stream = match chosen.format {
        cpal::SampleFormat::F32 => input_stream::<f32, _>(
            &device,
            &chosen.config,
            chosen.info,
            callback,
            errors,
            Arc::clone(&fault),
        ),
        cpal::SampleFormat::I16 => input_stream::<i16, _>(
            &device,
            &chosen.config,
            chosen.info,
            callback,
            errors,
            Arc::clone(&fault),
        ),
        cpal::SampleFormat::U16 => input_stream::<u16, _>(
            &device,
            &chosen.config,
            chosen.info,
            callback,
            errors,
            Arc::clone(&fault),
        ),
        other => {
            return Err(Error::Device(format!(
                "{context}: unsupported sample format {other:?}"
            )));
        }
    }
    .map_err(|error| Error::Device(format!("{context}: {error}")))?;
    stream
        .play()
        .map_err(|error| Error::Device(format!("{context}: {error}")))?;
    Ok(OpenedOutput {
        stream,
        info: chosen.info,
        health: StreamHealth {
            fault,
            receiver,
            context,
        },
    })
}

fn output_stream<T, F>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    info: OutputInfo,
    callback: F,
    errors: Sender<cpal::StreamError>,
    fault: Arc<AtomicUsize>,
) -> std::result::Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::SizedSample + cpal::FromSample<f32> + 'static,
    F: FnMut(&mut [f32], &OutputInfo) + Send + 'static,
{
    let mut callback = OutputCallback::<T, _>::new(info, callback);
    device.build_output_stream(
        config,
        move |data: &mut [T], _| callback.process(data),
        move |error| record_error(&errors, &fault, error),
        None,
    )
}

fn input_stream<T, F>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    info: OutputInfo,
    callback: F,
    errors: Sender<cpal::StreamError>,
    fault: Arc<AtomicUsize>,
) -> std::result::Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
    F: FnMut(&[f32], &OutputInfo) + Send + 'static,
{
    let mut callback = InputCallback::new(info, callback);
    device.build_input_stream(
        config,
        move |data: &[T], _| callback.process(data),
        move |error| record_error(&errors, &fault, error),
        None,
    )
}

fn record_error(errors: &Sender<cpal::StreamError>, fault: &AtomicUsize, error: cpal::StreamError) {
    if errors.try_send(error).is_err() {
        fault.store(2, Ordering::Release);
    }
}

/// Bluetooth and some aggregate devices refuse a fixed buffer size. Probe
/// with a silent stream of the chosen format first and use the device's
/// own buffer size when the fixed one is refused, as the engine did before
/// the shared host. Any callback length is rendered, so this never changes
/// what is heard.
fn fall_back_when_fixed_is_refused(device: &cpal::Device, chosen: &mut Chosen, input: bool) {
    let refused = match chosen.format {
        cpal::SampleFormat::F32 => probe::<f32>(device, &chosen.config, input),
        cpal::SampleFormat::I16 => probe::<i16>(device, &chosen.config, input),
        cpal::SampleFormat::U16 => probe::<u16>(device, &chosen.config, input),
        _ => false,
    };
    if refused {
        chosen.config.buffer_size = cpal::BufferSize::Default;
    }
}

fn probe<T: cpal::SizedSample + 'static>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    input: bool,
) -> bool {
    if input {
        device
            .build_input_stream(config, |_: &[T], _| {}, drop, None)
            .is_err()
    } else {
        device
            .build_output_stream(config, |_: &mut [T], _| {}, drop, None)
            .is_err()
    }
}
