//! Transport: play, stop, record, loop, timeline position, BPM.
//!
//! The [`TransportClock`] owns the authoritative timeline state and advances
//! sample-by-sample during audio callbacks. The UI reads a [`TransportSnapshot`]
//! each frame and sends [`TransportCommand`]s to mutate the clock.

pub mod metronome;

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
// Recording workflow
// ---------------------------------------------------------------------------

/// Recording workflow mode configured by the user.
///
/// Determines what happens when the user initiates a workflow-aware recording
/// via [`TransportCommand::RecordWithCountIn`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingWorkflow {
    /// Record immediately with no count-in. On stop, quantize recording
    /// boundaries to the nearest bar boundary.
    FreeRecord,
    /// Count-in for `count_in_bars` bars, then record. If `record_bars` is
    /// greater than 0, auto-stop after that many bars; otherwise record
    /// until manual stop.
    CountIn {
        /// Number of bars to count in before recording starts.
        count_in_bars: u8,
        /// Number of bars to record (0 = unlimited, manual stop).
        record_bars: u8,
    },
    /// Record exactly `bars` bars starting immediately, then auto-stop.
    FixedLength {
        /// Number of bars to record before auto-stopping.
        bars: u8,
    },
}

// ---------------------------------------------------------------------------
// Count-in state machine (internal)
// ---------------------------------------------------------------------------

/// Internal state machine for count-in and auto-stop recording workflows.
///
/// Lives on [`TransportClock`] and is inspected by [`advance`] each audio
/// block to detect when count-in completes or auto-stop should trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CountInState {
    /// No count-in or auto-stop active (normal operation).
    Inactive,
    /// Counting in before recording. The transport is Playing (so the
    /// metronome sounds) and will transition to Recording when the position
    /// reaches `count_in_end`.
    CountingIn {
        /// Sample position at which the count-in ends and recording begins.
        count_in_end: u64,
        /// Sample position at which recording auto-stops (0 = no auto-stop).
        auto_stop: u64,
        /// Total count-in bars (for UI display).
        count_in_bars: u8,
    },
    /// Recording is active with an optional auto-stop boundary.
    AutoRecording {
        /// Sample position where recording started.
        record_start: u64,
        /// Sample position at which recording auto-stops (0 = no auto-stop).
        auto_stop: u64,
    },
}

// ---------------------------------------------------------------------------
// Advance flags
// ---------------------------------------------------------------------------

/// Flags returned by [`TransportClock::advance`] to signal state transitions
/// that the output callback must act on.
///
/// The output callback checks these flags each audio block and starts or
/// stops per-track recordings accordingly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdvanceFlags {
    /// The count-in period has completed; the output callback should start
    /// per-track recordings and transition the transport to Recording.
    pub count_in_completed: bool,
    /// The auto-stop boundary has been reached; the output callback should
    /// finalize per-track recordings and stop the transport.
    pub auto_stop_triggered: bool,
    /// The exact sample position where recording should start (the bar
    /// boundary at which the count-in ended). Only meaningful when
    /// `count_in_completed` is `true`. The output callback should use this
    /// as the clip start position rather than reading the current (overshot)
    /// transport position.
    pub record_start_position: u64,
}

impl AdvanceFlags {
    /// No events occurred.
    const NONE: Self = Self {
        count_in_completed: false,
        auto_stop_triggered: false,
        record_start_position: 0,
    };
}

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
    /// Set the metronome volume.
    SetMetronomeVolume(crate::Db),
    /// Set the recording workflow mode.
    SetRecordingWorkflow(RecordingWorkflow),
    /// Initiate a workflow-aware recording (count-in, fixed-length, etc.).
    ///
    /// This variant is intercepted by the output callback, which sets up the
    /// count-in state machine and starts/stops track recordings based on the
    /// configured [`RecordingWorkflow`].
    RecordWithCountIn,
}

// ---------------------------------------------------------------------------
// TransportSnapshot
// ---------------------------------------------------------------------------

