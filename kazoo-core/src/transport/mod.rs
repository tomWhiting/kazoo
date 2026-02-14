//! Transport: play, stop, record, loop, timeline position, BPM.
//!
//! The [`TransportClock`] owns the authoritative timeline state and advances
//! sample-by-sample during audio callbacks. The UI reads a [`TransportSnapshot`]
//! each frame and sends [`TransportCommand`]s to mutate the clock.

use crate::TimePosition;

// ---------------------------------------------------------------------------
// BPM validation range
// ---------------------------------------------------------------------------

/// Minimum allowed tempo (beats per minute).
const MIN_BPM: f64 = 20.0;

/// Maximum allowed tempo (beats per minute).
const MAX_BPM: f64 = 300.0;

/// Default tempo.
const DEFAULT_BPM: f64 = 120.0;

// ---------------------------------------------------------------------------
// TransportState
// ---------------------------------------------------------------------------

/// The current playback state of the transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportState {
    /// The transport is stopped and the position is at zero.
    Stopped,
    /// Audio is actively playing back.
    Playing,
    /// Audio playback and recording are active.
    Recording,
    /// Playback is suspended; position is preserved.
    Paused,
}

// ---------------------------------------------------------------------------
// TransportCommand
// ---------------------------------------------------------------------------

/// Commands that can be sent to the transport to change its state or settings.
#[derive(Debug, Clone, Copy)]
pub enum TransportCommand {
    /// Begin playback from the current position.
    Play,
    /// Stop playback and reset the position to zero.
    Stop,
    /// Pause playback, preserving the current position.
    Pause,
    /// Begin recording (implies playback).
    Record,
    /// Seek to an absolute sample position.
    Seek(u64),
    /// Set the tempo in beats per minute. Values outside [`MIN_BPM`]..=[`MAX_BPM`]
    /// are clamped.
    SetTempo(f64),
    /// Set the time signature as (numerator, denominator). Both values must be
    /// at least 1; invalid values are silently ignored.
    SetTimeSignature(u8, u8),
    /// Set a loop region `(start_sample, end_sample)`, or `None` to disable
    /// looping. The region is validated: `start` must be strictly less than
    /// `end`.
    SetLoop(Option<(u64, u64)>),
    /// Toggle the metronome on/off.
    ToggleMetronome,
}

// ---------------------------------------------------------------------------
// TransportSnapshot
// ---------------------------------------------------------------------------

/// An immutable, cloneable snapshot of the transport state at a single instant.
///
/// Created by [`TransportClock::snapshot`] and intended for UI consumption.
#[derive(Debug, Clone)]
pub struct TransportSnapshot {
    /// Current playback state.
    pub state: TransportState,
    /// Current timeline position.
    pub position: TimePosition,
    /// Tempo in beats per minute.
    pub bpm: f64,
    /// Number of beats per bar (time-signature numerator).
    pub beats_per_bar: u8,
    /// Beat unit (time-signature denominator, e.g. 4 = quarter note).
    pub beat_unit: u8,
    /// Active loop region in samples, if any.
    pub loop_region: Option<(u64, u64)>,
    /// Whether looping is enabled.
    pub loop_enabled: bool,
    /// Whether the metronome click is enabled.
    pub metronome_enabled: bool,
}

// ---------------------------------------------------------------------------
// TransportClock
// ---------------------------------------------------------------------------

/// The authoritative transport clock that lives on the audio thread.
///
/// Maintains timeline position, tempo, time signature, loop state, and
/// metronome state. Designed for real-time use: every method is O(1) with
/// no allocation, locking, or I/O.
#[derive(Debug)]
pub struct TransportClock {
    state: TransportState,
    position_samples: u64,
    sample_rate: u32,
    bpm: f64,
    beats_per_bar: u8,
    beat_unit: u8,
    loop_region: Option<(u64, u64)>,
    loop_enabled: bool,
    metronome_enabled: bool,
}

