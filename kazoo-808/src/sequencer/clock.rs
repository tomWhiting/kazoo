//! Sequencer clock with swing and hub transport sync.
//!
//! Drives the 16-step sequencer at a configurable BPM with per-step
//! swing applied to alternate steps.

use super::STEPS_PER_PATTERN;

/// Clock division relative to quarter note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockDivision {
    Eighth,
    Sixteenth,
    ThirtySecond,
}

impl ClockDivision {
    /// Steps per beat (quarter note).
    #[must_use]
    pub const fn steps_per_beat(self) -> f64 {
        match self {
            Self::Eighth => 2.0,
            Self::Sixteenth => 4.0,
            Self::ThirtySecond => 8.0,
        }
    }
}

/// Sample-accurate step clock with swing.
///
/// Tracks position within a 16-step pattern and fires step triggers
/// at the correct sample offsets. Swing delays every other step.
#[derive(Debug)]
pub struct SequencerClock {
    sample_rate: f64,
    bpm: f64,
    /// Swing amount: 50.0 = straight, up to 75.0 = heavy swing.
    swing: f64,
    division: ClockDivision,
    /// Remaining samples until the next step fires.
    samples_until_next: f64,
    /// Current step index (0..`STEPS_PER_PATTERN`).
    current_step: usize,
}

impl SequencerClock {
    /// Default BPM.
    pub const DEFAULT_BPM: f64 = 120.0;
    /// Default swing (straight).
    pub const DEFAULT_SWING: f64 = 50.0;

    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let mut clock = Self {
            sample_rate: f64::from(sample_rate.max(1.0)),
            bpm: Self::DEFAULT_BPM,
            swing: Self::DEFAULT_SWING,
            division: ClockDivision::Sixteenth,
            samples_until_next: 0.0,
            current_step: 0,
        };
        clock.samples_until_next = clock.step_duration_samples();
        clock
    }

    /// Set BPM (20-300).
    pub const fn set_bpm(&mut self, bpm: f64) {
        self.bpm = bpm.clamp(20.0, 300.0);
    }

    /// Get current BPM.
    #[must_use]
    pub const fn bpm(&self) -> f64 {
        self.bpm
    }

    /// Set swing amount (50 = straight, 75 = heavy swing).
    pub const fn set_swing(&mut self, swing: f64) {
        self.swing = swing.clamp(50.0, 75.0);
    }

    /// Get current swing.
    #[must_use]
    pub const fn swing(&self) -> f64 {
        self.swing
    }

    /// Set clock division.
    pub const fn set_division(&mut self, division: ClockDivision) {
        self.division = division;
    }

    /// Current step position (0..15).
    #[must_use]
    pub const fn current_step(&self) -> usize {
        self.current_step
    }

    /// Reset to step 0.
    pub fn reset(&mut self) {
        self.current_step = 0;
        self.samples_until_next = self.step_duration_samples();
    }

    /// Advance clock by one sample. Returns `Some(step_index)` if a
    /// new step should trigger this sample.
    pub fn tick(&mut self) -> Option<usize> {
        self.samples_until_next -= 1.0;
        if self.samples_until_next <= 0.0 {
            let step = self.current_step;
            self.current_step = (self.current_step + 1) % STEPS_PER_PATTERN;
            // Compute duration for the NEXT interval.
            self.samples_until_next += self.step_duration_samples();
            Some(step)
        } else {
            None
        }
    }

    /// Compute the duration in samples for the current step interval.
    ///
    /// Swing works by adjusting the timing of step pairs. For each pair
    /// of steps (0-1, 2-3, 4-5, ...), the total duration equals two
    /// base step periods. The swing ratio determines how that duration
    /// is split: at 50% both are equal (straight), at 66% the first step
    /// gets 2/3 and the second 1/3 (triplet feel).
    fn step_duration_samples(&self) -> f64 {
        let base_step = self.sample_rate * 60.0 / (self.bpm * self.division.steps_per_beat());
        let pair_duration = base_step * 2.0;
        let swing_ratio = self.swing / 100.0;

        if self.current_step % 2 == 0 {
            pair_duration * swing_ratio
        } else {
            pair_duration * (1.0 - swing_ratio)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_fires_at_correct_intervals() {
        let mut clock = SequencerClock::new(44100.0);
        clock.set_bpm(120.0);
        clock.set_swing(50.0);

        // At 120 BPM, 16th notes = 4 per beat = 0.125s per step.
        // 0.125 * 44100 = 5512.5 samples per step.
        let expected_period = 5512.5;

        let mut step_positions = Vec::new();
        let mut total_samples = 0u64;

        for _ in 0..100_000 {
            if let Some(step) = clock.tick() {
                step_positions.push((step, total_samples));
            }
            total_samples += 1;
        }

        assert!(
            step_positions.len() >= 16,
            "should have at least 16 steps in 100k samples"
        );

        for i in 1..step_positions.len().min(10) {
            let interval = (step_positions[i].1 - step_positions[i - 1].1) as f64;
            assert!(
                (interval - expected_period).abs() < 2.0,
                "step interval should be ~{expected_period}, got {interval}"
            );
        }
    }

    #[test]
    fn clock_swing_alters_timing() {
        let mut clock = SequencerClock::new(44100.0);
        clock.set_bpm(120.0);
        clock.set_swing(66.6);

        let mut step_positions = Vec::new();
        let mut total_samples = 0u64;

        for _ in 0..50_000 {
            if let Some(step) = clock.tick() {
                step_positions.push((step, total_samples));
            }
            total_samples += 1;
        }

        if step_positions.len() >= 3 {
            let int_0_1 = (step_positions[1].1 - step_positions[0].1) as f64;
            let int_1_2 = (step_positions[2].1 - step_positions[1].1) as f64;
            assert!(
                (int_0_1 - int_1_2).abs() > 100.0,
                "swing should create uneven intervals: {int_0_1} vs {int_1_2}"
            );
        }
    }

    #[test]
    fn clock_wraps_at_pattern_length() {
        let mut clock = SequencerClock::new(44100.0);
        clock.set_bpm(300.0);

        let mut seen_steps = [false; STEPS_PER_PATTERN];
        for _ in 0..500_000 {
            if let Some(step) = clock.tick() {
                assert!(step < STEPS_PER_PATTERN);
                seen_steps[step] = true;
            }
        }
        for (i, &seen) in seen_steps.iter().enumerate() {
            assert!(seen, "step {i} was never triggered");
        }
    }

    #[test]
    fn clock_reset() {
        let mut clock = SequencerClock::new(44100.0);
        for _ in 0..10_000 {
            clock.tick();
        }
        clock.reset();
        assert_eq!(clock.current_step(), 0);
    }
}
