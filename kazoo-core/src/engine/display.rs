//! Display state snapshot for UI rendering.
//!
//! [`DisplayState`] is a self-contained, clonable snapshot of everything the
//! UI needs to render one frame. It is produced on the processing thread and
//! consumed by the TUI thread via a ring buffer.

use crate::analysis::{FormantData, PitchEstimate};
use crate::mixer::MixerSnapshot;
use crate::transport::TransportSnapshot;
use crate::{Db, TimePosition};

/// A complete snapshot of the engine state for a single UI frame.
///
/// Produced by the processing thread every audio block and pushed into a
/// display ring buffer. The UI thread pops the latest value and renders it.
/// Fields that allocate (`Vec`, `Option<FormantData>`) are pre-sized where
/// possible to minimise allocation churn, but this struct lives outside the
/// real-time path so occasional allocation is acceptable.
#[derive(Debug, Clone)]
pub struct DisplayState {
    /// Transport state (position, tempo, time signature, loop, metronome).
    pub transport: TransportSnapshot,

    /// Mixer meter readings (per-track and master levels).
    pub mixer: MixerSnapshot,

    /// Most recent pitch estimate from the analysis thread.
    pub pitch: PitchEstimate,

    /// Smoothed magnitude spectrum for the spectrum display (dB values).
    pub spectrum_magnitudes: Vec<f32>,

    /// Recent waveform samples for the oscilloscope display.
    pub waveform: Vec<f32>,

    /// Input signal level in dB (envelope follower output).
    pub input_level_db: f32,

    /// Whether disk recording is currently active.
    pub is_recording: bool,

    /// Most recent formant data from the analysis thread, if available.
    pub formants: Option<FormantData>,

    /// Estimated CPU load of the processing thread as a fraction in [0, 1].
    pub cpu_load: f32,
}

impl DisplayState {
    /// Create a default display state with all values at their neutral /
    /// silent positions.
    ///
    /// This is used as the initial state before the first real snapshot
    /// arrives from the processing thread.
    #[must_use]
    pub fn initial(sample_rate: u32) -> Self {
        Self {
            transport: TransportSnapshot {
                state: crate::transport::TransportState::Stopped,
                position: TimePosition::new(0, sample_rate),
                bpm: 120.0,
                beats_per_bar: 4,
                beat_unit: 4,
                loop_region: None,
                loop_enabled: false,
                metronome_enabled: false,
            },
            mixer: MixerSnapshot {
                track_meters: Vec::new(),
                master_peak_db: [Db::SILENCE.value(); 2],
                master_rms_db: [Db::SILENCE.value(); 2],
                master_clipping: false,
            },
            pitch: PitchEstimate {
                frequency: None,
                voiced_probability: 0.0,
                midi_note: None,
            },
            spectrum_magnitudes: Vec::new(),
            waveform: Vec::new(),
            input_level_db: Db::SILENCE.value(),
            is_recording: false,
            formants: None,
            cpu_load: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::TransportState;

    #[test]
    fn initial_display_state_has_sensible_defaults() {
        let state = DisplayState::initial(44_100);

        assert_eq!(state.transport.state, TransportState::Stopped);
        assert_eq!(state.transport.position.sample_rate, 44_100);
        assert_eq!(state.transport.position.samples, 0);
        assert!((state.transport.bpm - 120.0).abs() < f64::EPSILON);
        assert_eq!(state.transport.beats_per_bar, 4);
        assert_eq!(state.transport.beat_unit, 4);
        assert!(state.transport.loop_region.is_none());
        assert!(!state.transport.loop_enabled);
        assert!(!state.transport.metronome_enabled);
    }

    #[test]
    fn initial_mixer_is_silent() {
        let state = DisplayState::initial(48_000);

        assert!(state.mixer.track_meters.is_empty());
        assert!((state.mixer.master_peak_db[0] - Db::SILENCE.value()).abs() < f32::EPSILON);
        assert!((state.mixer.master_peak_db[1] - Db::SILENCE.value()).abs() < f32::EPSILON);
        assert!(!state.mixer.master_clipping);
    }

    #[test]
    fn initial_pitch_is_unvoiced() {
        let state = DisplayState::initial(44_100);

        assert!(state.pitch.frequency.is_none());
        assert!(state.pitch.voiced_probability.abs() < f32::EPSILON);
        assert!(state.pitch.midi_note.is_none());
    }

    #[test]
    fn initial_spectrum_and_waveform_empty() {
        let state = DisplayState::initial(44_100);

        assert!(state.spectrum_magnitudes.is_empty());
        assert!(state.waveform.is_empty());
    }

    #[test]
    fn initial_recording_is_off() {
        let state = DisplayState::initial(44_100);
        assert!(!state.is_recording);
    }

    #[test]
    fn initial_formants_are_none() {
        let state = DisplayState::initial(44_100);
        assert!(state.formants.is_none());
    }

    #[test]
    fn initial_cpu_load_is_zero() {
        let state = DisplayState::initial(44_100);
        assert!(state.cpu_load.abs() < f32::EPSILON);
    }

    #[test]
    fn initial_input_level_is_silence() {
        let state = DisplayState::initial(44_100);
        assert!((state.input_level_db - Db::SILENCE.value()).abs() < f32::EPSILON);
    }

    #[test]
    fn display_state_is_clone() {
        let state = DisplayState::initial(44_100);
        let cloned = state.clone();
        assert_eq!(cloned.transport.state, state.transport.state);
        assert!((cloned.cpu_load - state.cpu_load).abs() < f32::EPSILON);
    }

    #[test]
    fn display_state_debug_does_not_panic() {
        let state = DisplayState::initial(44_100);
        let dbg = format!("{state:?}");
        assert!(dbg.contains("DisplayState"));
    }
}