impl TransportClock {
    /// Create a new transport clock at sample position 0 with default settings.
    ///
    /// - Tempo: 120 BPM
    /// - Time signature: 4/4
    /// - Loop: disabled
    /// - Metronome: disabled
    /// - State: Stopped
    ///
    /// A `sample_rate` of 0 is treated as 1 to avoid division-by-zero.
    #[must_use]
    pub const fn new(sample_rate: u32) -> Self {
        Self {
            state: TransportState::Stopped,
            position_samples: 0,
            sample_rate: if sample_rate == 0 { 1 } else { sample_rate },
            bpm: DEFAULT_BPM,
            beats_per_bar: 4,
            beat_unit: 4,
            loop_region: None,
            loop_enabled: false,
            metronome_enabled: false,
        }
    }

    /// Apply a command to mutate the transport state.
    ///
    /// Invalid state transitions (e.g. `Pause` while `Stopped`) are silently
    /// ignored, making this safe to call from untrusted UI input.
    pub const fn apply_command(&mut self, cmd: TransportCommand) {
        match cmd {
            TransportCommand::Play => self.handle_play(),
            TransportCommand::Stop => self.handle_stop(),
            TransportCommand::Pause => self.handle_pause(),
            TransportCommand::Record => self.handle_record(),
            TransportCommand::Seek(pos) => self.position_samples = pos,
            TransportCommand::SetTempo(bpm) => self.set_tempo(bpm),
            TransportCommand::SetTimeSignature(num, den) => {
                self.set_time_signature(num, den);
            }
            TransportCommand::SetLoop(region) => self.set_loop(region),
            TransportCommand::ToggleMetronome => {
                self.metronome_enabled = !self.metronome_enabled;
            }
        }
    }

    /// Advance the timeline position by `num_samples`.
    ///
    /// Only advances when the transport is [`Playing`](TransportState::Playing)
    /// or [`Recording`](TransportState::Recording). When a loop region is
    /// active and enabled, the position wraps around the loop boundaries.
    pub fn advance(&mut self, num_samples: u32) {
        if !self.is_playing() && !self.is_recording() {
            return;
        }

        self.position_samples = self.position_samples.saturating_add(u64::from(num_samples));

        // Handle loop wrapping when looping is enabled and we have a valid region.
        if self.loop_enabled {
            if let Some((start, end)) = self.loop_region {
                // Only wrap if we have actually reached or passed the end.
                if self.position_samples >= end {
                    // Calculate how far past the end we are, then wrap within
                    // the loop region. This handles overshoots of any size.
                    let loop_len = end - start;
                    if loop_len > 0 {
                        let overshoot = self.position_samples - end;
                        self.position_samples = start + (overshoot % loop_len);
                    } else {
                        // Degenerate loop region (length 0) -- just sit at start.
                        self.position_samples = start;
                    }
                }
            }
        }
    }

    /// Create an immutable snapshot of the current transport state.
    ///
    /// Intended for reading on the UI thread without blocking the audio thread.
    #[must_use]
    pub fn snapshot(&self) -> TransportSnapshot {
        TransportSnapshot {
            state: self.state,
            position: TimePosition::new(self.position_samples, self.sample_rate),
            bpm: self.bpm,
            beats_per_bar: self.beats_per_bar,
            beat_unit: self.beat_unit,
            loop_region: self.loop_region,
            loop_enabled: self.loop_enabled,
            metronome_enabled: self.metronome_enabled,
        }
    }

    /// Current timeline position in samples.
    #[must_use]
    pub const fn position_samples(&self) -> u64 {
        self.position_samples
    }

    /// Returns `true` if the transport is currently playing (not paused, not
    /// stopped, not recording).
    #[must_use]
    pub const fn is_playing(&self) -> bool {
        matches!(self.state, TransportState::Playing)
    }

