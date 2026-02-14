//! Vocoder synthesis: voice spectral envelope applied to a carrier signal.
//!
//! The [`Vocoder`] splits both modulator (voice input) and carrier through a
//! bank of bandpass filters. An envelope follower on each modulator band
//! extracts the amplitude contour, which is then multiplied onto the
//! corresponding carrier band. The result is the classic "talking robot" effect.

use crate::analysis::EnvelopeFollower;
use crate::effects::{BiquadFilter, FilterType};
use crate::{Error, ParamInfo, Processor, Result, sanitize_sample};

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

    // Per-band scratch buffers (pre-allocated).
    mod_band_buf: Vec<f32>,
    carrier_band_buf: Vec<f32>,

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
            let _ = mf.set_param(0, freq);
            // Q for vocal bands: moderate width.
            let _ = mf.set_param(1, 4.0);
            mod_filters.push(mf);

            let mut cf = BiquadFilter::new(FilterType::BandPass, sr);
            let _ = cf.set_param(0, freq);
            let _ = cf.set_param(1, 4.0);
            carrier_filters.push(cf);

            envelopes.push(EnvelopeFollower::new(
                Self::DEFAULT_ATTACK_MS,
                Self::DEFAULT_RELEASE_MS,
                sr,
            ));
        }

        Self {
            sample_rate: sr,
            num_bands,
            mod_filters,
            carrier_filters,
            envelopes,
            mod_band_buf: vec![0.0; 1],
            carrier_band_buf: vec![0.0; 1],
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
    fn rebuild_envelopes(&mut self) {
        self.envelopes.clear();
        for _ in 0..self.num_bands {
            self.envelopes.push(EnvelopeFollower::new(
                self.attack_ms,
                self.release_ms,
                self.sample_rate,
            ));
        }
    }

    fn param_infos() -> [ParamInfo; Self::PARAM_COUNT] {
        [
            ParamInfo {
                name: "Carrier Mode".into(),
                min: 0.0,
                max: 3.0,
                default: VocoderCarrierMode::InternalSaw.to_param(),
                unit: String::new(),
            },
            ParamInfo {
                name: "Carrier Frequency".into(),
                min: 20.0,
                max: 20_000.0,
                default: Self::DEFAULT_CARRIER_FREQ,
                unit: "Hz".into(),
            },
            ParamInfo {
                name: "Attack".into(),
                min: 0.1,
                max: 100.0,
                default: Self::DEFAULT_ATTACK_MS,
                unit: "ms".into(),
            },
            ParamInfo {
                name: "Release".into(),
                min: 1.0,
                max: 500.0,
                default: Self::DEFAULT_RELEASE_MS,
                unit: "ms".into(),
            },
        ]
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

impl Processor for Vocoder {
    fn process(&mut self, input: &[f32], output: &mut [f32]) {
        if output.is_empty() {
            return;
        }

        let input_len = input.len();

        for (i, out_sample) in output.iter_mut().enumerate() {
            let modulator_sample = if i < input_len {
                sanitize_sample(input[i])
            } else {
                0.0
            };

            // Generate or use carrier sample.
            let carrier_sample = if self.carrier_mode == VocoderCarrierMode::ExternalInput {
                modulator_sample
            } else {
                self.generate_carrier_sample()
            };

            let mut sum = 0.0_f32;

            for band in 0..self.num_bands {
                // Filter modulator through bandpass.
                self.mod_band_buf[0] = modulator_sample;
                let mut mod_out = [0.0_f32; 1];
                self.mod_filters[band].process(&self.mod_band_buf, &mut mod_out);

                // Track envelope of the modulator band.
                let env = self.envelopes[band].process_sample(mod_out[0]);

                // Filter carrier through matching bandpass.
                self.carrier_band_buf[0] = carrier_sample;
                let mut car_out = [0.0_f32; 1];
                self.carrier_filters[band].process(&self.carrier_band_buf, &mut car_out);

                // Multiply carrier band by modulator envelope.
                sum += car_out[0] * env;
            }

            *out_sample = sanitize_sample(sum);
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
        let infos = Self::param_infos();
        infos.get(index).cloned()
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

    fn set_param(&mut self, index: usize, value: f32) -> Result<()> {
        let infos = Self::param_infos();
        let info = infos
            .get(index)
            .ok_or_else(|| Error::Config(format!("invalid param index {index}")))?;
        let clamped = info.clamp(value);

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
            _ => unreachable!(),
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
            let _ = self.mod_filters[i].set_param(0, freq);
            let _ = self.mod_filters[i].set_param(1, 4.0);
            self.carrier_filters[i].set_sample_rate(sr);
            let _ = self.carrier_filters[i].set_param(0, freq);
            let _ = self.carrier_filters[i].set_param(1, 4.0);
        }
        self.rebuild_envelopes();
        self.reset();
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
}