/// An immutable, cloneable snapshot of the transport state at a single instant.
///
/// Created by [`TransportClock::snapshot`] and intended for UI consumption.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
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
    /// Current beat number within the bar (0-indexed).
    pub current_beat: u8,
    /// Whether the current position is near a beat boundary (for visual flash).
    pub beat_active: bool,
    /// Whether a count-in is currently active (transport is Playing, waiting
    /// for count-in to finish before recording starts).
    pub count_in_active: bool,
    /// Current bar within the count-in period (1-indexed). Zero when inactive.
    pub count_in_bar: u8,
    /// Total count-in bars configured. Zero when inactive.
    pub count_in_total: u8,
    /// The currently configured recording workflow.
    pub recording_workflow: RecordingWorkflow,
    /// Number of bars to auto-record (0 = unlimited / manual stop).
    ///
    /// Derived from the active workflow configuration. The TUI can use this
    /// to display "Recording 2/4 bars" during auto-recording.
    pub auto_record_bars: u8,
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
    recording_workflow: RecordingWorkflow,
    count_in_state: CountInState,
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
            recording_workflow: RecordingWorkflow::FreeRecord,
            count_in_state: CountInState::Inactive,
        }
    }

    /// Current transport state.
    #[must_use]
    pub const fn state(&self) -> TransportState {
        self.state
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
            TransportCommand::SetMetronomeVolume(_db) => {
                // Volume is handled by the Metronome struct directly;
                // this variant exists so the command can route through
                // the transport command pipeline.
            }
            TransportCommand::SetRecordingWorkflow(workflow) => {
                self.recording_workflow = workflow;
            }
            TransportCommand::RecordWithCountIn => {
                // Handled entirely by the output callback, which reads
                // the configured workflow and sets up the count-in state
                // machine via the public helper methods below.
            }
        }
    }

    /// Advance the timeline position by `num_samples`.
    ///
    /// Only advances when the transport is [`Playing`](TransportState::Playing)
    /// or [`Recording`](TransportState::Recording). When a loop region is
    /// active and enabled, the position wraps around the loop boundaries.
    pub fn advance(&mut self, num_samples: u32) -> AdvanceFlags {
        if !self.is_playing() && !self.is_recording() {
            return AdvanceFlags::NONE;
        }

        self.position_samples = self.position_samples.saturating_add(u64::from(num_samples));

        // Handle loop wrapping when looping is enabled and we have a valid region.
        // Loop wrapping is disabled during count-in and auto-recording to
        // prevent the position from wrapping backwards before reaching the
        // count-in end or auto-stop boundary.
        let loop_active =
            self.loop_enabled && matches!(self.count_in_state, CountInState::Inactive);
        if loop_active {
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

        // Check count-in state machine for transitions.
        match self.count_in_state {
            CountInState::Inactive => AdvanceFlags::NONE,
            CountInState::CountingIn {
                count_in_end,
                auto_stop,
                ..
            } => {
                if self.position_samples >= count_in_end {
                    self.count_in_state = CountInState::AutoRecording {
                        record_start: count_in_end,
                        auto_stop,
                    };
                    AdvanceFlags {
                        count_in_completed: true,
                        auto_stop_triggered: false,
                        record_start_position: count_in_end,
                    }
                } else {
                    AdvanceFlags::NONE
                }
            }
            CountInState::AutoRecording { auto_stop, .. } => {
                if auto_stop > 0 && self.position_samples >= auto_stop {
                    self.count_in_state = CountInState::Inactive;
                    AdvanceFlags {
                        count_in_completed: false,
                        auto_stop_triggered: true,
                        record_start_position: 0,
                    }
                } else {
                    AdvanceFlags::NONE
                }
            }
        }
    }

    /// Create an immutable snapshot of the current transport state.
    ///
    /// Intended for reading on the UI thread without blocking the audio thread.
    #[must_use]
    pub fn snapshot(&self) -> TransportSnapshot {
        // Compute beat position for visual indicator.
        let samples_per_beat = if self.bpm > 0.0 && self.bpm.is_finite() {
            f64::from(self.sample_rate) * 60.0 / self.bpm
        } else {
            1.0
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let beat_number = (self.position_samples as f64 / samples_per_beat).floor() as u64;
        #[allow(clippy::cast_possible_truncation)]
        let current_beat = (beat_number % u64::from(self.beats_per_bar)) as u8;

        // Beat is "active" for the first ~100ms after a beat boundary.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let samples_into_beat = (self.position_samples as f64 % samples_per_beat) as u64;
        // 100ms in samples.
        let active_window = u64::from(self.sample_rate) / 10;
        let beat_active = samples_into_beat < active_window;

        // Compute count-in display fields.
        let (count_in_active, count_in_bar, count_in_total) = match self.count_in_state {
            CountInState::Inactive | CountInState::AutoRecording { .. } => (false, 0, 0),
            CountInState::CountingIn {
                count_in_end,
                count_in_bars,
                ..
            } => {
                let spb = self.samples_per_bar();
                let bar = if spb > 0 {
                    let count_in_start =
                        count_in_end.saturating_sub(u64::from(count_in_bars) * spb);
                    let samples_into = self.position_samples.saturating_sub(count_in_start);
                    #[allow(clippy::cast_possible_truncation)]
                    let b = (samples_into / spb) as u8 + 1;
                    b.min(count_in_bars)
                } else {
                    1
                };
                (true, bar, count_in_bars)
            }
        };

        TransportSnapshot {
            state: self.state,
            position: TimePosition::new(self.position_samples, self.sample_rate),
            bpm: self.bpm,
            beats_per_bar: self.beats_per_bar,
            beat_unit: self.beat_unit,
            loop_region: self.loop_region,
            loop_enabled: self.loop_enabled,
            metronome_enabled: self.metronome_enabled,
            current_beat,
            beat_active,
            count_in_active,
            count_in_bar,
            count_in_total,
            recording_workflow: self.recording_workflow,
            auto_record_bars: match self.recording_workflow {
                RecordingWorkflow::FreeRecord => 0,
                RecordingWorkflow::CountIn { record_bars, .. } => record_bars,
                RecordingWorkflow::FixedLength { bars } => bars,
            },
        }
    }

    /// Current timeline position in samples.
    #[must_use]
    pub const fn position_samples(&self) -> u64 {
        self.position_samples
    }

    /// Current tempo in beats per minute.
    #[must_use]
    pub const fn bpm(&self) -> f64 {
        self.bpm
    }

    /// Number of beats per bar (time-signature numerator).
    #[must_use]
    pub const fn beats_per_bar(&self) -> u8 {
        self.beats_per_bar
    }

    /// Whether the metronome is enabled.
    #[must_use]
    pub const fn metronome_enabled(&self) -> bool {
        self.metronome_enabled
    }

    /// The sample rate of this transport clock.
    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The currently configured recording workflow.
    #[must_use]
    pub const fn recording_workflow(&self) -> RecordingWorkflow {
        self.recording_workflow
    }

    /// Number of samples in one bar at the current tempo and time signature.
    ///
    /// Returns 0 if BPM is non-positive or non-finite (defensive).
    #[must_use]
    pub fn samples_per_bar(&self) -> u64 {
        if self.bpm <= 0.0 || !self.bpm.is_finite() {
            return 0;
        }
        let samples_per_beat = f64::from(self.sample_rate) * 60.0 / self.bpm;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let result = (samples_per_beat * f64::from(self.beats_per_bar)) as u64;
        result
    }

    /// Quantize a sample position to the nearest bar boundary.
    ///
    /// Rounds to the nearest bar start. If `samples_per_bar` is 0 (invalid
    /// tempo), returns the position unchanged.
    #[must_use]
    pub fn quantize_to_bar(&self, position: u64) -> u64 {
        let spb = self.samples_per_bar();
        if spb == 0 {
            return position;
        }
        let remainder = position % spb;
        let half = spb / 2;
        if remainder >= half {
            // Round up to the next bar boundary.
            position.saturating_add(spb - remainder)
        } else {
            // Round down to the previous bar boundary.
            position - remainder
        }
    }

    /// Begin a count-in sequence. Sets the transport to Playing (so the
    /// metronome sounds) and configures the count-in state machine.
    ///
    /// After `count_in_bars` bars, [`advance`] will return
    /// [`AdvanceFlags::count_in_completed`] = `true`. If `record_bars` > 0,
    /// it will later return [`AdvanceFlags::auto_stop_triggered`] = `true`
    /// after that many additional bars.
    ///
    /// Does nothing if BPM is invalid (`samples_per_bar` == 0).
    pub fn start_count_in(&mut self, count_in_bars: u8, record_bars: u8) {
        let spb = self.samples_per_bar();
        if spb == 0 || count_in_bars == 0 {
            return;
        }
        let count_in_end = self
            .position_samples
            .saturating_add(u64::from(count_in_bars).saturating_mul(spb));
        let auto_stop = if record_bars > 0 {
            count_in_end.saturating_add(u64::from(record_bars).saturating_mul(spb))
        } else {
            0
        };
        self.count_in_state = CountInState::CountingIn {
            count_in_end,
            auto_stop,
            count_in_bars,
        };
        // Start playing so the metronome sounds during count-in.
        self.state = TransportState::Playing;
    }

    /// Begin a fixed-length recording. Sets the transport to Recording and
    /// configures auto-stop after `bars` bars.
    ///
    /// Does nothing if BPM is invalid or `bars` is 0.
    pub fn start_fixed_length(&mut self, bars: u8) {
        let spb = self.samples_per_bar();
        if spb == 0 || bars == 0 {
            return;
        }
        let auto_stop = self
            .position_samples
            .saturating_add(u64::from(bars).saturating_mul(spb));
        self.count_in_state = CountInState::AutoRecording {
            record_start: self.position_samples,
            auto_stop,
        };
        self.state = TransportState::Recording;
    }

    /// Reset the count-in state machine to Inactive.
    ///
    /// Called by the output callback after handling the auto-stop flag.
    pub const fn reset_count_in(&mut self) {
        self.count_in_state = CountInState::Inactive;
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
        self.count_in_state = CountInState::Inactive;
    }

    /// Pause is valid only from Playing or Recording.
    ///
    /// Pausing also cancels any active count-in sequence. The user must
    /// re-initiate the count-in workflow after resuming.
    const fn handle_pause(&mut self) {
        match self.state {
            TransportState::Playing | TransportState::Recording => {
                self.state = TransportState::Paused;
                self.count_in_state = CountInState::Inactive;
            }
            TransportState::Stopped | TransportState::Paused => {
                // Cannot pause if already stopped or paused -- no-op.
            }
        }
    }

    /// Record is valid from Stopped (start recording from the beginning) or
    /// from Playing (transition to recording, e.g. after a count-in completes).
    const fn handle_record(&mut self) {
        match self.state {
            TransportState::Stopped | TransportState::Playing => {
                self.state = TransportState::Recording;
            }
            TransportState::Recording | TransportState::Paused => {
                // Already recording or paused -- no-op.
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
    fn record_while_playing_transitions_to_recording() {
        // Record from Playing is a valid transition (e.g. after a count-in).
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::Play);
        clock.apply_command(TransportCommand::Record);
        assert_eq!(clock.state, TransportState::Recording);
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

    // -- Recording workflow --------------------------------------------------

    #[test]
    fn default_recording_workflow_is_free_record() {
        let clock = TransportClock::new(44_100);
        assert_eq!(clock.recording_workflow(), RecordingWorkflow::FreeRecord);
    }

    #[test]
    fn set_recording_workflow_updates_state() {
        let mut clock = TransportClock::new(44_100);
        let workflow = RecordingWorkflow::CountIn {
            count_in_bars: 2,
            record_bars: 4,
        };
        clock.apply_command(TransportCommand::SetRecordingWorkflow(workflow));
        assert_eq!(clock.recording_workflow(), workflow);
    }

    #[test]
    fn set_recording_workflow_fixed_length() {
        let mut clock = TransportClock::new(44_100);
        let workflow = RecordingWorkflow::FixedLength { bars: 8 };
        clock.apply_command(TransportCommand::SetRecordingWorkflow(workflow));
        assert_eq!(clock.recording_workflow(), workflow);
    }

    // -- samples_per_bar / quantize_to_bar -----------------------------------

    #[test]
    fn samples_per_bar_at_120_bpm_4_4() {
        let clock = TransportClock::new(44_100);
        // 120 BPM, 4/4: one beat = 44100 * 60 / 120 = 22050 samples.
        // One bar = 4 beats = 88200 samples.
        assert_eq!(clock.samples_per_bar(), 88_200);
    }

    #[test]
    fn samples_per_bar_at_60_bpm_3_4() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::SetTempo(60.0));
        clock.apply_command(TransportCommand::SetTimeSignature(3, 4));
        // 60 BPM, 3/4: one beat = 44100 samples, one bar = 3 * 44100 = 132300.
        assert_eq!(clock.samples_per_bar(), 132_300);
    }

    #[test]
    fn samples_per_bar_zero_bpm_returns_zero() {
        // Cannot actually set BPM to 0 (clamped), but test the guard.
        let clock = TransportClock::new(0);
        // sample_rate=1, bpm=120, beats=4 => 1*60/120*4 = 2
        assert_eq!(clock.samples_per_bar(), 2);
    }

    #[test]
    fn quantize_to_bar_rounds_down() {
        let clock = TransportClock::new(44_100);
        // spb = 88200. Position 10000 is closer to 0 than to 88200.
        assert_eq!(clock.quantize_to_bar(10_000), 0);
    }

    #[test]
    fn quantize_to_bar_rounds_up() {
        let clock = TransportClock::new(44_100);
        // spb = 88200. Position 60000 is closer to 88200 than to 0.
        assert_eq!(clock.quantize_to_bar(60_000), 88_200);
    }

    #[test]
    fn quantize_to_bar_exact_boundary() {
        let clock = TransportClock::new(44_100);
        // Exactly on the boundary should stay there.
        assert_eq!(clock.quantize_to_bar(88_200), 88_200);
    }

    #[test]
    fn quantize_to_bar_midpoint() {
        let clock = TransportClock::new(44_100);
        // Exactly at half a bar (44100) — rounds up.
        assert_eq!(clock.quantize_to_bar(44_100), 88_200);
    }

    // -- Count-in state machine ----------------------------------------------

    #[test]
    fn start_count_in_sets_playing_state() {
        let mut clock = TransportClock::new(44_100);
        clock.start_count_in(2, 4);
        assert!(clock.is_playing());
    }

    #[test]
    fn start_count_in_zero_bars_is_noop() {
        let mut clock = TransportClock::new(44_100);
        clock.start_count_in(0, 4);
        // Should remain stopped since count_in_bars=0.
        assert_eq!(clock.state, TransportState::Stopped);
    }

    #[test]
    fn count_in_completes_after_bars_elapsed() {
        let mut clock = TransportClock::new(44_100);
        // 120 BPM, 4/4 => one bar = 88200 samples. Count-in of 1 bar.
        clock.start_count_in(1, 4);

        // Advance less than one bar — no flag yet.
        let flags = clock.advance(44_100); // half a bar
        assert!(!flags.count_in_completed);
        assert!(!flags.auto_stop_triggered);

        // Advance past the bar boundary.
        let flags = clock.advance(44_100); // total = 88200
        assert!(flags.count_in_completed);
        assert!(!flags.auto_stop_triggered);
    }

    #[test]
    fn count_in_then_auto_stop() {
        let mut clock = TransportClock::new(44_100);
        // 1 bar count-in, 1 bar recording.
        clock.start_count_in(1, 1);

        // Advance through count-in (88200 samples).
        let flags = clock.advance(88_200);
        assert!(flags.count_in_completed);
        assert!(!flags.auto_stop_triggered);

        // Now in AutoRecording. Advance through 1 more bar.
        // The state machine should trigger auto-stop.
        let flags = clock.advance(88_200);
        assert!(!flags.count_in_completed);
        assert!(flags.auto_stop_triggered);
    }

    #[test]
    fn count_in_unlimited_recording_no_auto_stop() {
        let mut clock = TransportClock::new(44_100);
        // 1 bar count-in, 0 = unlimited recording.
        clock.start_count_in(1, 0);

        // Advance through count-in.
        let flags = clock.advance(88_200);
        assert!(flags.count_in_completed);

        // Now in AutoRecording with auto_stop=0. Advance a lot — no auto-stop.
        let flags = clock.advance(88_200 * 100);
        assert!(!flags.auto_stop_triggered);
    }

    #[test]
    fn start_fixed_length_sets_recording_state() {
        let mut clock = TransportClock::new(44_100);
        clock.start_fixed_length(4);
        assert!(clock.is_recording());
    }

    #[test]
    fn start_fixed_length_zero_bars_is_noop() {
        let mut clock = TransportClock::new(44_100);
        clock.start_fixed_length(0);
        assert_eq!(clock.state, TransportState::Stopped);
    }

    #[test]
    fn fixed_length_auto_stops_after_bars() {
        let mut clock = TransportClock::new(44_100);
        clock.start_fixed_length(1); // 1 bar = 88200 samples

        // Advance less than a bar.
        let flags = clock.advance(44_100);
        assert!(!flags.auto_stop_triggered);

        // Advance past the bar boundary.
        let flags = clock.advance(44_100);
        assert!(flags.auto_stop_triggered);
    }

    #[test]
    fn stop_resets_count_in_state() {
        let mut clock = TransportClock::new(44_100);
        clock.start_count_in(2, 4);
        assert!(clock.is_playing());

        clock.apply_command(TransportCommand::Stop);
        assert_eq!(clock.state, TransportState::Stopped);
        assert_eq!(clock.count_in_state, CountInState::Inactive);
    }

    #[test]
    fn reset_count_in_clears_state() {
        let mut clock = TransportClock::new(44_100);
        clock.start_count_in(2, 4);
        clock.reset_count_in();
        assert_eq!(clock.count_in_state, CountInState::Inactive);
    }

    #[test]
    fn advance_returns_none_when_stopped() {
        let mut clock = TransportClock::new(44_100);
        let flags = clock.advance(256);
        assert_eq!(flags, AdvanceFlags::NONE);
    }

    #[test]
    fn advance_returns_none_when_no_count_in() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::Play);
        let flags = clock.advance(256);
        assert_eq!(flags, AdvanceFlags::NONE);
    }

    // -- Snapshot count-in fields --------------------------------------------

    #[test]
    fn snapshot_count_in_inactive() {
        let clock = TransportClock::new(44_100);
        let snap = clock.snapshot();
        assert!(!snap.count_in_active);
        assert_eq!(snap.count_in_bar, 0);
        assert_eq!(snap.count_in_total, 0);
        assert_eq!(snap.recording_workflow, RecordingWorkflow::FreeRecord);
    }

    #[test]
    fn snapshot_count_in_active() {
        let mut clock = TransportClock::new(44_100);
        clock.start_count_in(4, 0);

        let snap = clock.snapshot();
        assert!(snap.count_in_active);
        assert_eq!(snap.count_in_bar, 1); // first bar
        assert_eq!(snap.count_in_total, 4);
    }

    #[test]
    fn snapshot_count_in_advances_bar_number() {
        let mut clock = TransportClock::new(44_100);
        clock.start_count_in(4, 0);

        // Advance past the first bar (88200 samples at 120 BPM, 4/4).
        clock.advance(88_200);
        let snap = clock.snapshot();
        assert!(snap.count_in_active);
        assert_eq!(snap.count_in_bar, 2); // second bar
    }

    #[test]
    fn snapshot_recording_workflow_reflects_config() {
        let mut clock = TransportClock::new(44_100);
        let workflow = RecordingWorkflow::CountIn {
            count_in_bars: 2,
            record_bars: 8,
        };
        clock.apply_command(TransportCommand::SetRecordingWorkflow(workflow));

        let snap = clock.snapshot();
        assert_eq!(snap.recording_workflow, workflow);
    }

    #[test]
    fn count_in_two_bars_completion() {
        let mut clock = TransportClock::new(44_100);
        // 2 bar count-in, 2 bar recording.
        clock.start_count_in(2, 2);

        // Advance 1 bar — still counting in.
        let flags = clock.advance(88_200);
        assert!(!flags.count_in_completed);

        // Advance 1 more bar — count-in completes.
        let flags = clock.advance(88_200);
        assert!(flags.count_in_completed);

        // Advance 1 bar — still recording.
        let flags = clock.advance(88_200);
        assert!(!flags.auto_stop_triggered);

        // Advance 1 more bar — auto-stop.
        let flags = clock.advance(88_200);
        assert!(flags.auto_stop_triggered);
    }

    #[test]
    fn record_with_count_in_command_is_noop_on_transport() {
        // RecordWithCountIn doesn't change TransportClock directly.
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::RecordWithCountIn);
        assert_eq!(clock.state, TransportState::Stopped);
    }

    // -- Issue #1 fix: Record from Playing state ----------------------------

    #[test]
    fn record_from_playing_transitions_to_recording() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::Play);
        assert_eq!(clock.state, TransportState::Playing);

        clock.apply_command(TransportCommand::Record);
        assert_eq!(clock.state, TransportState::Recording);
    }

    #[test]
    fn count_in_then_record_transitions_correctly() {
        let mut clock = TransportClock::new(44_100);
        clock.start_count_in(1, 0); // 1 bar count-in
        assert!(clock.is_playing());

        // Complete the count-in.
        let flags = clock.advance(88_200);
        assert!(flags.count_in_completed);

        // Simulating what output callback does: apply Record command.
        clock.apply_command(TransportCommand::Record);
        assert!(clock.is_recording());
    }

    // -- Issue #2 fix: Loop wrapping suppressed during count-in ---------------

    #[test]
    fn loop_wrapping_suppressed_during_count_in() {
        let mut clock = TransportClock::new(44_100);
        // Loop region smaller than one bar.
        clock.apply_command(TransportCommand::SetLoop(Some((0, 44_100))));
        clock.start_count_in(1, 0);

        // Advance past the loop end. Without the fix, position would wrap
        // to 0 and never reach the count-in end (88200).
        let flags = clock.advance(88_200);
        assert_eq!(clock.position_samples, 88_200);
        assert!(flags.count_in_completed);
    }

    #[test]
    fn loop_wrapping_resumes_after_count_in() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::SetLoop(Some((0, 44_100))));
        clock.start_count_in(1, 0);

        // Complete count-in.
        let flags = clock.advance(88_200);
        assert!(flags.count_in_completed);

        // Count-in state transitions to AutoRecording(auto_stop=0).
        // Now reset count-in (simulating what output callback does
        // after it's done handling the flags and recording is underway).
        // Once count-in state is Inactive, loop wrapping should resume.
        clock.reset_count_in();
        clock.apply_command(TransportCommand::Seek(44_000));
        let flags = clock.advance(200);
        // 44000 + 200 = 44200 >= 44100 (loop end), wraps to 100.
        assert_eq!(clock.position_samples, 100);
        assert_eq!(flags, AdvanceFlags::NONE);
    }

    // -- Issue #5 fix: auto_record_bars in snapshot --------------------------

    #[test]
    fn snapshot_auto_record_bars_free_record() {
        let clock = TransportClock::new(44_100);
        let snap = clock.snapshot();
        assert_eq!(snap.auto_record_bars, 0);
    }

    #[test]
    fn snapshot_auto_record_bars_count_in() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::SetRecordingWorkflow(
            RecordingWorkflow::CountIn {
                count_in_bars: 2,
                record_bars: 8,
            },
        ));
        let snap = clock.snapshot();
        assert_eq!(snap.auto_record_bars, 8);
    }

    #[test]
    fn snapshot_auto_record_bars_fixed_length() {
        let mut clock = TransportClock::new(44_100);
        clock.apply_command(TransportCommand::SetRecordingWorkflow(
            RecordingWorkflow::FixedLength { bars: 4 },
        ));
        let snap = clock.snapshot();
        assert_eq!(snap.auto_record_bars, 4);
    }

    // -- Issue #7 fix: Pause cancels count-in --------------------------------

    #[test]
    fn pause_during_count_in_cancels_count_in() {
        let mut clock = TransportClock::new(44_100);
        clock.start_count_in(4, 0);
        assert!(clock.is_playing());
        assert_eq!(
            clock.count_in_state,
            CountInState::CountingIn {
                count_in_end: 88_200 * 4,
                auto_stop: 0,
                count_in_bars: 4,
            }
        );

        clock.advance(44_100);
        clock.apply_command(TransportCommand::Pause);
        assert_eq!(clock.state, TransportState::Paused);
        assert_eq!(clock.count_in_state, CountInState::Inactive);
    }

    #[test]
    fn pause_during_count_in_snapshot_shows_inactive() {
        let mut clock = TransportClock::new(44_100);
        clock.start_count_in(2, 0);
        clock.advance(44_100);
        clock.apply_command(TransportCommand::Pause);

        let snap = clock.snapshot();
        assert!(!snap.count_in_active);
        assert_eq!(snap.count_in_bar, 0);
        assert_eq!(snap.count_in_total, 0);
    }

    #[test]
    fn pause_during_auto_recording_cancels_auto_stop() {
        let mut clock = TransportClock::new(44_100);
        clock.start_fixed_length(2);
        assert!(clock.is_recording());

        clock.advance(44_100);
        clock.apply_command(TransportCommand::Pause);
        assert_eq!(clock.state, TransportState::Paused);
        assert_eq!(clock.count_in_state, CountInState::Inactive);
    }

    // -- Edge cases ----------------------------------------------------------

    #[test]
    fn samples_per_bar_at_48k() {
        let mut clock = TransportClock::new(48_000);
        clock.apply_command(TransportCommand::SetTempo(120.0));
        // 48000 * 60 / 120 = 24000 samples per beat, 4 beats = 96000.
        assert_eq!(clock.samples_per_bar(), 96_000);
    }

    #[test]
    fn quantize_to_bar_zero_position() {
        let clock = TransportClock::new(44_100);
        assert_eq!(clock.quantize_to_bar(0), 0);
    }

    #[test]
    fn count_in_large_overshoot_still_completes() {
        let mut clock = TransportClock::new(44_100);
        clock.start_count_in(1, 1);
        // Advance way past both count-in and auto-stop in a single block.
        let flags = clock.advance(88_200 * 3);
        // count_in_completed fires first.
        assert!(flags.count_in_completed);
    }

    #[test]
    fn fixed_length_large_overshoot_auto_stops() {
        let mut clock = TransportClock::new(44_100);
        clock.start_fixed_length(1);
        // Advance way past the auto-stop in a single block.
        let flags = clock.advance(88_200 * 3);
        assert!(flags.auto_stop_triggered);
    }

    #[test]
    fn count_in_completed_carries_exact_bar_boundary() {
        let mut clock = TransportClock::new(44_100);
        // 120 BPM, 4/4 => one bar = 88200 samples. Count-in of 1 bar.
        clock.start_count_in(1, 4);

        // Advance past the bar boundary with overshoot.
        let flags = clock.advance(90_000); // overshoots by 1800 samples
        assert!(flags.count_in_completed);
        // record_start_position should be the exact bar boundary (88200),
        // not the overshot position (90000).
        assert_eq!(flags.record_start_position, 88_200);
    }

    #[test]
    fn count_in_completed_multi_bar_boundary() {
        let mut clock = TransportClock::new(44_100);
        // 2 bar count-in => boundary at 176400 samples.
        clock.start_count_in(2, 0);

        let flags = clock.advance(176_500); // overshoots by 100 samples
        assert!(flags.count_in_completed);
        assert_eq!(flags.record_start_position, 176_400);
    }

    #[test]
    fn auto_stop_record_start_position_is_zero() {
        let mut clock = TransportClock::new(44_100);
        clock.start_count_in(1, 1);

        // Complete count-in.
        let flags = clock.advance(88_200);
        assert!(flags.count_in_completed);
        assert_eq!(flags.record_start_position, 88_200);

        // Auto-stop: record_start_position is irrelevant (0).
        let flags = clock.advance(88_200);
        assert!(flags.auto_stop_triggered);
        assert_eq!(flags.record_start_position, 0);
    }
}
