use super::audio::OutputInfo;
use std::marker::PhantomData;

pub(super) struct OutputCallback<T, F> {
    info: OutputInfo,
    callback: F,
    scratch: Vec<f32>,
    sample: PhantomData<fn() -> T>,
}

impl<T, F> OutputCallback<T, F>
where
    T: cpal::FromSample<f32>,
    F: FnMut(&mut [f32], &OutputInfo),
{
    pub(super) fn new(info: OutputInfo, callback: F) -> Self {
        Self {
            info,
            callback,
            scratch: vec![0.0; info.frames * usize::from(info.channels)],
            sample: PhantomData,
        }
    }

    /// Renders a callback of any length. The device may deliver more or
    /// fewer frames than were negotiated (another client changed the shared
    /// device buffer size, a Bluetooth route, a rate converter), so the
    /// buffer is rendered in whole chunks of the pre-allocated scratch and
    /// never silenced for its size.
    pub(super) fn process(&mut self, data: &mut [T]) {
        for chunk in data.chunks_mut(self.scratch.len()) {
            let scratch = &mut self.scratch[..chunk.len()];
            scratch.fill(0.0);
            (self.callback)(scratch, &self.info);
            for (target, source) in chunk.iter_mut().zip(scratch.iter()) {
                *target = T::from_sample_(sanitize(*source));
            }
        }
    }
}

pub(super) struct InputCallback<F> {
    info: OutputInfo,
    callback: F,
    scratch: Vec<f32>,
}

impl<F> InputCallback<F>
where
    F: FnMut(&[f32], &OutputInfo),
{
    pub(super) fn new(info: OutputInfo, callback: F) -> Self {
        Self {
            info,
            callback,
            scratch: vec![0.0; info.frames * usize::from(info.channels)],
        }
    }

    pub(super) fn process<T>(&mut self, data: &[T])
    where
        T: cpal::Sample,
        f32: cpal::FromSample<T>,
    {
        for chunk in data.chunks(self.scratch.len()) {
            let scratch = &mut self.scratch[..chunk.len()];
            for (target, source) in scratch.iter_mut().zip(chunk) {
                *target = sanitize(source.to_sample::<f32>());
            }
            (self.callback)(scratch, &self.info);
        }
    }
}

const fn sanitize(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}
