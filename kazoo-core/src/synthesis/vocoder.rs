//! Vocoder synthesis: voice spectral envelope applied to a carrier signal.
//!
//! The [`Vocoder`] splits both modulator (voice input) and carrier through a
//! bank of bandpass filters. An envelope follower on each modulator band
//! extracts the amplitude contour, which is then multiplied onto the
//! corresponding carrier band. The result is the classic "talking robot" effect.

use crate::analysis::EnvelopeFollower;
use crate::effects::{BiquadFilter, FilterType};
use crate::{ParamError, ParamInfo, ParamResult, Processor, checked_param, sanitize_sample};

// ---------------------------------------------------------------------------
// Carrier mode
// ---------------------------------------------------------------------------

/// Carrier signal source for the vocoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VocoderCarrierMode {
    /// Internally generated sawtooth oscillator.
    InternalSaw,
    /// Internally generated square oscillator.
    InternalSquare,
    /// Internally generated white noise.
    InternalNoise,
    /// The input signal itself is used as both modulator and carrier.
    ExternalInput,
}

impl VocoderCarrierMode {
    #[must_use]
    fn from_param(value: f32) -> Self {
        match value.round() as i32 {
            1 => Self::InternalSquare,
            2 => Self::InternalNoise,
            3 => Self::ExternalInput,
            _ => Self::InternalSaw,
        }
    }

    #[must_use]
    const fn to_param(self) -> f32 {
        match self {
            Self::InternalSaw => 0.0,
            Self::InternalSquare => 1.0,
            Self::InternalNoise => 2.0,
            Self::ExternalInput => 3.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Noise RNG
// ---------------------------------------------------------------------------

/// Simple xorshift32 for noise generation.
#[derive(Debug, Clone)]
struct NoiseGen {
    state: u32,
}

impl NoiseGen {
    const fn new(seed: u32) -> Self {
        let s = if seed == 0 { 0xDEAD_BEEF } else { seed };
        Self { state: s }
    }

    fn next_sample(&mut self) -> f32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 17;
        self.state ^= self.state << 5;
        // Map to [-1, 1).
        (self.state as f32 / u32::MAX as f32).mul_add(2.0, -1.0)
    }
}

// ---------------------------------------------------------------------------
// Vocoder
// ---------------------------------------------------------------------------

/// Default number of filter bands.
const DEFAULT_NUM_BANDS: usize = 16;

/// Resonance of every analysis/synthesis band-pass filter: a moderate width
/// suited to vocal formant bands.
const BAND_Q: f32 = 4.0;

/// Voice spectral envelope applied to a carrier signal.
///
/// Parameters:
/// 0. `carrier_mode` — 0=Saw, 1=Square, 2=Noise, 3=External
/// 1. `carrier_frequency` — fundamental of internal carrier (20–20 000 Hz)
/// 2. `attack_ms` — envelope follower attack time (0.1–100 ms)
/// 3. `release_ms` — envelope follower release time (1–500 ms)
#[derive(Debug)]
pub struct Vocoder {
    sample_rate: f32,
    num_bands: usize,

    // Per-band processors.
    mod_filters: Vec<BiquadFilter>,
    carrier_filters: Vec<BiquadFilter>,
    envelopes: Vec<EnvelopeFollower>,

    // Per-band scratch buffers (pre-allocated for block processing).
    mod_band_buf: Vec<f32>,
    carrier_band_buf: Vec<f32>,
    carrier_block: Vec<f32>,

    // Internal carrier oscillator state.
    carrier_phase: f32,
    noise_gen: NoiseGen,

    // Parameters.
    carrier_mode: VocoderCarrierMode,
    carrier_frequency: f32,
    attack_ms: f32,
    release_ms: f32,
}

impl Vocoder {
    const PARAM_CARRIER_MODE: usize = 0;
    const PARAM_CARRIER_FREQ: usize = 1;
    const PARAM_ATTACK: usize = 2;
    const PARAM_RELEASE: usize = 3;
    const PARAM_COUNT: usize = 4;

    const DEFAULT_CARRIER_FREQ: f32 = 100.0;
    const DEFAULT_ATTACK_MS: f32 = 5.0;
    const DEFAULT_RELEASE_MS: f32 = 50.0;

    /// Create a new vocoder at the given sample rate.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            44100.0
        };

        let num_bands = DEFAULT_NUM_BANDS;
        let band_freqs = compute_band_frequencies(num_bands, sr);

        let mut mod_filters = Vec::with_capacity(num_bands);
        let mut carrier_filters = Vec::with_capacity(num_bands);
        let mut envelopes = Vec::with_capacity(num_bands);

        for &freq in &band_freqs {
            let mut mf = BiquadFilter::new(FilterType::BandPass, sr);
            mf.set_frequency(freq);
            // Q for vocal bands: moderate width.
            mf.set_q(BAND_Q);
            mod_filters.push(mf);

            let mut cf = BiquadFilter::new(FilterType::BandPass, sr);
            cf.set_frequency(freq);
            cf.set_q(BAND_Q);
            carrier_filters.push(cf);

            envelopes.push(EnvelopeFollower::new(
                Self::DEFAULT_ATTACK_MS,
                Self::DEFAULT_RELEASE_MS,
                sr,
            ));
        }

        // Pre-allocate scratch buffers sized for the default block size.
        let block_cap = crate::DEFAULT_BUFFER_SIZE;

        Self {
            sample_rate: sr,
            num_bands,
            mod_filters,
            carrier_filters,
            envelopes,
            mod_band_buf: vec![0.0; block_cap],
            carrier_band_buf: vec![0.0; block_cap],
            carrier_block: vec![0.0; block_cap],
            carrier_phase: 0.0,
            noise_gen: NoiseGen::new(0xCAFE_BABE),
            carrier_mode: VocoderCarrierMode::InternalSaw,
            carrier_frequency: Self::DEFAULT_CARRIER_FREQ,
            attack_ms: Self::DEFAULT_ATTACK_MS,
            release_ms: Self::DEFAULT_RELEASE_MS,
        }
    }

