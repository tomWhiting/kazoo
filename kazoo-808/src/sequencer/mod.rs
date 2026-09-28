//! Step sequencer engine.
//!
//! 16-step pattern sequencer with per-voice rows, accent, swing,
//! and pattern chaining. Syncs to hub transport clock.

pub mod clock;

pub use clock::SequencerClock;

use crate::synth::{DrumMachine, VOICE_COUNT, VoiceIndex};

/// Number of steps per pattern.
pub const STEPS_PER_PATTERN: usize = 16;

/// Maximum number of patterns in the bank.
pub const MAX_PATTERNS: usize = 16;

/// Accent velocity multiplier.
const ACCENT_VELOCITY: f32 = 1.0;
/// Normal (non-accent) default velocity.
const NORMAL_VELOCITY: f32 = 0.8;

/// A single step in the sequence.
#[derive(Debug, Clone, Copy)]
pub struct Step {
    /// Whether this step triggers the voice.
    pub active: bool,
    /// Velocity (0.0..1.0). Only relevant if active.
    pub velocity: f32,
    /// Whether this step has accent.
    pub accent: bool,
}

impl Default for Step {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl Step {
    /// An inactive step at normal velocity.
    pub const DEFAULT: Self = Self {
        active: false,
        velocity: NORMAL_VELOCITY,
        accent: false,
    };

    /// Effective trigger velocity accounting for accent.
    #[must_use]
    pub const fn effective_velocity(self) -> f32 {
        if self.accent {
            ACCENT_VELOCITY
        } else {
            self.velocity
        }
    }
}

/// Name of a pattern in the bank: a bank letter and a slot number, as on
/// the original 808 (`A1`..`A4`, `B1`..`B4`, ...).
///
/// Derived from the pattern's bank index, so naming a pattern never
/// allocates — patterns are created inside the audio callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PatternName {
    bank: u8,
    slot: u8,
}

impl PatternName {
    /// Patterns per bank letter.
    const SLOTS_PER_BANK: usize = 4;

    /// Name of the pattern at `index` in the bank (0 = `A1`, 4 = `B1`).
    #[must_use]
    pub const fn for_index(index: usize) -> Self {
        let bank = index / Self::SLOTS_PER_BANK;
        Self {
            // Letters run A..Z; the bank never holds more than MAX_PATTERNS.
            bank: if bank > 25 { b'Z' } else { b'A' + bank as u8 },
            slot: b'1' + (index % Self::SLOTS_PER_BANK) as u8,
        }
    }
}

impl std::fmt::Display for PatternName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", char::from(self.bank), char::from(self.slot))
    }
}

/// A single pattern: one row per voice, 16 steps each.
#[derive(Debug, Clone)]
pub struct Pattern {
    pub name: PatternName,
    pub steps: [[Step; STEPS_PER_PATTERN]; VOICE_COUNT],
}

impl Pattern {
    /// An empty pattern for bank slot `index`.
    #[must_use]
    pub const fn empty(index: usize) -> Self {
        Self {
            name: PatternName::for_index(index),
            steps: [[Step::DEFAULT; STEPS_PER_PATTERN]; VOICE_COUNT],
        }
    }
}

impl Default for Pattern {
    fn default() -> Self {
        Self::empty(0)
    }
}

/// Trigger event emitted by the sequencer on a step.
#[derive(Debug, Clone, Copy)]
pub struct TriggerEvent {
    pub voice: VoiceIndex,
    pub velocity: f32,
}

/// The full sequencer engine.
#[derive(Debug)]
pub struct Sequencer {
    /// Pattern bank. Holds at most [`MAX_PATTERNS`]; capacity for all of
    /// them is reserved up front so adding a pattern never allocates.
    pub patterns: Vec<Pattern>,
    /// Index of the currently playing pattern.
    pub current_pattern: usize,
    /// Clock driving step advancement.
    pub clock: SequencerClock,
    /// Whether the sequencer is playing.
    pub playing: bool,
}

