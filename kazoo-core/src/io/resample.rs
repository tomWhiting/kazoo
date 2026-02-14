//! Sample rate conversion and channel format utilities.
//!
//! These functions run at file-load time on the UI thread. They are NOT
//! called from the audio output callback and may allocate freely.

use audioadapter_buffers::direct::SequentialSliceOfVecs;
use rubato::{Fft, FixedSync, Resampler};

use crate::{Error, Result, sanitize_buffer, sanitize_sample};

/// Default chunk size for the FFT resampler (in frames).
///
/// 1024 provides a good balance between quality and memory usage for
/// offline resampling at file-load time.
const CHUNK_SIZE: usize = 1024;

/// Number of sub-chunks for the FFT resampler.
///
/// Using 2 sub-chunks reduces anti-aliasing filter cutoff frequency slightly
/// but keeps latency manageable for offline use.
const SUB_CHUNKS: usize = 2;

/// Resample mono audio from `source_rate` to `target_rate`.
///
/// Uses rubato's FFT-based synchronous resampler for high quality. Returns a
/// new `Vec` containing the resampled audio. The resampler handles
/// anti-aliasing filtering internally.
///
/// This function runs at file-load time and allocates freely.
///
/// # Errors
///
/// Returns [`Error::Config`] if either sample rate is zero.
/// Returns [`Error::AudioFormat`] if the resampler fails during processing.
pub fn resample_mono(samples: &[f32], source_rate: u32, target_rate: u32) -> Result<Vec<f32>> {
    // Validate rates before anything else -- zero is never valid.
    if source_rate == 0 || target_rate == 0 {
        return Err(Error::Config(format!(
            "sample rates must be non-zero (source={source_rate}, target={target_rate})"
        )));
    }

    // Same rate: return a sanitized clone.
    if source_rate == target_rate {
        let mut out = samples.to_vec();
        sanitize_buffer(&mut out);
        return Ok(out);
    }

    // Empty input: nothing to resample.
    if samples.is_empty() {
        return Ok(Vec::new());
    }

    let source_rate_usize = source_rate as usize;
    let target_rate_usize = target_rate as usize;
    let num_channels: usize = 1;

    // Construct the FFT synchronous resampler with fixed input chunk size.
    let mut resampler = Fft::<f32>::new(
        source_rate_usize,
        target_rate_usize,
        CHUNK_SIZE,
        SUB_CHUNKS,
        num_channels,
        FixedSync::Input,
    )
    .map_err(|e| Error::AudioFormat(format!("failed to create resampler: {e}")))?;

    // Prepare input: rubato expects channel-separated data (Vec<Vec<f32>>).
    // Sanitize input samples to replace NaN/Inf with 0.0 before resampling.
    let input_channel: Vec<f32> = samples.iter().copied().map(sanitize_sample).collect();
    let input_data = vec![input_channel];
    let input_len = samples.len();

    // Calculate required output buffer length and allocate.
    let output_len = resampler.process_all_needed_output_len(input_len);
    let mut output_data = vec![vec![0.0f32; output_len]; num_channels];

    // Wrap in audioadapter types for rubato's API.
    let input_adapter = SequentialSliceOfVecs::new(&input_data, num_channels, input_len)
        .map_err(|e| Error::AudioFormat(format!("input adapter creation failed: {e}")))?;
    let mut output_adapter =
        SequentialSliceOfVecs::new_mut(&mut output_data, num_channels, output_len)
            .map_err(|e| Error::AudioFormat(format!("output adapter creation failed: {e}")))?;

    // Resample the entire clip in one call.
    let (_nbr_in, nbr_out) = resampler
        .process_all_into_buffer(&input_adapter, &mut output_adapter, input_len, None)
        .map_err(|e| Error::AudioFormat(format!("resampling failed: {e}")))?;

    // Extract the resampled output, trimmed to the actual output length.
    let mut result = output_data.into_iter().next().unwrap_or_default();
    result.truncate(nbr_out);

    // Final sanitization pass on the output.
    sanitize_buffer(&mut result);

    Ok(result)
}

