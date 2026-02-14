//! Analysis thread: pitch detection, spectrum analysis, formant extraction.
//!
//! Runs in a dedicated thread at lower priority than the processing thread.
//! Reads raw mic samples from a ring buffer, runs the analysis pipeline, and
//! pushes results back into ring buffers consumed by the processing thread.

use ringbuf::traits::{Consumer, Producer};
use ringbuf::{HeapCons, HeapProd};

use crate::analysis::{
    FormantData, FormantExtractor, OnsetDetector, PitchDetector, PitchDetectorConfig,
    PitchEstimate, SpectrumAnalyzer,
};

/// Configuration for the analysis pipeline.
#[derive(Debug, Clone)]
pub struct AnalysisConfig {
    /// Pitch detector configuration.
    pub pitch: PitchDetectorConfig,
    /// FFT size for spectrum analysis (must be >= 2, typically 2048).
    pub spectrum_fft_size: usize,
    /// EMA smoothing factor for spectrum display (0.0 = no smoothing, 1.0 = max).
    pub spectrum_smoothing: f32,
    /// FFT size for onset detection.
    pub onset_fft_size: usize,
    /// Threshold factor for onset detection.
    pub onset_threshold: f32,
    /// Audio sample rate in Hz.
    pub sample_rate: u32,
    /// Audio buffer size (used for sizing the internal read buffer).
    pub buffer_size: usize,
}

/// Entry point for the analysis thread.
///
/// Reads raw mic audio from `input_cons`, runs pitch detection, spectrum
/// analysis, onset detection, and formant extraction, then pushes results
/// into the respective ring buffer producers.
///
/// The thread exits when `input_cons` is empty and the producer side has
/// been dropped (indicating engine shutdown), or when it detects that all
/// result producers have been dropped.
///
/// # Arguments
///
/// * `input_cons` -- ring buffer consumer for raw mic samples (f32)
/// * `pitch_prod` -- ring buffer producer for `PitchEstimate` results
/// * `spectrum_prod` -- ring buffer producer for spectrum magnitude `Vec<f32>`
/// * `formant_prod` -- ring buffer producer for `Option<FormantData>` results
/// * `config` -- analysis pipeline configuration
pub fn run(
    mut input_cons: HeapCons<f32>,
    mut pitch_prod: HeapProd<PitchEstimate>,
    mut spectrum_prod: HeapProd<Vec<f32>>,
    mut formant_prod: HeapProd<Option<FormantData>>,
    config: AnalysisConfig,
) {
    let sr = config.sample_rate;
    #[allow(clippy::cast_precision_loss)]
    let sr_f32 = sr as f32;

    // Initialise analysis components. Pitch detector can fail if the config
    // is invalid, so we handle that gracefully.
    let mut pitch_detector = PitchDetector::new(config.pitch).ok();

    let mut spectrum_analyzer = SpectrumAnalyzer::new(
        config.spectrum_fft_size.max(2),
        sr_f32,
        config.spectrum_smoothing.clamp(0.0, 1.0),
    );

    let onset_fft = config.onset_fft_size.max(4);
    let onset_hop = onset_fft / 4;
    let mut onset_detector =
        OnsetDetector::new(onset_fft, onset_hop, sr_f32, config.onset_threshold);

    // Formant extractor: LPC order 24 is reasonable for 44.1kHz speech.
    // Frame size of 1024 gives ~23ms frames at 44.1kHz.
    let lpc_order = 24;
    let formant_frame_size = 1024;
    let mut formant_extractor = FormantExtractor::new(lpc_order, formant_frame_size, sr_f32);

    // Pre-allocate read buffer sized to one processing block.
    let read_buf_size = config.buffer_size.max(256);
    let mut read_buf = vec![0.0_f32; read_buf_size];

    // Track consecutive empty reads to detect shutdown via ring buffer exhaustion.
    let mut consecutive_empty = 0_u32;
    let max_consecutive_empty: u32 = 500; // ~500ms at 1ms sleep

    loop {
        let num_read = input_cons.pop_slice(&mut read_buf);

        if num_read == 0 {
            consecutive_empty += 1;
            if consecutive_empty >= max_consecutive_empty {
                // The producer side has likely been dropped; exit gracefully.
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
            continue;
        }
        consecutive_empty = 0;

        let samples = &read_buf[..num_read];

        // Pitch detection.
        if let Some(ref mut detector) = pitch_detector {
            if let Some(estimate) = detector.push_samples(samples) {
                // Best-effort push: if the ring buffer is full, the consumer
                // has not drained it yet. We discard the result silently;
                // the UI always uses the latest available estimate.
                let _ = pitch_prod.try_push(estimate);
            }
        }

        // Spectrum analysis.
        if let Some(spectrum_data) = spectrum_analyzer.push_samples(samples) {
            let _ = spectrum_prod.try_push(spectrum_data.magnitudes_db);
        }

        // Onset detection (results currently not displayed but computed for
        // future use; kept to validate the pipeline end-to-end).
        let _onsets = onset_detector.push_samples(samples);

        // Formant extraction.
        let formant_result = formant_extractor.push_samples(samples);
        if formant_result.is_some() {
            let _ = formant_prod.try_push(formant_result);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analysis_config_construction() {
        let config = AnalysisConfig {
            pitch: PitchDetectorConfig::default(),
            spectrum_fft_size: 2048,
            spectrum_smoothing: 0.8,
            onset_fft_size: 1024,
            onset_threshold: 0.3,
            sample_rate: 44_100,
            buffer_size: 256,
        };

        assert_eq!(config.spectrum_fft_size, 2048);
        assert!((config.spectrum_smoothing - 0.8).abs() < f32::EPSILON);
        assert_eq!(config.onset_fft_size, 1024);
        assert!((config.onset_threshold - 0.3).abs() < f32::EPSILON);
        assert_eq!(config.sample_rate, 44_100);
        assert_eq!(config.buffer_size, 256);
    }

    #[test]
    fn analysis_config_debug_format() {
        let config = AnalysisConfig {
            pitch: PitchDetectorConfig::default(),
            spectrum_fft_size: 2048,
            spectrum_smoothing: 0.8,
            onset_fft_size: 1024,
            onset_threshold: 0.3,
            sample_rate: 44_100,
            buffer_size: 256,
        };

        let dbg = format!("{config:?}");
        assert!(dbg.contains("AnalysisConfig"));
    }

    #[test]
    fn analysis_config_clone() {
        let config = AnalysisConfig {
            pitch: PitchDetectorConfig::default(),
            spectrum_fft_size: 4096,
            spectrum_smoothing: 0.5,
            onset_fft_size: 2048,
            onset_threshold: 0.5,
            sample_rate: 48_000,
            buffer_size: 512,
        };

        let cloned = config.clone();
        assert_eq!(cloned.spectrum_fft_size, 4096);
        assert_eq!(cloned.sample_rate, 48_000);
    }
}