    /// Set the carrier mode directly.
    pub const fn set_carrier_mode(&mut self, mode: VocoderCarrierMode) {
        self.carrier_mode = mode;
    }

    /// Generate one sample of the internal carrier oscillator.
    fn generate_carrier_sample(&mut self) -> f32 {
        match self.carrier_mode {
            VocoderCarrierMode::InternalSaw => {
                let sample = self.carrier_phase.mul_add(2.0, -1.0);
                self.advance_carrier_phase();
                sample
            }
            VocoderCarrierMode::InternalSquare => {
                let sample = if self.carrier_phase < 0.5 { 1.0 } else { -1.0 };
                self.advance_carrier_phase();
                sample
            }
            VocoderCarrierMode::InternalNoise => self.noise_gen.next_sample(),
            VocoderCarrierMode::ExternalInput => 0.0, // handled in process()
        }
    }

    fn advance_carrier_phase(&mut self) {
        let dt = self.carrier_frequency / self.sample_rate;
        self.carrier_phase += dt;
        self.carrier_phase -= self.carrier_phase.floor();
    }

    /// Rebuild envelope followers when attack/release changes.
    /// Apply the current attack/release times and sample rate to every band's
    /// envelope follower in place. Never allocates, so it is safe from
    /// `set_param` on the audio thread.
    fn rebuild_envelopes(&mut self) {
        for envelope in &mut self.envelopes {
            envelope.set_times(self.attack_ms, self.release_ms, self.sample_rate);
        }
    }

    /// Parameter metadata, indexed by the `PARAM_*` constants.
    pub const PARAMS: [ParamInfo; Self::PARAM_COUNT] = [
        ParamInfo {
            name: "Carrier Mode",
            min: 0.0,
            max: 3.0,
            default: VocoderCarrierMode::InternalSaw.to_param(),
            unit: "",
        },
        ParamInfo {
            name: "Carrier Frequency",
            min: 20.0,
            max: 20_000.0,
            default: Self::DEFAULT_CARRIER_FREQ,
            unit: "Hz",
        },
        ParamInfo {
            name: "Attack",
            min: 0.1,
            max: 100.0,
            default: Self::DEFAULT_ATTACK_MS,
            unit: "ms",
        },
        ParamInfo {
            name: "Release",
            min: 1.0,
            max: 500.0,
            default: Self::DEFAULT_RELEASE_MS,
            unit: "ms",
        },
    ];
}

