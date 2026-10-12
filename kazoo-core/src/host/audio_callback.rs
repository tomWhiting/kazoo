use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::audio::OutputInfo;

pub(super) struct OutputCallback<T, F> {
    info: OutputInfo,
    callback: F,
    scratch: Vec<f32>,
    fault: Arc<AtomicUsize>,
    sample: PhantomData<fn() -> T>,
}

impl<T, F> OutputCallback<T, F>
where
    T: cpal::FromSample<f32>,
    F: FnMut(&mut [f32], &OutputInfo),
{
    pub(super) fn new(info: OutputInfo, callback: F, fault: Arc<AtomicUsize>) -> Self {
        Self {
            info,
            callback,
            scratch: vec![0.0; info.frames * usize::from(info.channels)],
            fault,
            sample: PhantomData,
        }
    }

    pub(super) fn process(&mut self, data: &mut [T]) {
        if data.len() != self.scratch.len() {
            self.fault.store(1, Ordering::Release);
            for sample in data {
                *sample = T::from_sample_(0.0);
            }
            return;
        }
        self.scratch.fill(0.0);
        (self.callback)(&mut self.scratch, &self.info);
        for (target, source) in data.iter_mut().zip(&self.scratch) {
            *target = T::from_sample_(sanitize(*source));
        }
    }
}

pub(super) struct InputCallback<F> {
    info: OutputInfo,
    callback: F,
    scratch: Vec<f32>,
    fault: Arc<AtomicUsize>,
}

impl<F> InputCallback<F>
where
    F: FnMut(&[f32], &OutputInfo),
{
    pub(super) fn new(info: OutputInfo, callback: F, fault: Arc<AtomicUsize>) -> Self {
        Self {
            info,
            callback,
            scratch: vec![0.0; info.frames * usize::from(info.channels)],
            fault,
        }
    }

    pub(super) fn process<T>(&mut self, data: &[T])
    where
        T: cpal::Sample,
        f32: cpal::FromSample<T>,
    {
        if data.len() != self.scratch.len() {
            self.fault.store(1, Ordering::Release);
            return;
        }
        for (target, source) in self.scratch.iter_mut().zip(data) {
            *target = sanitize(source.to_sample::<f32>());
        }
        (self.callback)(&self.scratch, &self.info);
    }
}

const fn sanitize(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}
