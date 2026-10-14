use super::{OscillatorShape, PitchTrackedSynth, poly_blep};
use crate::sanitize_sample;

impl PitchTrackedSynth {
    /// Render the first `min(input, output, carrier)` samples exactly as
    /// [`crate::Processor::process`] would, and write the raw oscillator
    /// (before the envelope and the filter) for those samples to `carrier`,
    /// for a vocoder to use as its carrier. A sine has no harmonics to carry
    /// a voice, so on sine the carrier is an anti-aliased saw at the same
    /// pitch and phase; `output` keeps the sine. The rest of `output` and
    /// `carrier` is silence. Never allocates, locks or panics.
    pub fn process_with_carrier(&mut self, input: &[f32], output: &mut [f32], carrier: &mut [f32]) {
        let len = input.len().min(output.len()).min(carrier.len());
        output[len..].fill(0.0);
        carrier[len..].fill(0.0);
        let piece = self.filter_scratch.len().max(1);
        for ((input, output), carrier) in input[..len]
            .chunks(piece)
            .zip(output[..len].chunks_mut(piece))
            .zip(carrier[..len].chunks_mut(piece))
        {
            self.render_piece(input, output, Some(carrier));
        }
    }

    /// The carrier for one sample: the oscillator `osc` as rendered, or on
    /// sine a saw at the phase the sine was rendered from. Silence wherever
    /// the oscillator itself is silent (no pitch, or above Nyquist).
    pub(super) fn carrier_sample(&self, osc: f32, phase: f32, frequency: f32) -> f32 {
        if self.shape != OscillatorShape::Sine {
            return osc;
        }
        if frequency <= 0.0 || !frequency.is_finite() || self.sample_rate <= 0.0 {
            return 0.0;
        }
        let dt = frequency / self.sample_rate;
        if dt >= 0.5 {
            return 0.0;
        }
        sanitize_sample(phase.mul_add(2.0, -1.0) - poly_blep(phase, dt))
    }
}
