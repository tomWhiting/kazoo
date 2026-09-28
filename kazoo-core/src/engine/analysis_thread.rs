//! Analysis thread: pitch detection, spectrum analysis, formant extraction.
//!
//! Runs in a dedicated thread at lower priority than the audio callback.
//! Reads raw mic samples from a ring buffer, runs the analysis pipeline, and
//! pushes results back into ring buffers consumed by the output callback.

use std::sync::Arc;

use ringbuf::traits::{Consumer, Observer, Producer};
use ringbuf::{HeapCons, HeapProd};

use super::stats::EngineStats;
use crate::analysis::{
    FormantData, FormantExtractor, OnsetDetector, PitchDetector, PitchEstimate, SpectrumAnalyzer,
};

/// Configuration for the analysis pipeline.
///
/// The pitch detector is constructed (and its configuration validated) by
/// the caller and handed to [`run`] directly.
#[derive(Debug, Clone)]
pub struct AnalysisConfig {
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

/// Ring-buffer endpoints and counters owned by the analysis thread.
pub struct AnalysisIo {
    /// Raw mic (or clip) samples from the output callback.
    pub input_cons: HeapCons<f32>,
    /// Pitch estimates back to the output callback.
    pub pitch_prod: HeapProd<PitchEstimate>,
    /// Spectrum magnitudes (dB) back to the output callback.
    pub spectrum_prod: HeapProd<Vec<f32>>,
    /// Formant results back to the output callback.
    pub formant_prod: HeapProd<Option<FormantData>>,
    /// Shared counters; results that do not fit are counted here.
    pub stats: Arc<EngineStats>,
}

// Ring buffer endpoints are not `Debug`; implement manually.
impl std::fmt::Debug for AnalysisIo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnalysisIo").finish_non_exhaustive()
    }
}

/// Entry point for the analysis thread.
///
/// Reads raw mic audio from `io.input_cons`, runs pitch detection, spectrum
/// analysis, onset detection, and formant extraction, then pushes results
/// into the respective ring buffer producers. Results that do not fit
/// because the output callback has not drained the previous ones are
/// counted in [`EngineStats`] (the callback always uses the latest value, so
/// nothing else needs to happen).
///
/// The thread exits once the input ring is drained and its producer (owned
/// by the output callback) has been dropped at engine shutdown. A temporarily
/// silent input — e.g. a stalled or disconnected microphone — does not stop
/// analysis.
pub fn run(mut io: AnalysisIo, mut pitch_detector: PitchDetector, config: &AnalysisConfig) {
    let sr_f32 = config.sample_rate as f32;

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

    loop {
        let num_read = io.input_cons.pop_slice(&mut read_buf);

        if num_read == 0 {
            if !io.input_cons.write_is_held() {
                // Drained and the producer is gone: the engine shut down.
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
            continue;
        }

        let samples = &read_buf[..num_read];

        if let Some(estimate) = pitch_detector.push_samples(samples) {
            if io.pitch_prod.try_push(estimate).is_err() {
                io.stats.analysis_result_dropped();
            }
        }

        if let Some(spectrum_data) = spectrum_analyzer.push_samples(samples) {
            if io
                .spectrum_prod
                .try_push(spectrum_data.magnitudes_db)
                .is_err()
            {
                io.stats.analysis_result_dropped();
            }
        }

        // Onset detection (results currently not displayed but computed for
        // future use; kept to validate the pipeline end-to-end).
        let _onsets = onset_detector.push_samples(samples);

        let formant_result = formant_extractor.push_samples(samples);
        if formant_result.is_some() && io.formant_prod.try_push(formant_result).is_err() {
            io.stats.analysis_result_dropped();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::PitchDetectorConfig;
    use ringbuf::HeapRb;
    use ringbuf::traits::Split;

    #[test]
    fn run_drains_input_and_exits_when_producer_dropped() {
        let (mut in_prod, input_cons) = HeapRb::<f32>::new(8192).split();
        // Result rings of capacity 1: later results cannot fit and must be
        // counted rather than silently discarded.
        let (pitch_prod, _pitch_cons) = HeapRb::<PitchEstimate>::new(1).split();
        let (spectrum_prod, mut spectrum_cons) = HeapRb::<Vec<f32>>::new(1).split();
        let (formant_prod, _formant_cons) = HeapRb::<Option<FormantData>>::new(1).split();
        let stats = Arc::new(EngineStats::new());

        let sine: Vec<f32> = (0..8192)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / 44_100.0).sin() * 0.5)
            .collect();
        assert_eq!(in_prod.push_slice(&sine), sine.len());
        drop(in_prod);

        let config = AnalysisConfig {
            spectrum_fft_size: 1024,
            spectrum_smoothing: 0.0,
            onset_fft_size: 1024,
            onset_threshold: 0.3,
            sample_rate: 44_100,
            buffer_size: 256,
        };
        let detector = PitchDetector::new(PitchDetectorConfig::default()).unwrap();
        run(
            AnalysisIo {
                input_cons,
                pitch_prod,
                spectrum_prod,
                formant_prod,
                stats: Arc::clone(&stats),
            },
            detector,
            &config,
        );

        // 8192 samples at FFT size 1024 yield several spectra; only one fits.
        assert!(spectrum_cons.try_pop().is_some());
        assert!(stats.snapshot().analysis_results_dropped > 0);
    }

    #[test]
    fn analysis_config_construction() {
        let config = AnalysisConfig {
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
            spectrum_fft_size: 4096,
            spectrum_smoothing: 0.5,
            onset_fft_size: 2048,
            onset_threshold: 0.5,
            sample_rate: 48_000,
            buffer_size: 512,
        };

        let cloned = config;
        assert_eq!(cloned.spectrum_fft_size, 4096);
        assert_eq!(cloned.sample_rate, 48_000);
    }
}
