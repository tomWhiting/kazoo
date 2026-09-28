//! Display state snapshot for UI rendering.
//!
//! [`DisplayState`] is a self-contained, clonable snapshot of everything the
//! UI needs to render one frame. The output callback writes it and the UI
//! reads it through [`super::EngineHandle::poll_display`]; the timeline part
//! is built off the audio thread and merged in by the handle.

use crate::analysis::{FormantData, PitchEstimate};
use crate::mixer::MixerSnapshot;
use crate::transport::TransportSnapshot;
use crate::{Db, MAX_TRACKS, TimePosition};

/// Most waveform points in a snapshot's oscilloscope trace.
pub const WAVEFORM_POINTS: usize = 256;

/// Most formants a snapshot carries (the analysis thread's LPC order bounds
/// the real number far below this).
pub const FORMANT_CAPACITY: usize = 32;

// ---------------------------------------------------------------------------
// Timeline snapshot types
// ---------------------------------------------------------------------------

/// Display data for a single clip on the timeline.
#[derive(Debug, Clone)]
pub struct ClipSnapshot {
    /// Unique clip identifier.
    pub id: u64,
    /// Display name.
    pub name: String,
    /// Start position on the timeline in samples.
    pub position: u64,
    /// Length in samples.
    pub length: u64,
    /// Gain in dB.
    pub gain_db: f32,
    /// Whether the clip is muted.
    pub muted: bool,
    /// Pre-computed waveform overview: up to 128 (min, max) pairs for rendering.
    pub waveform_overview: Vec<(f32, f32)>,
}

/// The part of the timeline a recording in progress has covered so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordingSpan {
    /// Timeline position (in samples) where the recording started.
    pub start: u64,
    /// Samples recorded so far.
    pub length: u64,
}

impl RecordingSpan {
    /// Timeline position just past the last recorded sample.
    #[must_use]
    pub const fn end(&self) -> u64 {
        self.start.saturating_add(self.length)
    }
}

/// Display data for one track's clips on the timeline.
///
/// Arm, mute and solo are not repeated here: they belong to the track's
/// mixer state, which the UI already owns and edits.
#[derive(Debug, Clone)]
pub struct TrackClipSnapshot {
    /// Track identifier (index).
    pub track_id: usize,
    /// Track name.
    pub track_name: String,
    /// All clips on this track, sorted by position.
    pub clips: Vec<ClipSnapshot>,
    /// The recording in progress on this track, if one is.
    pub recording: Option<RecordingSpan>,
}

/// Timeline snapshot for the TUI to render.
#[derive(Debug, Clone)]
pub struct TimelineSnapshot {
    /// Per-track clip data.
    pub tracks: Vec<TrackClipSnapshot>,
    /// Total timeline length in samples (end of the last clip or recording).
    pub total_length: u64,
}

impl TimelineSnapshot {
    /// Create an empty timeline snapshot.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            tracks: Vec::new(),
            total_length: 0,
        }
    }
}

/// A recording in progress on one track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackRecording {
    /// The track being recorded (`TrackId.0`).
    pub track_id: usize,
    /// The part of the timeline recorded so far.
    pub span: RecordingSpan,
}

// ---------------------------------------------------------------------------
// Display state
// ---------------------------------------------------------------------------

/// A complete snapshot of the engine state for a single UI frame.
///
/// The output callback writes snapshots at the UI's frame rate into frames
/// that circulate between it and the [`super::EngineHandle`]: each frame's
/// buffers are sized when the engine starts
/// ([`DisplayState::with_capacity`]) and only ever overwritten within that
/// capacity, so the callback never allocates or frees one. The handle keeps
/// the newest frame, fills in the [`TimelineSnapshot`] (built off the audio
/// thread) and the recording spans on it, and hands older frames back.
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

    /// Most recent formant data from the analysis thread
    /// (`num_formants == 0` until formants have been detected).
    pub formants: FormantData,

    /// Estimated CPU load of the output callback as a fraction in [0, 1].
    pub cpu_load: f32,

    /// Timeline snapshot for clip display, with each track's recording in
    /// progress (from `recordings`) filled in.
    pub timeline: TimelineSnapshot,

    /// Recordings in progress, one per recording track, in no particular
    /// order; unused slots are `None`.
    pub recordings: [Option<TrackRecording>; MAX_TRACKS],
}