/// Convert interleaved multi-channel audio to mono by averaging channels.
///
/// The input is expected to be interleaved: for stereo, samples alternate
/// `[L0, R0, L1, R1, ...]`. The output contains one sample per frame,
/// computed as the arithmetic mean of all channels in that frame.
///
/// If `channels` is 0 or 1, the input is returned as a sanitized clone
/// (already mono or degenerate).
///
/// Any trailing samples that do not form a complete frame are silently
/// discarded.
#[must_use]
pub fn to_mono(samples: &[f32], channels: u16) -> Vec<f32> {
    // Zero or mono: return sanitized clone.
    if channels <= 1 {
        return samples.iter().copied().map(sanitize_sample).collect();
    }

    let ch = channels as usize;
    let num_frames = samples.len() / ch;
    let mut output = Vec::with_capacity(num_frames);
    let inverse_channels = 1.0 / f32::from(channels);

    for frame in 0..num_frames {
        let base = frame * ch;
        let mut sum: f32 = 0.0;
        for c in 0..ch {
            sum += sanitize_sample(samples[base + c]);
        }
        output.push(sanitize_sample(sum * inverse_channels));
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- resample_mono tests --

    #[test]
    fn same_rate_returns_input_unchanged() {
        let input = vec![0.1, 0.2, 0.3, 0.4, 0.5];
        let result = resample_mono(&input, 44100, 44100).unwrap();
        assert_eq!(result.len(), input.len());
        for (a, b) in result.iter().zip(input.iter()) {
            assert!((a - b).abs() < f32::EPSILON, "expected {b}, got {a}");
        }
    }

    #[test]
    fn zero_source_rate_returns_error() {
        let input = vec![0.1, 0.2, 0.3];
        let result = resample_mono(&input, 0, 48000);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("non-zero"),
            "error should mention non-zero rates, got: {err}"
        );
    }

    #[test]
    fn zero_target_rate_returns_error() {
        let input = vec![0.1, 0.2, 0.3];
        let result = resample_mono(&input, 44100, 0);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("non-zero"),
            "error should mention non-zero rates, got: {err}"
        );
    }

    #[test]
    fn both_rates_zero_returns_error() {
        let input = vec![0.1, 0.2, 0.3];
        let result = resample_mono(&input, 0, 0);
        assert!(result.is_err());
    }

    #[test]
    fn empty_input_returns_empty_output() {
        let result = resample_mono(&[], 44100, 48000).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn resample_44100_to_48000_output_length() {
        // Generate a 1-second sine wave at 440 Hz, sampled at 44100 Hz.
        let source_rate = 44100_u32;
        let target_rate = 48000_u32;
        let num_samples = source_rate as usize;
        let input: Vec<f32> = (0..num_samples)
            .map(|i| {
                let t = i as f32 / source_rate as f32;
                (2.0 * std::f32::consts::PI * 440.0 * t).sin()
            })
            .collect();

        let result = resample_mono(&input, source_rate, target_rate).unwrap();

        // Expected output length: approximately input_len * target_rate / source_rate.
        let expected_len =
            (num_samples as f64 * f64::from(target_rate) / f64::from(source_rate)).ceil() as usize;
        let tolerance = expected_len / 50; // 2% tolerance
        assert!(
            result.len().abs_diff(expected_len) <= tolerance,
            "44100->48000: expected ~{expected_len} samples, got {}",
            result.len()
        );
    }

    #[test]
    fn resample_48000_to_44100_output_length() {
        // Generate a 1-second sine wave at 440 Hz, sampled at 48000 Hz.
        let source_rate = 48000_u32;
        let target_rate = 44100_u32;
        let num_samples = source_rate as usize;
        let input: Vec<f32> = (0..num_samples)
            .map(|i| {
                let t = i as f32 / source_rate as f32;
                (2.0 * std::f32::consts::PI * 440.0 * t).sin()
            })
            .collect();

        let result = resample_mono(&input, source_rate, target_rate).unwrap();

        let expected_len =
            (num_samples as f64 * f64::from(target_rate) / f64::from(source_rate)).ceil() as usize;
        let tolerance = expected_len / 50; // 2% tolerance
        assert!(
            result.len().abs_diff(expected_len) <= tolerance,
            "48000->44100: expected ~{expected_len} samples, got {}",
            result.len()
        );
    }

    #[test]
    fn roundtrip_preserves_signal() {
        // Generate a low-frequency sine wave (well below Nyquist at both rates)
        // so the round-trip should preserve the signal shape closely.
        let rate_a = 44100_u32;
        let rate_b = 48000_u32;
        let duration_samples = rate_a as usize; // 1 second
        let freq = 100.0_f32; // 100 Hz is well below both Nyquist frequencies

        let original: Vec<f32> = (0..duration_samples)
            .map(|i| {
                let t = i as f32 / rate_a as f32;
                (2.0 * std::f32::consts::PI * freq * t).sin()
            })
            .collect();

        // Resample A -> B -> A.
        let intermediate = resample_mono(&original, rate_a, rate_b).unwrap();
        let recovered = resample_mono(&intermediate, rate_b, rate_a).unwrap();

        // Compare signals in the middle (avoiding edge transients from the
        // resampler's filter delay).
        let margin = duration_samples / 10; // skip first/last 10%
        let compare_len = original.len().min(recovered.len());
        assert!(
            compare_len > 2 * margin,
            "recovered signal too short for comparison: {compare_len}"
        );

        let mut max_diff: f32 = 0.0;
        for i in margin..(compare_len - margin) {
            let diff = (original[i] - recovered[i]).abs();
            if diff > max_diff {
                max_diff = diff;
            }
        }

        // FFT resampler introduces some error; for a clean low-frequency sine
        // the round-trip error should be well under 0.1.
        assert!(
            max_diff < 0.1,
            "round-trip max error {max_diff} exceeds threshold 0.1"
        );
    }

    #[test]
    fn resample_sanitizes_nan_input() {
        let mut input = vec![0.5; 4096];
        input[100] = f32::NAN;
        input[200] = f32::INFINITY;
        input[300] = f32::NEG_INFINITY;

        let result = resample_mono(&input, 44100, 48000).unwrap();

        // All output samples must be finite.
        for (i, &sample) in result.iter().enumerate() {
            assert!(
                sample.is_finite(),
                "output sample at index {i} is not finite: {sample}"
            );
        }
    }

    #[test]
    fn same_rate_sanitizes_nan_input() {
        let input = vec![1.0, f32::NAN, -0.5, f32::INFINITY];
        let result = resample_mono(&input, 44100, 44100).unwrap();
        for (i, &sample) in result.iter().enumerate() {
            assert!(
                sample.is_finite(),
                "same-rate output sample at index {i} is not finite: {sample}"
            );
        }
    }

    // -- to_mono tests --

    #[test]
    fn to_mono_single_channel_returns_clone() {
        let input = vec![0.1, 0.2, 0.3, 0.4];
        let result = to_mono(&input, 1);
        assert_eq!(result.len(), input.len());
        for (a, b) in result.iter().zip(input.iter()) {
            assert!((a - b).abs() < f32::EPSILON, "expected {b}, got {a}");
        }
    }

    #[test]
    fn to_mono_zero_channels_returns_clone() {
        let input = vec![0.1, 0.2, 0.3, 0.4];
        let result = to_mono(&input, 0);
        assert_eq!(result.len(), input.len());
        for (a, b) in result.iter().zip(input.iter()) {
            assert!((a - b).abs() < f32::EPSILON, "expected {b}, got {a}");
        }
    }

    #[test]
    fn to_mono_stereo_averages_channels() {
        // Interleaved stereo: [L0, R0, L1, R1, L2, R2]
        let input = vec![0.2, 0.8, 0.4, 0.6, 1.0, 0.0];
        let result = to_mono(&input, 2);
        assert_eq!(result.len(), 3);
        assert!(
            (result[0] - 0.5).abs() < f32::EPSILON,
            "frame 0: expected 0.5, got {}",
            result[0]
        );
        assert!(
            (result[1] - 0.5).abs() < f32::EPSILON,
            "frame 1: expected 0.5, got {}",
            result[1]
        );
        assert!(
            (result[2] - 0.5).abs() < f32::EPSILON,
            "frame 2: expected 0.5, got {}",
            result[2]
        );
    }

    #[test]
    fn to_mono_three_channels() {
        // Interleaved 3-channel: [C0a, C0b, C0c, C1a, C1b, C1c]
        let input = vec![0.3, 0.6, 0.9, 1.0, 0.5, 0.2];
        let result = to_mono(&input, 3);
        assert_eq!(result.len(), 2);
        // Frame 0: (0.3 + 0.6 + 0.9) / 3 = 0.6
        assert!(
            (result[0] - 0.6).abs() < 1e-6,
            "frame 0: expected 0.6, got {}",
            result[0]
        );
        // Frame 1: (1.0 + 0.5 + 0.2) / 3 ≈ 0.5666...
        let expected = (1.0 + 0.5 + 0.2) / 3.0;
        assert!(
            (result[1] - expected).abs() < 1e-6,
            "frame 1: expected {expected}, got {}",
            result[1]
        );
    }

    #[test]
    fn to_mono_discards_trailing_samples() {
        // 5 samples with 2 channels: only 2 complete frames, 1 sample discarded.
        let input = vec![0.2, 0.8, 0.4, 0.6, 0.9];
        let result = to_mono(&input, 2);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn to_mono_empty_input() {
        let result = to_mono(&[], 2);
        assert!(result.is_empty());
    }

    #[test]
    fn to_mono_sanitizes_nan() {
        let input = vec![f32::NAN, 0.4, 0.6, f32::INFINITY];
        let result = to_mono(&input, 2);
        assert_eq!(result.len(), 2);
        for (i, &sample) in result.iter().enumerate() {
            assert!(
                sample.is_finite(),
                "to_mono output sample at index {i} is not finite: {sample}"
            );
        }
        // Frame 0: (0.0 + 0.4) / 2 = 0.2 (NaN sanitized to 0.0)
        assert!(
            (result[0] - 0.2).abs() < f32::EPSILON,
            "frame 0: expected 0.2, got {}",
            result[0]
        );
        // Frame 1: (0.6 + 0.0) / 2 = 0.3 (Inf sanitized to 0.0)
        assert!(
            (result[1] - 0.3).abs() < f32::EPSILON,
            "frame 1: expected 0.3, got {}",
            result[1]
        );
    }

    #[test]
    fn to_mono_mono_sanitizes_nan() {
        let input = vec![1.0, f32::NAN, f32::INFINITY, -0.5];
        let result = to_mono(&input, 1);
        assert_eq!(result.len(), 4);
        assert!((result[0] - 1.0).abs() < f32::EPSILON);
        assert!((result[1] - 0.0).abs() < f32::EPSILON);
        assert!((result[2] - 0.0).abs() < f32::EPSILON);
        assert!((result[3] - (-0.5)).abs() < f32::EPSILON);
    }
}
