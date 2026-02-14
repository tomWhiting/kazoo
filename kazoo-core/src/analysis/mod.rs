//! Audio analysis: pitch detection, onset detection, FFT, formant extraction.
//!
//! This module provides five analysis components:
//!
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

mod envelope;
mod formant;
mod onset;
mod pitch;
mod spectrum;

pub use envelope::EnvelopeFollower;
pub use formant::{FormantData, FormantExtractor};
pub use onset::{OnsetDetector, OnsetEvent};
pub use pitch::{PitchDetector, PitchDetectorConfig, PitchEstimate};
pub use spectrum::{SpectrumAnalyzer, SpectrumData};
