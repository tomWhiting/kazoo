use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use kazoo_core::sanitize_sample;

pub const SAMPLES: usize = 4096;

#[derive(Debug)]
pub struct Capture {
    voice: Box<[AtomicU32]>,
    synth: Box<[AtomicU32]>,
    sequence: AtomicU64,
    end: AtomicU64,
}

impl Default for Capture {
    fn default() -> Self {
        Self {
            voice: (0..SAMPLES).map(|_| AtomicU32::new(0)).collect(),
            synth: (0..SAMPLES).map(|_| AtomicU32::new(0)).collect(),
            sequence: AtomicU64::new(0),
            end: AtomicU64::new(0),
        }
    }
}

impl Capture {
    pub(super) fn push(&self, voice: &[f32], stereo: &[f32]) {
        self.sequence.fetch_add(1, Ordering::SeqCst);
        let mut end = self.end.load(Ordering::SeqCst);
        for (input, output) in voice.iter().zip(stereo.chunks_exact(2)) {
            let index = end as usize % SAMPLES;
            self.voice[index].store(sanitize_sample(*input).to_bits(), Ordering::SeqCst);
            let sample = sanitize_sample(output[1].mul_add(0.5, output[0] * 0.5));
            self.synth[index].store(sample.to_bits(), Ordering::SeqCst);
            end = end.wrapping_add(1);
        }
        self.end.store(end, Ordering::SeqCst);
        self.sequence.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn read(&self, voice: &mut [f32; SAMPLES], synth: &mut [f32; SAMPLES]) -> bool {
        // A bounded retry leaves drawing to the next UI frame if a callback overlaps.
        for _ in 0..3 {
            let sequence = self.sequence.load(Ordering::SeqCst);
            if sequence % 2 != 0 {
                continue;
            }
            let end = self.end.load(Ordering::SeqCst);
            let count = end.min(SAMPLES as u64) as usize;
            voice[..SAMPLES - count].fill(0.0);
            synth[..SAMPLES - count].fill(0.0);
            for offset in 0..count {
                let index =
                    end.wrapping_sub(count as u64).wrapping_add(offset as u64) as usize % SAMPLES;
                voice[SAMPLES - count + offset] =
                    f32::from_bits(self.voice[index].load(Ordering::SeqCst));
                synth[SAMPLES - count + offset] =
                    f32::from_bits(self.synth[index].load(Ordering::SeqCst));
            }
            if self.sequence.load(Ordering::SeqCst) == sequence {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rings_keep_the_latest_pair_and_sanitize_faults() {
        let capture = Capture::default();
        let voice: Vec<_> = (0..SAMPLES + 128)
            .map(|index| index as f32 / 10_000.0)
            .collect();
        let stereo: Vec<_> = voice
            .iter()
            .flat_map(|sample| [*sample, -*sample])
            .collect();
        capture.push(&voice, &stereo);
        let mut input = [0.0; SAMPLES];
        let mut output = [0.0; SAMPLES];
        assert!(capture.read(&mut input, &mut output));
        assert_eq!(input.as_slice(), &voice[128..]);
        assert!(output.iter().all(|sample| *sample == 0.0));
        capture.push(
            &[f32::NAN, f32::INFINITY],
            &[f32::NAN, 0.0, f32::INFINITY, f32::NEG_INFINITY],
        );
        assert!(capture.read(&mut input, &mut output));
        assert!(input.iter().chain(&output).all(|sample| sample.is_finite()));
        assert_eq!(input[SAMPLES - 1].to_bits(), 0.0_f32.to_bits());
        assert_eq!(output[SAMPLES - 1].to_bits(), 0.0_f32.to_bits());
    }
}
