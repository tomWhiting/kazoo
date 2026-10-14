use super::Vocoder;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VocoderBands {
    Eight,
    Sixteen,
    TwentyFour,
}

impl Vocoder {
    #[must_use]
    pub fn with_bands(sample_rate: f32, bands: VocoderBands) -> Self {
        let mut vocoder = Self::build(
            sample_rate,
            match bands {
                VocoderBands::Eight => 8,
                VocoderBands::Sixteen => 16,
                VocoderBands::TwentyFour => 24,
            },
        );
        // Narrower bands keep low carrier harmonics out of higher formants.
        vocoder.band_q = 8.0;
        for filter in vocoder
            .mod_filters
            .iter_mut()
            .chain(&mut vocoder.carrier_filters)
        {
            filter.set_q(vocoder.band_q);
        }
        vocoder
    }

    pub fn process_with_carrier(&mut self, modulator: &[f32], carrier: &[f32], output: &mut [f32]) {
        let piece = self.piece_len();
        let mut start = 0;
        while start < output.len() {
            let end = (start + piece).min(output.len());
            let modulator = &modulator[start.min(modulator.len())..end.min(modulator.len())];
            let carrier = &carrier[start.min(carrier.len())..end.min(carrier.len())];
            self.render_piece(modulator, &mut output[start..end], Some(carrier));
            start = end;
        }
    }
}
