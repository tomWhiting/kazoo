use super::PitchTrackedSynth;

impl PitchTrackedSynth {
    /// Render the first `min(input, output, carrier)` samples exactly as
    /// [`crate::Processor::process`] would, and write the raw oscillator
    /// (before the envelope and the filter) for those samples to `carrier`,
    /// for a vocoder to use as its carrier. The rest of `output` and
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
}