    /// Returns `true` if the transport is currently recording.
    #[must_use]
    pub const fn is_recording(&self) -> bool {
        matches!(self.state, TransportState::Recording)
    }

    // -- Private helpers ---------------------------------------------------

    /// Transition to Playing from valid prior states.
    const fn handle_play(&mut self) {
        match self.state {
            TransportState::Stopped | TransportState::Paused => {
                self.state = TransportState::Playing;
            }
            TransportState::Playing | TransportState::Recording => {
                // Already playing or recording -- no-op.
            }
        }
    }

    /// Stop always works from any state and resets position to 0.
    const fn handle_stop(&mut self) {
        self.state = TransportState::Stopped;
        self.position_samples = 0;
    }

    /// Pause is valid only from Playing or Recording.
    const fn handle_pause(&mut self) {
        match self.state {
            TransportState::Playing | TransportState::Recording => {
                self.state = TransportState::Paused;
            }
            TransportState::Stopped | TransportState::Paused => {
                // Cannot pause if already stopped or paused -- no-op.
            }
        }
    }

    /// Record is valid only from Stopped (start recording from the beginning).
    const fn handle_record(&mut self) {
        match self.state {
            TransportState::Stopped => {
                self.state = TransportState::Recording;
            }
            TransportState::Playing | TransportState::Recording | TransportState::Paused => {
                // No-op for invalid transitions.
            }
        }
    }

    /// Set tempo, clamping to the valid range and rejecting non-finite values.
    const fn set_tempo(&mut self, bpm: f64) {
        if !bpm.is_finite() {
            return;
        }
        self.bpm = bpm.clamp(MIN_BPM, MAX_BPM);
    }

    /// Set time signature, rejecting zero values for either component.
    const fn set_time_signature(&mut self, numerator: u8, denominator: u8) {
        if numerator == 0 || denominator == 0 {
            return;
        }
        self.beats_per_bar = numerator;
        self.beat_unit = denominator;
    }