/// Fill `dest` with sanitized samples from `src`, zero-padding if `src` is
/// shorter. Free function to avoid borrow conflicts with `self`.
fn fill_sanitized_input(dest: &mut [f32], src: &[f32]) {
    let copy_len = dest.len().min(src.len());
    for (d, &s) in dest[..copy_len].iter_mut().zip(&src[..copy_len]) {
        *d = sanitize_sample(s);
    }
    for d in &mut dest[copy_len..] {
        *d = 0.0;
    }
}

/// Compute logarithmically spaced band centre frequencies.
fn compute_band_frequencies(num_bands: usize, sample_rate: f32) -> Vec<f32> {
    if num_bands == 0 {
        return Vec::new();
    }
    let nyquist = sample_rate * 0.5;
    let min_freq = 80.0_f32;
    let max_freq = nyquist.min(12_000.0);
    let log_min = min_freq.ln();
    let log_max = max_freq.ln();

    (0..num_bands)
        .map(|i| {
            let t = i as f32 / (num_bands as f32 - 1.0).max(1.0);
            let freq = t.mul_add(log_max - log_min, log_min).exp();
            freq.clamp(min_freq, nyquist - 1.0)
        })
        .collect()
}

impl Vocoder {
    /// Largest piece the scratch buffers (sized by `prepare`) can render.
    fn piece_len(&self) -> usize {
        self.mod_band_buf
            .len()
            .min(self.carrier_band_buf.len())
            .min(self.carrier_block.len())
            .max(1)
    }

    /// Render one piece no longer than [`Vocoder::piece_len`].
    fn render_piece(&mut self, input: &[f32], output: &mut [f32]) {
        let len = output.len().min(self.piece_len());

        // Generate the carrier signal for the entire block.
        if self.carrier_mode == VocoderCarrierMode::ExternalInput {
            fill_sanitized_input(&mut self.carrier_block[..len], input);
        } else {
            // Generate carrier samples. We iterate by index because
            // generate_carrier_sample() requires &mut self, creating a
            // borrow conflict with iter_mut().
            for i in 0..len {
                self.carrier_block[i] = self.generate_carrier_sample();
            }
        }

        // Prepare sanitized modulator input in mod_band_buf (reused per band).
        fill_sanitized_input(&mut self.mod_band_buf[..len], input);

        // Zero the output buffer before accumulating.
        for sample in &mut output[..len] {
            *sample = 0.0;
        }

        // Process each band as a block: filter entire block, extract envelope,
        // multiply carrier, accumulate into output.
        //
        // Buffer usage per band iteration:
        //   1. mod_band_buf (modulator input) -> mod_filters -> carrier_band_buf (filtered mod)
        //   2. carrier_band_buf in-place -> envelopes -> carrier_band_buf (envelope values)
        //   3. carrier_block (carrier input) -> carrier_filters -> mod_band_buf (filtered carrier)
        //   4. output[i] += mod_band_buf[i] * carrier_band_buf[i]
        //   5. Re-fill mod_band_buf from input for the next band.
        for band in 0..self.num_bands {
            // 1. Filter modulator through bandpass (entire block).
            self.mod_filters[band]
                .process(&self.mod_band_buf[..len], &mut self.carrier_band_buf[..len]);

            // 2. Extract envelope from filtered modulator output in-place.
            for i in 0..len {
                self.carrier_band_buf[i] =
                    self.envelopes[band].process_sample(self.carrier_band_buf[i]);
            }

            // 3. Filter carrier through matching bandpass (entire block).
            //    mod_band_buf has been consumed by step 1, safe to overwrite.
            //    carrier_block is never modified, so it's stable across bands.
            self.carrier_filters[band]
                .process(&self.carrier_block[..len], &mut self.mod_band_buf[..len]);

            // 4. Accumulate: filtered_carrier * envelope.
            for ((out, &carrier), &env) in output[..len]
                .iter_mut()
                .zip(&self.mod_band_buf[..len])
                .zip(&self.carrier_band_buf[..len])
            {
                *out = carrier.mul_add(env, *out);
            }

            // 5. Re-fill modulator input for the next band.
            if band + 1 < self.num_bands {
                fill_sanitized_input(&mut self.mod_band_buf[..len], input);
            }
        }

        // Final sanitization.
        for sample in &mut output[..len] {
            *sample = sanitize_sample(*sample);
        }
    }
}

