//! Audio analysis: pitch detection, onset detection, FFT, formant extraction.
//!
//! This module provides six analysis components:
//!
//! - [`cycle::CycleTracker`] - Measures pitch at cycle marks with bounded
//!   autocorrelation searches on preallocated sample history.
//! - [`EnvelopeFollower`] - Tracks the amplitude contour of an audio signal
//!   using one-pole smoothing with separate attack and release times.
//! - [`PitchDetector`] - Detects fundamental frequency using the pYIN algorithm,
//!   returning frequency, voicing probability, and nearest MIDI note.
//! - [`SpectrumAnalyzer`] - Computes magnitude spectra via FFT with Hann
//!   windowing, dB conversion, and optional EMA smoothing.
//! - [`OnsetDetector`] - Finds note onsets using spectral flux (positive
//!   half-wave rectified magnitude differences between consecutive frames).
//! - [`FormantExtractor`] - Extracts vocal-tract resonance frequencies via
//!   LPC analysis with Levinson-Durbin recursion and polynomial root finding.

pub mod cycle;
mod envelope;
mod formant;
mod onset;
mod pitch;
mod pyin;
pub mod score;
mod spectrum;

pub use envelope::EnvelopeFollower;
pub use formant::{FormantData, FormantExtractor};
pub use onset::{OnsetDetector, OnsetEvent};
pub use pitch::{PitchDetector, PitchDetectorConfig, PitchEstimate};
pub use spectrum::{SpectrumAnalyzer, SpectrumData};

/// The lowest pitch the voice trackers look for: G1 is 49 Hz, the bottom of a low male hum.
pub const VOICE_MIN_HZ: f32 = 50.0;
/// The highest: a whistle tops out near B7 (3951 Hz).
pub const VOICE_MAX_HZ: f32 = 4000.0;
