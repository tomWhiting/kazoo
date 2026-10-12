//! Fit a stream callback written for one channel count to a device opened
//! with another. The shared host opens the nearest count a device offers
//! (wider, or narrower on a mono device), so the engine's stereo output and
//! configured input width are adapted here. Scratch space
//! is allocated once, when the adapter is built; the callback path does
//! not allocate.

/// Adapts an output callback that writes `channels` interleaved channels to
/// a device buffer of whatever width the device was opened with.
pub struct OutputAdapter {
    channels: usize,
    scratch: Vec<f32>,
}

impl OutputAdapter {
    /// `frames` is the most frames rendered in one pass; longer device
    /// buffers are rendered in passes of at most that many frames.
    pub fn new(channels: u16, frames: usize) -> Self {
        let channels = usize::from(channels.max(1));
        Self {
            channels,
            scratch: vec![0.0; frames.max(1) * channels],
        }
    }

    /// Fill `data` (interleaved at the device's `device_channels`) by
    /// calling `render` with buffers interleaved at the configured width.
    pub fn process(
        &mut self,
        data: &mut [f32],
        device_channels: u16,
        render: &mut impl FnMut(&mut [f32]),
    ) {
        let device_channels = usize::from(device_channels.max(1));
        data.fill(0.0);
        if self.channels == device_channels {
            render(data);
            return;
        }
        let pass_frames = self.scratch.len() / self.channels;
        for device_chunk in data.chunks_mut(pass_frames * device_channels) {
            let frames = device_chunk.len() / device_channels;
            let scratch = &mut self.scratch[..frames * self.channels];
            scratch.fill(0.0);
            render(scratch);
            for (out, frame) in device_chunk
                .chunks_exact_mut(device_channels)
                .zip(scratch.chunks_exact(self.channels))
            {
                if device_channels == 1 {
                    out[0] = mean(frame);
                } else {
                    let shared = self.channels.min(device_channels);
                    out[..shared].copy_from_slice(&frame[..shared]);
                }
            }
        }
    }
}

/// Adapts an input callback that reads `channels` interleaved channels to a
/// device buffer of whatever width the device was opened with. A device
/// frame of a different width is reduced to the mean of its channels,
/// given to every configured channel.
pub struct InputAdapter {
    channels: usize,
    scratch: Vec<f32>,
}

impl InputAdapter {
    /// `frames` is the most frames passed on in one call; longer device
    /// buffers are passed on in pieces of at most that many frames.
    pub fn new(channels: u16, frames: usize) -> Self {
        let channels = usize::from(channels.max(1));
        Self {
            channels,
            scratch: vec![0.0; frames.max(1) * channels],
        }
    }

    /// Pass `data` (interleaved at the device's `device_channels`) to
    /// `consume`, interleaved at the configured width.
    pub fn process(
        &mut self,
        data: &[f32],
        device_channels: u16,
        consume: &mut impl FnMut(&[f32]),
    ) {
        let device_channels = usize::from(device_channels.max(1));
        if self.channels == device_channels {
            consume(data);
            return;
        }
        let pass_frames = self.scratch.len() / self.channels;
        for device_chunk in data.chunks(pass_frames * device_channels) {
            let frames = device_chunk.len() / device_channels;
            let scratch = &mut self.scratch[..frames * self.channels];
            for (out, frame) in scratch
                .chunks_exact_mut(self.channels)
                .zip(device_chunk.chunks_exact(device_channels))
            {
                out.fill(mean(frame));
            }
            consume(scratch);
        }
    }
}

/// Mean of a frame's channels; a non-finite sum is silence.
fn mean(frame: &[f32]) -> f32 {
    crate::sanitize_sample(frame.iter().sum::<f32>() / frame.len() as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stereo_ramp(data: &mut [f32]) {
        for (index, frame) in data.chunks_exact_mut(2).enumerate() {
            frame[0] = index as f32 * 0.001;
            frame[1] = -(index as f32) * 0.0005;
        }
    }

    #[test]
    fn stereo_output_lands_on_the_first_two_of_eight_device_channels() {
        let mut adapter = OutputAdapter::new(2, 64);
        let mut data = vec![1.0_f32; 200 * 8];
        adapter.process(&mut data, 8, &mut stereo_ramp);
        for (index, frame) in data.chunks_exact(8).enumerate() {
            let expected = (index % 64) as f32;
            assert!((frame[0] - expected * 0.001).abs() < 1.0e-6);
            assert!((frame[1] - expected * -0.0005).abs() < 1.0e-6);
            assert!(frame[2..].iter().all(|sample| *sample == 0.0));
        }
    }

    #[test]
    fn stereo_output_on_a_mono_device_is_the_mean_of_left_and_right() {
        let mut adapter = OutputAdapter::new(2, 64);
        let mut data = vec![1.0_f32; 100];
        adapter.process(&mut data, 1, &mut stereo_ramp);
        for (index, sample) in data.iter().enumerate() {
            let frame = (index % 64) as f32;
            let expected = (frame * 0.001 - frame * 0.0005) * 0.5;
            assert!((sample - expected).abs() < 1.0e-6);
        }
    }

    #[test]
    fn a_mono_output_device_gets_silence_for_a_non_finite_frame() {
        let mut adapter = OutputAdapter::new(2, 64);
        let mut data = vec![1.0_f32; 4];
        adapter.process(&mut data, 1, &mut |scratch: &mut [f32]| {
            scratch.fill(f32::NAN);
        });
        assert!(data.iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn a_matching_output_width_is_passed_through() {
        let mut adapter = OutputAdapter::new(2, 64);
        let mut data = vec![1.0_f32; 300 * 2];
        let mut calls = 0;
        adapter.process(&mut data, 2, &mut |scratch: &mut [f32]| {
            calls += 1;
            stereo_ramp(scratch);
        });
        assert_eq!(calls, 1);
        assert!((data[299 * 2] - 0.299).abs() < 1.0e-6);
    }

    #[test]
    fn a_stereo_mic_reaches_a_mono_engine_as_the_mean_of_its_channels() {
        let mut adapter = InputAdapter::new(1, 64);
        let data: Vec<f32> = (0..200).map(|index| index as f32).collect();
        let mut received = Vec::new();
        adapter.process(&data, 2, &mut |scratch: &[f32]| {
            received.extend_from_slice(scratch);
        });
        assert_eq!(received.len(), 100);
        for (index, sample) in received.iter().enumerate() {
            let expected = (index * 2) as f32 + 0.5;
            assert!((sample - expected).abs() < 1.0e-4);
        }
    }

    #[test]
    fn a_mono_mic_reaches_a_stereo_engine_on_both_channels() {
        let mut adapter = InputAdapter::new(2, 16);
        let data = [0.25_f32, -0.5, f32::INFINITY];
        let mut received = Vec::new();
        adapter.process(&data, 1, &mut |scratch: &[f32]| {
            received.extend_from_slice(scratch);
        });
        assert_eq!(received, vec![0.25, 0.25, -0.5, -0.5, 0.0, 0.0]);
    }
}