impl Processor for Vocoder {
    /// Never allocates or panics: a block longer than the buffers sized by
    /// `prepare` is rendered in pieces.
    fn process(&mut self, input: &[f32], output: &mut [f32]) {
        if output.is_empty() {
            return;
        }
        let piece = self.piece_len();
        let mut start = 0;
        while start < output.len() {
            let end = (start + piece).min(output.len());
            let input_piece = &input[start.min(input.len())..end.min(input.len())];
            self.render_piece(input_piece, &mut output[start..end]);
            start = end;
        }
    }

    fn reset(&mut self) {
        for f in &mut self.mod_filters {
            f.reset();
        }
        for f in &mut self.carrier_filters {
            f.reset();
        }
        for e in &mut self.envelopes {
            e.reset();
        }
        self.carrier_phase = 0.0;
        self.noise_gen = NoiseGen::new(0xCAFE_BABE);
    }

    fn name(&self) -> &'static str {
        "Vocoder"
    }

    fn param_count(&self) -> usize {
        Self::PARAM_COUNT
    }

    fn param_info(&self, index: usize) -> Option<ParamInfo> {
        Self::PARAMS.get(index).copied()
    }

    fn param_value(&self, index: usize) -> Option<f32> {
        match index {
            Self::PARAM_CARRIER_MODE => Some(self.carrier_mode.to_param()),
            Self::PARAM_CARRIER_FREQ => Some(self.carrier_frequency),
            Self::PARAM_ATTACK => Some(self.attack_ms),
            Self::PARAM_RELEASE => Some(self.release_ms),
            _ => None,
        }
    }

    fn set_param(&mut self, index: usize, value: f32) -> ParamResult<()> {
        let clamped = checked_param(&Self::PARAMS, index, value)?;

        match index {
            Self::PARAM_CARRIER_MODE => {
                self.carrier_mode = VocoderCarrierMode::from_param(clamped);
            }
            Self::PARAM_CARRIER_FREQ => {
                self.carrier_frequency = clamped;
            }
            Self::PARAM_ATTACK => {
                self.attack_ms = clamped;
                self.rebuild_envelopes();
            }
            Self::PARAM_RELEASE => {
                self.release_ms = clamped;
                self.rebuild_envelopes();
            }
            _ => {
                return Err(ParamError::UnknownIndex {
                    index,
                    count: Self::PARAMS.len(),
                });
            }
        }
        Ok(())
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            44100.0
        };
        self.sample_rate = sr;

        let band_freqs = compute_band_frequencies(self.num_bands, sr);
        for (i, &freq) in band_freqs.iter().enumerate() {
            self.mod_filters[i].set_sample_rate(sr);
            self.mod_filters[i].set_frequency(freq);
            self.mod_filters[i].set_q(BAND_Q);
            self.carrier_filters[i].set_sample_rate(sr);
            self.carrier_filters[i].set_frequency(freq);
            self.carrier_filters[i].set_q(BAND_Q);
        }
        self.rebuild_envelopes();
        self.reset();
    }

    fn prepare(&mut self, max_block_size: usize) {
        let cap = max_block_size.max(crate::DEFAULT_BUFFER_SIZE);
        if self.mod_band_buf.len() < cap {
            self.mod_band_buf.resize(cap, 0.0);
        }
        if self.carrier_band_buf.len() < cap {
            self.carrier_band_buf.resize(cap, 0.0);
        }
        if self.carrier_block.len() < cap {
            self.carrier_block.resize(cap, 0.0);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn sine_wave(freq: f32, sr: f32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| (2.0 * PI * freq * i as f32 / sr).sin())
            .collect()
    }

    #[test]
    fn voice_modulates_carrier() {
        let sr = 44100.0;
        let mut vocoder = Vocoder::new(sr);
        vocoder.prepare(4096);
        vocoder.set_carrier_mode(VocoderCarrierMode::InternalSaw);

        // Voice input: 220 Hz sine.
        let voice = sine_wave(220.0, sr, 4096);
        let mut output = vec![0.0_f32; 4096];
        vocoder.process(&voice, &mut output);

        // With voice input, the carrier should be modulated.
        let energy: f32 = output[2048..].iter().map(|s| s * s).sum();
        assert!(
            energy > 0.001,
            "voiced input should produce output, energy = {energy}"
        );
    }

    #[test]
    fn silence_input_produces_quiet_output() {
        let sr = 44100.0;
        let mut vocoder = Vocoder::new(sr);
        vocoder.prepare(4096);

        let silence = vec![0.0_f32; 4096];
        let mut output = vec![0.0_f32; 4096];
        vocoder.process(&silence, &mut output);

        let energy: f32 = output[2048..].iter().map(|s| s * s).sum();
        assert!(
            energy < 0.01,
            "silence should produce near-silence, energy = {energy}"
        );
    }

    #[test]
    fn different_carrier_modes_work() {
        let sr = 44100.0;
        let voice = sine_wave(220.0, sr, 4096);

        for mode in [
            VocoderCarrierMode::InternalSaw,
            VocoderCarrierMode::InternalSquare,
            VocoderCarrierMode::InternalNoise,
            VocoderCarrierMode::ExternalInput,
        ] {
            let mut vocoder = Vocoder::new(sr);
            vocoder.prepare(4096);
            vocoder.set_carrier_mode(mode);

            let mut output = vec![0.0_f32; 4096];
            vocoder.process(&voice, &mut output);

            for (i, &s) in output.iter().enumerate() {
                assert!(s.is_finite(), "mode {mode:?}: output[{i}] = {s} not finite");
            }
        }
    }

    #[test]
    fn empty_buffers_no_panic() {
        let mut vocoder = Vocoder::new(44100.0);
        vocoder.process(&[], &mut []);
    }

    #[test]
    fn nan_input_handled() {
        let mut vocoder = Vocoder::new(44100.0);
        let input = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.5];
        let mut output = [0.0_f32; 4];
        vocoder.process(&input, &mut output);
        for (i, &s) in output.iter().enumerate() {
            assert!(s.is_finite(), "output[{i}] = {s}");
        }
    }

    #[test]
    fn param_count_and_info() {
        let vocoder = Vocoder::new(44100.0);
        assert_eq!(vocoder.param_count(), 4);
        for i in 0..4 {
            assert!(vocoder.param_info(i).is_some());
            assert!(vocoder.param_value(i).is_some());
        }
        assert!(vocoder.param_info(4).is_none());
        assert!(vocoder.param_value(4).is_none());
    }

    #[test]
    fn set_param_clamps() {
        let mut vocoder = Vocoder::new(44100.0);
        vocoder
            .set_param(Vocoder::PARAM_CARRIER_FREQ, 50_000.0)
            .unwrap();
        let v = vocoder.param_value(Vocoder::PARAM_CARRIER_FREQ).unwrap();
        assert!(
            (v - 20_000.0).abs() < f32::EPSILON,
            "should clamp to 20000, got {v}"
        );
    }

    #[test]
    fn set_param_invalid_index() {
        let mut vocoder = Vocoder::new(44100.0);
        assert!(vocoder.set_param(99, 0.0).is_err());
    }

    #[test]
    fn reset_clears_state() {
        let mut vocoder = Vocoder::new(44100.0);
        vocoder.prepare(2048);
        let voice = sine_wave(220.0, 44100.0, 2048);
        let mut output = vec![0.0_f32; 2048];
        vocoder.process(&voice, &mut output);

        vocoder.reset();
        assert!(vocoder.carrier_phase.abs() < f32::EPSILON);
    }

    #[test]
    fn sample_rate_change() {
        let mut vocoder = Vocoder::new(44100.0);
        vocoder.set_sample_rate(96000.0);
        vocoder.prepare(2048);
        let voice = sine_wave(220.0, 96000.0, 2048);
        let mut output = vec![0.0_f32; 2048];
        vocoder.process(&voice, &mut output);
        for &s in &output {
            assert!(s.is_finite());
        }
    }

    #[test]
    fn band_frequencies_are_valid() {
        let freqs = compute_band_frequencies(16, 44100.0);
        assert_eq!(freqs.len(), 16);
        for (i, &f) in freqs.iter().enumerate() {
            assert!(f > 0.0, "band {i} frequency should be positive");
            assert!(f < 22050.0, "band {i} frequency should be below Nyquist");
        }
        // Frequencies should be monotonically increasing.
        for i in 1..freqs.len() {
            assert!(
                freqs[i] >= freqs[i - 1],
                "bands should be monotonically increasing"
            );
        }
    }

    #[test]
    fn band_frequencies_low_sample_rate() {
        // At 8 kHz, Nyquist is 4 kHz — bands should still be valid.
        let freqs = compute_band_frequencies(16, 8000.0);
        assert_eq!(freqs.len(), 16);
        for (i, &f) in freqs.iter().enumerate() {
            assert!(f > 0.0, "band {i} freq should be positive: {f}");
            assert!(
                f < 4000.0,
                "band {i} freq should be below Nyquist (4000): {f}"
            );
        }
    }

    #[test]
    fn carrier_mode_from_param_roundtrip() {
        for mode in [
            VocoderCarrierMode::InternalSaw,
            VocoderCarrierMode::InternalSquare,
            VocoderCarrierMode::InternalNoise,
            VocoderCarrierMode::ExternalInput,
        ] {
            let param = mode.to_param();
            let recovered = VocoderCarrierMode::from_param(param);
            assert_eq!(mode, recovered, "roundtrip failed for {mode:?}");
        }
    }

    #[test]
    fn carrier_mode_from_param_out_of_range_defaults_to_saw() {
        assert_eq!(
            VocoderCarrierMode::from_param(-1.0),
            VocoderCarrierMode::InternalSaw
        );
        assert_eq!(
            VocoderCarrierMode::from_param(99.0),
            VocoderCarrierMode::InternalSaw
        );
    }

    #[test]
    fn vocoder_carrier_frequency_extremes() {
        let sr = 44100.0;
        let voice = sine_wave(220.0, sr, 4096);

        for freq in [20.0, 20_000.0] {
            let mut vocoder = Vocoder::new(sr);
            vocoder.prepare(4096);
            vocoder
                .set_param(Vocoder::PARAM_CARRIER_FREQ, freq)
                .unwrap();

            let mut output = vec![0.0_f32; 4096];
            vocoder.process(&voice, &mut output);

            for (i, &s) in output.iter().enumerate() {
                assert!(
                    s.is_finite() && s.abs() < 100.0,
                    "freq={freq}: output[{i}] = {s}"
                );
            }
        }
    }

    #[test]
    fn vocoder_attack_release_extremes() {
        let sr = 44100.0;
        let voice = sine_wave(220.0, sr, 4096);

        // Fast attack, fast release.
        let mut fast = Vocoder::new(sr);
        fast.prepare(4096);
        fast.set_param(Vocoder::PARAM_ATTACK, 0.1).unwrap();
        fast.set_param(Vocoder::PARAM_RELEASE, 1.0).unwrap();
        let mut out_fast = vec![0.0_f32; 4096];
        fast.process(&voice, &mut out_fast);

        // Slow attack, slow release.
        let mut slow = Vocoder::new(sr);
        slow.prepare(4096);
        slow.set_param(Vocoder::PARAM_ATTACK, 100.0).unwrap();
        slow.set_param(Vocoder::PARAM_RELEASE, 500.0).unwrap();
        let mut out_slow = vec![0.0_f32; 4096];
        slow.process(&voice, &mut out_slow);

        // Both should produce finite output.
        for &s in &out_fast {
            assert!(s.is_finite());
        }
        for &s in &out_slow {
            assert!(s.is_finite());
        }

        // Fast vs slow should differ.
        let diff: f32 = out_fast
            .iter()
            .zip(out_slow.iter())
            .skip(1024)
            .map(|(a, b)| (a - b).powi(2))
            .sum();
        assert!(
            diff > 0.001,
            "fast vs slow attack/release should differ, diff = {diff}"
        );
    }

    #[test]
    fn vocoder_param_info_names_not_empty() {
        let vocoder = Vocoder::new(44100.0);
        for i in 0..vocoder.param_count() {
            let info = vocoder.param_info(i).unwrap();
            assert!(!info.name.is_empty(), "param {i} has empty name");
        }
    }

    #[test]
    fn vocoder_param_values_roundtrip() {
        let mut vocoder = Vocoder::new(44100.0);
        vocoder
            .set_param(Vocoder::PARAM_CARRIER_FREQ, 440.0)
            .unwrap();
        vocoder.set_param(Vocoder::PARAM_ATTACK, 10.0).unwrap();
        vocoder.set_param(Vocoder::PARAM_RELEASE, 100.0).unwrap();

        assert!(
            (vocoder.param_value(Vocoder::PARAM_CARRIER_FREQ).unwrap() - 440.0).abs()
                < f32::EPSILON
        );
        assert!((vocoder.param_value(Vocoder::PARAM_ATTACK).unwrap() - 10.0).abs() < f32::EPSILON);
        assert!(
            (vocoder.param_value(Vocoder::PARAM_RELEASE).unwrap() - 100.0).abs() < f32::EPSILON
        );
    }

    #[test]
    fn vocoder_stability_with_noise() {
        let mut vocoder = Vocoder::new(44100.0);
        vocoder.prepare(4096);

        let mut rng: u32 = 0xDEAD_C0DE;
        let noise: Vec<f32> = (0..4096)
            .map(|_| {
                rng ^= rng << 13;
                rng ^= rng >> 17;
                rng ^= rng << 5;
                (rng as f32 / u32::MAX as f32).mul_add(2.0, -1.0)
            })
            .collect();
        let mut output = vec![0.0_f32; 4096];
        vocoder.process(&noise, &mut output);

        for (i, &s) in output.iter().enumerate() {
            assert!(
                s.is_finite() && s.abs() < 100.0,
                "noise stability: output[{i}] = {s}"
            );
        }
    }

    #[test]
    fn vocoder_name_is_not_empty() {
        let vocoder = Vocoder::new(44100.0);
        assert!(!vocoder.name().is_empty());
    }

    #[test]
    fn vocoder_long_sustained_processing() {
        let sr = 44100.0;
        let mut vocoder = Vocoder::new(sr);
        vocoder.prepare(512);

        let voice: Vec<f32> = (0..512)
            .map(|i| (2.0 * PI * 220.0 * i as f32 / sr).sin())
            .collect();
        let mut output = vec![0.0_f32; 512];

        for _ in 0..50 {
            vocoder.process(&voice, &mut output);
            for &s in &output {
                assert!(s.is_finite() && s.abs() < 100.0);
            }
        }
    }

    #[test]
    fn blocks_longer_than_prepared_are_rendered_in_pieces() {
        let mut long = Vocoder::new(44_100.0);
        long.prepare(64);
        let buffers = (long.mod_band_buf.as_ptr(), long.mod_band_buf.len());
        let input: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let mut output = vec![0.0_f32; input.len()];
        long.process(&input, &mut output);
        assert_eq!(
            (long.mod_band_buf.as_ptr(), long.mod_band_buf.len()),
            buffers,
            "process() reallocated"
        );

        // The same audio rendered with buffers big enough for one piece.
        let mut whole = Vocoder::new(44_100.0);
        whole.prepare(1000);
        let mut expected = vec![0.0_f32; input.len()];
        whole.process(&input, &mut expected);
        for (i, (&a, &b)) in output.iter().zip(&expected).enumerate() {
            assert!((a - b).abs() < 1e-4, "[{i}] {a} != {b}");
        }
    }

    #[test]
    fn a_short_input_is_padded_with_silence_across_pieces() {
        let mut vocoder = Vocoder::new(44_100.0);
        vocoder.prepare(64);
        let input = vec![0.5_f32; 100];
        let mut output = vec![1.0_f32; 300];
        vocoder.process(&input, &mut output);
        assert!(output.iter().all(|s| s.is_finite()));
    }
}
