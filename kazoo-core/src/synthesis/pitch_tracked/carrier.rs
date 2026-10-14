use super::PitchTrackedSynth;

impl PitchTrackedSynth {
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