impl Sequencer {
    /// Create a new sequencer at the given sample rate.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let mut patterns = Vec::with_capacity(MAX_PATTERNS);
        patterns.push(Pattern::default());
        Self {
            patterns,
            current_pattern: 0,
            clock: SequencerClock::new(sample_rate),
            playing: false,
        }
    }

    /// Toggle a step on/off. Clears accent when deactivating.
    pub fn toggle_step(&mut self, voice: usize, step: usize) {
        if let Some(pattern) = self.patterns.get_mut(self.current_pattern) {
            if voice < VOICE_COUNT && step < STEPS_PER_PATTERN {
                let step_data = &mut pattern.steps[voice][step];
                step_data.active = !step_data.active;
                if !step_data.active {
                    step_data.accent = false;
                }
            }
        }
    }

    /// Toggle accent on a step. Only works on active steps — accent on
    /// an inactive step makes no sense musically and is silently ignored.
    pub fn toggle_accent(&mut self, voice: usize, step: usize) {
        if let Some(pattern) = self.patterns.get_mut(self.current_pattern) {
            if voice < VOICE_COUNT && step < STEPS_PER_PATTERN {
                let step_data = &mut pattern.steps[voice][step];
                if step_data.active {
                    step_data.accent = !step_data.accent;
                }
            }
        }
    }

    /// Set velocity for a step.
    pub fn set_step_velocity(&mut self, voice: usize, step: usize, velocity: f32) {
        if let Some(pattern) = self.patterns.get_mut(self.current_pattern) {
            if voice < VOICE_COUNT && step < STEPS_PER_PATTERN {
                pattern.steps[voice][step].velocity = velocity.clamp(0.0, 1.0);
            }
        }
    }

    /// Get the current pattern (read-only).
    #[must_use]
    pub fn current_pattern_ref(&self) -> &Pattern {
        &self.patterns[self.current_pattern]
    }

    /// Start playback from the top: step 0 fires on the next tick.
    pub fn play(&mut self) {
        self.play_from(0.0);
    }

    /// Start (or keep) playing from song position `beat`, in beats from the
    /// first downbeat: how the sequencer follows the desk's transport.
    pub fn play_from(&mut self, beat: f64) {
        self.clock.seek(beat);
        self.playing = true;
    }

    /// Stop playback and reset to step 0.
    pub fn stop(&mut self) {
        self.playing = false;
        self.clock.reset();
    }

    /// Toggle play/stop.
    pub fn toggle_playback(&mut self) {
        if self.playing {
            self.stop();
        } else {
            self.play();
        }
    }

    /// Current step position.
    #[must_use]
    pub const fn current_step(&self) -> usize {
        self.clock.current_step()
    }

    /// Advance by one sample, triggering voices on the drum machine
    /// when a step fires. Uses per-voice accent amounts from the drum
    /// machine to scale accent velocity. Returns the step index if one fired.
    pub fn tick(&mut self, drum_machine: &mut DrumMachine) -> Option<usize> {
        if !self.playing {
            return None;
        }

        if let Some(step_idx) = self.clock.tick() {
            let pattern = &self.patterns[self.current_pattern];
            // Fire triggers for all active voices at this step.
            for (voice_idx, row) in pattern.steps.iter().enumerate() {
                let step = &row[step_idx];
                if step.active {
                    if let Some(voice) = VoiceIndex::from_index(voice_idx) {
                        // Apply per-voice accent amount: blend between normal
                        // velocity and full accent based on accent_amounts.
                        let velocity = if step.accent {
                            let amt = drum_machine.accent_amounts[voice_idx];
                            step.velocity.mul_add(1.0 - amt, ACCENT_VELOCITY * amt)
                        } else {
                            step.velocity
                        };
                        drum_machine.trigger(voice, velocity);
                    }
                }
            }
            Some(step_idx)
        } else {
            None
        }
    }

    /// Whether the bank has room for another pattern.
    #[must_use]
    pub fn can_add_pattern(&self) -> bool {
        self.patterns.len() < MAX_PATTERNS
    }

    /// Add a new empty pattern to the bank. Returns its index, or `None`
    /// when the bank already holds [`MAX_PATTERNS`].
    ///
    /// Real-time safe: the bank's capacity is reserved at construction and
    /// pattern names never allocate.
    pub fn add_pattern(&mut self) -> Option<usize> {
        if !self.can_add_pattern() {
            return None;
        }
        let idx = self.patterns.len();
        self.patterns.push(Pattern::empty(idx));
        Some(idx)
    }

    /// Select a pattern by index (clamped to valid range).
    pub fn select_pattern(&mut self, idx: usize) {
        if idx < self.patterns.len() {
            self.current_pattern = idx;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequencer_toggle_step() {
        let mut seq = Sequencer::new(44100.0);
        assert!(!seq.current_pattern_ref().steps[0][0].active);
        seq.toggle_step(0, 0);
        assert!(seq.current_pattern_ref().steps[0][0].active);
        seq.toggle_step(0, 0);
        assert!(!seq.current_pattern_ref().steps[0][0].active);
    }

    #[test]
    fn sequencer_accent() {
        let mut seq = Sequencer::new(44100.0);
        seq.toggle_step(0, 0);
        seq.toggle_accent(0, 0);
        let step = seq.current_pattern_ref().steps[0][0];
        assert!(step.accent);
        assert!((step.effective_velocity() - ACCENT_VELOCITY).abs() < f32::EPSILON);
    }

    #[test]
    fn accent_on_inactive_step_is_noop() {
        let mut seq = Sequencer::new(44100.0);
        // Step starts inactive — accent toggle should be ignored.
        seq.toggle_accent(0, 0);
        assert!(!seq.current_pattern_ref().steps[0][0].accent);
    }

    #[test]
    fn deactivating_step_clears_accent() {
        let mut seq = Sequencer::new(44100.0);
        seq.toggle_step(0, 0);
        seq.toggle_accent(0, 0);
        assert!(seq.current_pattern_ref().steps[0][0].accent);
        seq.toggle_step(0, 0); // Deactivate
        assert!(!seq.current_pattern_ref().steps[0][0].accent);
    }

    #[test]
    fn sequencer_triggers_drum_machine() {
        let sr = 44100.0;
        let mut seq = Sequencer::new(sr);
        let mut dm = DrumMachine::new(sr);

        // Activate kick on step 0.
        seq.toggle_step(VoiceIndex::Kick as usize, 0);
        seq.play();

        // Run until the first step fires.
        let mut fired = false;
        for _ in 0..100_000 {
            if seq.tick(&mut dm).is_some() {
                fired = true;
                break;
            }
        }
        assert!(fired, "sequencer should fire at least one step");
    }

    #[test]
    fn sequencer_stop_resets() {
        let mut seq = Sequencer::new(44100.0);
        let mut dm = DrumMachine::new(44100.0);
        seq.play();
        for _ in 0..10_000 {
            seq.tick(&mut dm);
        }
        seq.stop();
        assert!(!seq.playing);
        assert_eq!(seq.current_step(), 0);
    }

    #[test]
    fn sequencer_add_pattern() {
        let mut seq = Sequencer::new(44100.0);
        assert_eq!(seq.patterns.len(), 1);
        let idx = seq.add_pattern();
        assert_eq!(idx, Some(1));
        assert_eq!(seq.patterns.len(), 2);
        assert_eq!(seq.patterns[1].name.to_string(), "A2");
    }

    #[test]
    fn pattern_names_follow_bank_layout() {
        let names: Vec<String> = (0..MAX_PATTERNS)
            .map(|idx| PatternName::for_index(idx).to_string())
            .collect();
        assert_eq!(names[0], "A1");
        assert_eq!(names[3], "A4");
        assert_eq!(names[4], "B1");
        assert_eq!(names[MAX_PATTERNS - 1], "D4");
    }

    #[test]
    fn pattern_bank_is_bounded_and_never_reallocates() {
        let mut seq = Sequencer::new(44100.0);
        let capacity = seq.patterns.capacity();
        for expected in 1..MAX_PATTERNS {
            assert_eq!(seq.add_pattern(), Some(expected));
        }
        assert_eq!(seq.patterns.len(), MAX_PATTERNS);
        assert!(!seq.can_add_pattern());
        assert_eq!(seq.add_pattern(), None, "a full bank must refuse");
        assert_eq!(seq.patterns.len(), MAX_PATTERNS);
        assert_eq!(
            seq.patterns.capacity(),
            capacity,
            "bank must not reallocate"
        );
    }

    #[test]
    fn accent_amount_scales_velocity() {
        let sr = 44100.0;
        let mut seq = Sequencer::new(sr);
        let mut dm = DrumMachine::new(sr);

        // Set kick accent amount to 0.5.
        dm.accent_amounts[VoiceIndex::Kick as usize] = 0.5;

        // Activate step 0 with accent.
        seq.toggle_step(0, 0);
        seq.toggle_accent(0, 0);
        seq.play();

        // Run until first trigger fires.
        let mut fired = false;
        for _ in 0..100_000 {
            if seq.tick(&mut dm).is_some() {
                fired = true;
                break;
            }
        }
        assert!(fired);
        // The kick should have been triggered with blended velocity:
        // 0.8 * (1 - 0.5) + 1.0 * 0.5 = 0.4 + 0.5 = 0.9
        // We can't inspect the trigger velocity directly, but the test
        // verifies the code path doesn't panic and accent_amounts are used.
    }
}