impl DisplayState {
    /// Create a default display state with all values at their neutral /
    /// silent positions.
    ///
    /// This is used as the initial state before the first real snapshot
    /// arrives from the output callback.
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
                metronome_enabled: false,
                beat: crate::transport::BeatIndicator {
                    beat: 0,
                    flash: false,
                },
                count_in: None,
                recording_workflow: crate::transport::RecordingWorkflow::FreeRecord,
                auto_record_bars: 0,
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
            formants: FormantData {
                frequencies: Vec::new(),
                bandwidths: Vec::new(),
                num_formants: 0,
            },
            cpu_load: 0.0,
            timeline: TimelineSnapshot::empty(),
            recordings: [None; MAX_TRACKS],
        }
    }

    /// Like [`DisplayState::initial`], with every buffer the output callback
    /// writes sized in advance: meters for [`MAX_TRACKS`] tracks,
    /// `spectrum_len` spectrum bins, [`WAVEFORM_POINTS`] waveform points and
    /// [`FORMANT_CAPACITY`] formants. Allocates.
    #[must_use]
    pub fn with_capacity(sample_rate: u32, spectrum_len: usize) -> Self {
        let mut state = Self::initial(sample_rate);
        state.mixer.track_meters.reserve_exact(MAX_TRACKS);
        state.spectrum_magnitudes.reserve_exact(spectrum_len);
        state.waveform.reserve_exact(WAVEFORM_POINTS);
        state.formants.frequencies.reserve_exact(FORMANT_CAPACITY);
        state.formants.bandwidths.reserve_exact(FORMANT_CAPACITY);
        state
    }

    /// The recording in progress on track `track_id` (`TrackId.0`), if any.
    #[must_use]
    pub fn recording_on(&self, track_id: usize) -> Option<RecordingSpan> {
        self.recordings
            .iter()
            .flatten()
            .find(|recording| recording.track_id == track_id)
            .map(|recording| recording.span)
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
        assert!(!state.transport.is_looping());
        assert_eq!(state.transport.count_in, None);
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
    fn initial_formants_are_empty() {
        let state = DisplayState::initial(44_100);
        assert_eq!(state.formants.num_formants, 0);
        assert!(state.formants.frequencies.is_empty());
    }

    #[test]
    fn with_capacity_sizes_every_buffer_the_callback_writes() {
        let state = DisplayState::with_capacity(48_000, 1025);
        assert!(state.mixer.track_meters.capacity() >= MAX_TRACKS);
        assert!(state.spectrum_magnitudes.capacity() >= 1025);
        assert!(state.waveform.capacity() >= WAVEFORM_POINTS);
        assert!(state.formants.frequencies.capacity() >= FORMANT_CAPACITY);
        assert!(state.formants.bandwidths.capacity() >= FORMANT_CAPACITY);
        assert!(state.recordings.iter().all(Option::is_none));
    }

    #[test]
    fn recording_on_finds_the_track() {
        let mut state = DisplayState::initial(44_100);
        let span = RecordingSpan {
            start: 10,
            length: 20,
        };
        state.recordings[3] = Some(TrackRecording { track_id: 7, span });
        assert_eq!(state.recording_on(7), Some(span));
        assert_eq!(state.recording_on(3), None);
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

    // -- Timeline snapshot tests --

    #[test]
    fn timeline_snapshot_empty() {
        let timeline = TimelineSnapshot::empty();
        assert!(timeline.tracks.is_empty());
        assert_eq!(timeline.total_length, 0);
    }

    #[test]
    fn clip_snapshot_debug() {
        let clip = ClipSnapshot {
            id: 1,
            name: "Test".into(),
            position: 0,
            length: 100,
            gain_db: 0.0,
            muted: false,
            waveform_overview: vec![(0.0, 0.5)],
        };
        let dbg = format!("{clip:?}");
        assert!(dbg.contains("ClipSnapshot"));
    }

    #[test]
    fn track_clip_snapshot_debug() {
        let snap = TrackClipSnapshot {
            track_id: 0,
            track_name: "Track 1".into(),
            clips: Vec::new(),
            recording: Some(RecordingSpan {
                start: u64::MAX - 1,
                length: 5,
            }),
        };
        assert_eq!(snap.recording.map(|span| span.end()), Some(u64::MAX));
        let dbg = format!("{snap:?}");
        assert!(dbg.contains("TrackClipSnapshot"));
    }

    #[test]
    fn timeline_snapshot_debug() {
        let timeline = TimelineSnapshot::empty();
        let dbg = format!("{timeline:?}");
        assert!(dbg.contains("TimelineSnapshot"));
    }

    #[test]
    fn display_state_initial_has_empty_timeline() {
        let state = DisplayState::initial(44_100);
        assert!(state.timeline.tracks.is_empty());
        assert_eq!(state.timeline.total_length, 0);
    }
}
