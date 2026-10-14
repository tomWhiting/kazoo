use super::Vocoder;

/// How many bands a [`Vocoder`] built by [`Vocoder::with_bands`] splits the
/// spectrum into, log-spaced from 80 Hz to 12 kHz (or just under Nyquist).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VocoderBands {
    /// Eight bands: the coarsest, most robotic.
    Eight,
    /// Sixteen bands, as [`Vocoder::new`] has.
    Sixteen,
    /// Twenty-four bands: the clearest words.
    TwentyFour,
}

/// Resonance of each of 16 or 24 bands of a vocoder built with
/// [`Vocoder::with_bands`]: narrower than [`Vocoder::new`]'s, so a low
/// carrier harmonic stays out of the formants above it.
const NARROW_BAND_Q: f32 = 8.0;

/// Resonance of each of 8 bands: an octave apart, bands as narrow as
/// [`NARROW_BAND_Q`] would let most of the voice fall between them and the
/// vocoder lose 4 to 6 dB of the voice, so they are as wide as
/// [`Vocoder::new`]'s.
const WIDE_BAND_Q: f32 = 4.0;

impl Vocoder {
    /// A vocoder with the given band count, its bands narrowed for use with
    /// [`Vocoder::process_with_carrier`]. Allocates; call it before the audio
    /// thread starts, then [`crate::Processor::prepare`] it for the largest block.
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
        vocoder.band_q = if bands == VocoderBands::Eight {
            WIDE_BAND_Q
        } else {
            NARROW_BAND_Q
        };
        for filter in vocoder
            .mod_filters
            .iter_mut()
            .chain(&mut vocoder.carrier_filters)
            .chain(&mut vocoder.carrier_stage_two)
        {
            filter.set_q(vocoder.band_q);
        }
        vocoder
    }

    /// Impose `modulator`'s spectral envelope on a separate `carrier`, writing
    /// `output.len()` samples. Each carrier band is brought to the
    /// modulator's level in that band, so the output follows the
    /// modulator's formants and loudness whatever the carrier's own
    /// spectral tilt; a band where the carrier has no harmonic of its own
    /// (more than 18 dB under its loudest band within an octave) is not
    /// raised, so a missing harmonic is not filled with its neighbours'
    /// skirts, and a carrier with nothing in a band leaves it near silent. A `modulator` or `carrier` shorter than `output` is
    /// read as silence past its end. NaN and Inf in either are read as
    /// silence, and the output is always finite. Never allocates, locks or
    /// panics: a block longer than [`crate::Processor::prepare`] sized is rendered
    /// in pieces. Ignores the carrier mode.
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

#[cfg(test)]
mod tests;