    /// Set or clear the loop region. Validates that start < end.
    const fn set_loop(&mut self, region: Option<(u64, u64)>) {
        match region {
            Some((start, end)) if start < end => {
                self.loop_region = Some((start, end));
                self.loop_enabled = true;
            }
            Some(_) => {
                // Invalid region (start >= end) -- silently ignore.
            }
            None => {
                self.loop_region = None;
                self.loop_enabled = false;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Construction -------------------------------------------------------

    #[test]
    fn new_transport_has_default_state() {
        let clock = TransportClock::new(44_100);
        assert_eq!(clock.state, TransportState::Stopped);
        assert_eq!(clock.position_samples, 0);
        assert_eq!(clock.sample_rate, 44_100);
        assert!((clock.bpm - 120.0).abs() < f64::EPSILON);
        assert_eq!(clock.beats_per_bar, 4);
        assert_eq!(clock.beat_unit, 4);
        assert!(clock.loop_region.is_none());
        assert!(!clock.loop_enabled);
        assert!(!clock.metronome_enabled);
    }

    #[test]
    fn new_transport_zero_sample_rate_becomes_one() {
        let clock = TransportClock::new(0);
        assert_eq!(clock.sample_rate, 1);
    }

    // -- Advance and position -----------------------------------------------

    #[test]
    fn advance_while_stopped_does_not_move() {
        let mut clock = TransportClock::new(44_100);
        clock.advance(256);
        assert_eq!(clock.position_samples, 0);
    }

    #[test]
    fn advance_while_playing_moves_position() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::Play);
        clock.advance(256);
        assert_eq!(clock.position_samples, 256);
    }

    #[test]
    fn advance_while_recording_moves_position() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::Record);
        clock.advance(512);
        assert_eq!(clock.position_samples, 512);
    }

    #[test]
    fn advance_while_paused_does_not_move() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::Play);
        clock.advance(100);
        clock.apply_command(TransportCommand::Pause);
        clock.advance(100);
        assert_eq!(clock.position_samples, 100);
    }

    #[test]
    fn advance_accumulates_position() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::Play);
        clock.advance(100);
        clock.advance(200);
        clock.advance(300);
        assert_eq!(clock.position_samples, 600);
    }

    // -- Loop wrapping ------------------------------------------------------

    #[test]
    fn loop_wraps_at_boundary() {
        let mut clock = TransportClock::new(44_100);
        // Loop from sample 100 to 200 (length 100).
        clock.apply_command(TransportCommand::SetLoop(Some((100, 200))));
        clock.apply_command(TransportCommand::Play);

        // Position to just before the loop end.
        clock.apply_command(TransportCommand::Seek(190));
        // Advance by 20 samples: 190 + 20 = 210, overshoot by 10 -> wraps to 110.
        clock.advance(20);
        assert_eq!(clock.position_samples, 110);
    }

    #[test]
    fn loop_wraps_exactly_at_end() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::SetLoop(Some((100, 200))));
        clock.apply_command(TransportCommand::Play);

        clock.apply_command(TransportCommand::Seek(190));
        // Advance exactly to the loop end: 190 + 10 = 200.
        // 200 >= 200, overshoot = 0, wraps to start (100).
        clock.advance(10);
        assert_eq!(clock.position_samples, 100);
    }

    #[test]
    fn loop_wraps_large_overshoot() {
        let mut clock = TransportClock::new(44_100);
        // Loop region of length 100 (samples 0..100).
        clock.apply_command(TransportCommand::SetLoop(Some((0, 100))));
        clock.apply_command(TransportCommand::Play);

        // Advance by 350 samples: position = 350, overshoot = 250, 250 % 100 = 50.
        clock.advance(350);
        assert_eq!(clock.position_samples, 50);
    }

    #[test]
    fn loop_does_not_wrap_before_end() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::SetLoop(Some((100, 200))));
        clock.apply_command(TransportCommand::Play);

        // Start before the loop region and advance within it.
        clock.advance(150);
        assert_eq!(clock.position_samples, 150);
    }

    #[test]
    fn loop_disabled_does_not_wrap() {
        let mut clock = TransportClock::new(44_100);
        // Set loop then disable it.
        clock.apply_command(TransportCommand::SetLoop(Some((100, 200))));
        clock.apply_command(TransportCommand::SetLoop(None));
        clock.apply_command(TransportCommand::Play);

        clock.apply_command(TransportCommand::Seek(190));
        clock.advance(20);
        // Without loop, position should be 210.
        assert_eq!(clock.position_samples, 210);
    }

    // -- State transitions --------------------------------------------------

    #[test]
    fn stop_to_play() {
        let mut clock = TransportClock::new(44_100);
        assert_eq!(clock.state, TransportState::Stopped);
        clock.apply_command(TransportCommand::Play);
        assert_eq!(clock.state, TransportState::Playing);
    }

    #[test]
    fn play_to_pause_to_play() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::Play);
        assert_eq!(clock.state, TransportState::Playing);

        clock.apply_command(TransportCommand::Pause);
        assert_eq!(clock.state, TransportState::Paused);

        clock.apply_command(TransportCommand::Play);
        assert_eq!(clock.state, TransportState::Playing);
    }

    #[test]
    fn play_to_stop_resets_position() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::Play);
        clock.advance(1000);
        assert_eq!(clock.position_samples, 1000);

        clock.apply_command(TransportCommand::Stop);
        assert_eq!(clock.state, TransportState::Stopped);
        assert_eq!(clock.position_samples, 0);
    }

    #[test]
    fn stop_to_record() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::Record);
        assert_eq!(clock.state, TransportState::Recording);
    }

    #[test]
    fn recording_to_stop() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::Record);
        clock.advance(500);
        clock.apply_command(TransportCommand::Stop);
        assert_eq!(clock.state, TransportState::Stopped);
        assert_eq!(clock.position_samples, 0);
    }

    #[test]
    fn recording_to_pause() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::Record);
        clock.advance(500);
        clock.apply_command(TransportCommand::Pause);
        assert_eq!(clock.state, TransportState::Paused);
        assert_eq!(clock.position_samples, 500);
    }

    #[test]
    fn full_lifecycle_stop_play_pause_play_stop() {
        let mut clock = TransportClock::new(44_100);
        assert_eq!(clock.state, TransportState::Stopped);

        clock.apply_command(TransportCommand::Play);
        assert_eq!(clock.state, TransportState::Playing);
        clock.advance(100);

        clock.apply_command(TransportCommand::Pause);
        assert_eq!(clock.state, TransportState::Paused);
        assert_eq!(clock.position_samples, 100);

        clock.apply_command(TransportCommand::Play);
        assert_eq!(clock.state, TransportState::Playing);
        clock.advance(100);
        assert_eq!(clock.position_samples, 200);

        clock.apply_command(TransportCommand::Stop);
        assert_eq!(clock.state, TransportState::Stopped);
        assert_eq!(clock.position_samples, 0);
    }

    // -- Invalid state transitions (should be no-ops) -----------------------

    #[test]
    fn pause_while_stopped_is_noop() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::Pause);
        assert_eq!(clock.state, TransportState::Stopped);
    }

    #[test]
    fn play_while_playing_is_noop() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::Play);
        clock.advance(100);
        clock.apply_command(TransportCommand::Play);
        assert_eq!(clock.state, TransportState::Playing);
        assert_eq!(clock.position_samples, 100);
    }

    #[test]
    fn record_while_playing_is_noop() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::Play);
        clock.apply_command(TransportCommand::Record);
        assert_eq!(clock.state, TransportState::Playing);
    }

    #[test]
    fn record_while_paused_is_noop() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::Play);
        clock.apply_command(TransportCommand::Pause);
        clock.apply_command(TransportCommand::Record);
        assert_eq!(clock.state, TransportState::Paused);
    }

    #[test]
    fn record_while_recording_is_noop() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::Record);
        clock.apply_command(TransportCommand::Record);
        assert_eq!(clock.state, TransportState::Recording);
    }

    #[test]
    fn pause_while_paused_is_noop() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::Play);
        clock.apply_command(TransportCommand::Pause);
        clock.advance(100); // should not advance
        clock.apply_command(TransportCommand::Pause);
        assert_eq!(clock.state, TransportState::Paused);
    }

    // -- Seek ---------------------------------------------------------------

    #[test]
    fn seek_to_arbitrary_position() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::Seek(88_200));
        assert_eq!(clock.position_samples, 88_200);
    }

    #[test]
    fn seek_while_playing_preserves_state() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::Play);
        clock.apply_command(TransportCommand::Seek(44_100));
        assert_eq!(clock.state, TransportState::Playing);
        assert_eq!(clock.position_samples, 44_100);
    }

    #[test]
    fn seek_to_zero() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::Play);
        clock.advance(1000);
        clock.apply_command(TransportCommand::Seek(0));
        assert_eq!(clock.position_samples, 0);
    }

    // -- Tempo changes ------------------------------------------------------

    #[test]
    fn set_tempo_valid() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::SetTempo(140.0));
        assert!((clock.bpm - 140.0).abs() < f64::EPSILON);
    }

    #[test]
    fn set_tempo_clamps_low() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::SetTempo(5.0));
        assert!((clock.bpm - MIN_BPM).abs() < f64::EPSILON);
    }

    #[test]
    fn set_tempo_clamps_high() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::SetTempo(500.0));
        assert!((clock.bpm - MAX_BPM).abs() < f64::EPSILON);
    }

    #[test]
    fn set_tempo_rejects_nan() {
        let mut clock = TransportClock::new(44_100);
        let original = clock.bpm;
        clock.apply_command(TransportCommand::SetTempo(f64::NAN));
        assert!((clock.bpm - original).abs() < f64::EPSILON);
    }

    #[test]
    fn set_tempo_rejects_infinity() {
        let mut clock = TransportClock::new(44_100);
        let original = clock.bpm;
        clock.apply_command(TransportCommand::SetTempo(f64::INFINITY));
        assert!((clock.bpm - original).abs() < f64::EPSILON);
    }

    #[test]
    fn set_tempo_rejects_neg_infinity() {
        let mut clock = TransportClock::new(44_100);
        let original = clock.bpm;
        clock.apply_command(TransportCommand::SetTempo(f64::NEG_INFINITY));
        assert!((clock.bpm - original).abs() < f64::EPSILON);
    }

    // -- Time signature changes ---------------------------------------------

    #[test]
    fn set_time_signature_valid() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::SetTimeSignature(3, 8));
        assert_eq!(clock.beats_per_bar, 3);
        assert_eq!(clock.beat_unit, 8);
    }

    #[test]
    fn set_time_signature_zero_numerator_ignored() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::SetTimeSignature(0, 4));
        // Should remain at default 4/4.
        assert_eq!(clock.beats_per_bar, 4);
        assert_eq!(clock.beat_unit, 4);
    }

    #[test]
    fn set_time_signature_zero_denominator_ignored() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::SetTimeSignature(3, 0));
        assert_eq!(clock.beats_per_bar, 4);
        assert_eq!(clock.beat_unit, 4);
    }

    // -- Loop region validation ---------------------------------------------

    #[test]
    fn set_loop_valid_region() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::SetLoop(Some((1000, 5000))));
        assert_eq!(clock.loop_region, Some((1000, 5000)));
        assert!(clock.loop_enabled);
    }

    #[test]
    fn set_loop_none_disables() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::SetLoop(Some((1000, 5000))));
        clock.apply_command(TransportCommand::SetLoop(None));
        assert!(clock.loop_region.is_none());
        assert!(!clock.loop_enabled);
    }

    #[test]
    fn set_loop_start_equal_end_rejected() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::SetLoop(Some((100, 100))));
        assert!(clock.loop_region.is_none());
        assert!(!clock.loop_enabled);
    }

    #[test]
    fn set_loop_start_greater_than_end_rejected() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::SetLoop(Some((200, 100))));
        assert!(clock.loop_region.is_none());
        assert!(!clock.loop_enabled);
    }

    #[test]
    fn set_loop_invalid_preserves_previous() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::SetLoop(Some((100, 200))));
        // Try to set an invalid loop -- should keep the existing one.
        clock.apply_command(TransportCommand::SetLoop(Some((500, 400))));
        assert_eq!(clock.loop_region, Some((100, 200)));
        assert!(clock.loop_enabled);
    }

    // -- Metronome toggle ---------------------------------------------------

    #[test]
    fn toggle_metronome() {
        let mut clock = TransportClock::new(44_100);
        assert!(!clock.metronome_enabled);
        clock.apply_command(TransportCommand::ToggleMetronome);
        assert!(clock.metronome_enabled);
        clock.apply_command(TransportCommand::ToggleMetronome);
        assert!(!clock.metronome_enabled);
    }

    // -- Snapshot -----------------------------------------------------------

    #[test]
    fn snapshot_reflects_state() {
        let mut clock = TransportClock::new(48_000);
        clock.apply_command(TransportCommand::SetTempo(90.0));
        clock.apply_command(TransportCommand::SetTimeSignature(6, 8));
        clock.apply_command(TransportCommand::SetLoop(Some((1000, 2000))));
        clock.apply_command(TransportCommand::ToggleMetronome);
        clock.apply_command(TransportCommand::Play);
        clock.advance(500);

        let snap = clock.snapshot();
        assert_eq!(snap.state, TransportState::Playing);
        assert_eq!(snap.position.samples, 500);
        assert_eq!(snap.position.sample_rate, 48_000);
        assert!((snap.bpm - 90.0).abs() < f64::EPSILON);
        assert_eq!(snap.beats_per_bar, 6);
        assert_eq!(snap.beat_unit, 8);
        assert_eq!(snap.loop_region, Some((1000, 2000)));
        assert!(snap.loop_enabled);
        assert!(snap.metronome_enabled);
    }

    // -- is_playing / is_recording ------------------------------------------

    #[test]
    fn is_playing_correct() {
        let mut clock = TransportClock::new(44_100);
        assert!(!clock.is_playing());
        clock.apply_command(TransportCommand::Play);
        assert!(clock.is_playing());
        clock.apply_command(TransportCommand::Pause);
        assert!(!clock.is_playing());
        clock.apply_command(TransportCommand::Play);
        assert!(clock.is_playing());
        clock.apply_command(TransportCommand::Stop);
        assert!(!clock.is_playing());
    }

    #[test]
    fn is_recording_correct() {
        let mut clock = TransportClock::new(44_100);
        assert!(!clock.is_recording());
        clock.apply_command(TransportCommand::Record);
        assert!(clock.is_recording());
        clock.apply_command(TransportCommand::Stop);
        assert!(!clock.is_recording());
    }

    #[test]
    fn is_playing_false_during_recording() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::Record);
        assert!(!clock.is_playing());
        assert!(clock.is_recording());
    }

    // -- Edge cases ---------------------------------------------------------

    #[test]
    fn advance_zero_samples_is_noop() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::Play);
        clock.advance(100);
        clock.advance(0);
        assert_eq!(clock.position_samples, 100);
    }

    #[test]
    fn stop_from_any_state_resets() {
        let mut clock = TransportClock::new(44_100);

        // From Playing
        clock.apply_command(TransportCommand::Play);
        clock.advance(100);
        clock.apply_command(TransportCommand::Stop);
        assert_eq!(clock.state, TransportState::Stopped);
        assert_eq!(clock.position_samples, 0);

        // From Paused
        clock.apply_command(TransportCommand::Play);
        clock.advance(100);
        clock.apply_command(TransportCommand::Pause);
        clock.apply_command(TransportCommand::Stop);
        assert_eq!(clock.state, TransportState::Stopped);
        assert_eq!(clock.position_samples, 0);

        // From Recording
        clock.apply_command(TransportCommand::Record);
        clock.advance(100);
        clock.apply_command(TransportCommand::Stop);
        assert_eq!(clock.state, TransportState::Stopped);
        assert_eq!(clock.position_samples, 0);

        // From Stopped (already stopped)
        clock.apply_command(TransportCommand::Stop);
        assert_eq!(clock.state, TransportState::Stopped);
        assert_eq!(clock.position_samples, 0);
    }

    #[test]
    fn snapshot_position_uses_correct_sample_rate() {
        let clock = TransportClock::new(48_000);
        let snap = clock.snapshot();
        assert_eq!(snap.position.sample_rate, 48_000);
    }

    #[test]
    fn loop_wrap_with_position_before_loop_start() {
        // If the current position is before the loop region, advancing past
        // the loop end should still wrap correctly.
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::SetLoop(Some((1000, 2000))));
        clock.apply_command(TransportCommand::Play);

        // Start at 0, advance by 2500: position = 2500.
        // 2500 >= 2000, overshoot = 500, 500 % 1000 = 500, wrap to 1500.
        clock.advance(2500);
        assert_eq!(clock.position_samples, 1500);
    }

    #[test]
    fn multiple_loop_wraps_in_single_advance() {
        // Advance far enough to wrap multiple times around the loop.
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::SetLoop(Some((0, 100))));
        clock.apply_command(TransportCommand::Play);

        // Advance 1050: 1050 >= 100, overshoot = 950, 950 % 100 = 50.
        clock.advance(1050);
        assert_eq!(clock.position_samples, 50);
    }
}
