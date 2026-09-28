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
        clock.reset();
        clock
    }

    /// Set BPM (20-300). A non-finite value (for example a corrupt hub
    /// transport message) is ignored: NaN would stall the clock forever.
    pub const fn set_bpm(&mut self, bpm: f64) {
        if bpm.is_finite() {
            self.bpm = bpm.clamp(20.0, 300.0);
        }
    }

    /// Get current BPM.
    #[must_use]
    pub const fn bpm(&self) -> f64 {
        self.bpm
    }

    /// Set swing amount (50 = straight, 75 = heavy swing). A non-finite
    /// value is ignored: NaN would stall the clock forever.
    pub const fn set_swing(&mut self, swing: f64) {
        if swing.is_finite() {
            self.swing = swing.clamp(50.0, 75.0);
        }
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

    /// Reset to the start of the pattern: step 0 fires on the next tick.
    pub fn reset(&mut self) {
        self.seek(0.0);
    }

    /// Move to song position `beat` (in beats from the first downbeat), so
    /// the clock is where a clock started on beat 0 at this tempo and swing
    /// would be. A step that falls exactly on `beat` fires on the next
    /// tick; otherwise the next step fires on the first tick at or after its
    /// time. A non-finite or negative position counts as beat 0.
    pub fn seek(&mut self, beat: f64) {
        let beat = if beat.is_finite() { beat.max(0.0) } else { 0.0 };
        let base = self.base_step_samples();
        // Position in steps, and within its swing pair (two steps).
        let position = beat * self.division.steps_per_beat();
        let pair = (position / 2.0).floor();
        let within = position - pair * 2.0;
        let split = 2.0 * self.swing / 100.0;
        // Song positions are small and non-negative: the conversion is exact.
        let pair_step = ((pair as u64 * 2) % STEPS_PER_PATTERN as u64) as usize;
        let (step, until) = if within <= 0.0 {
            (pair_step, 0.0)
        } else if within <= split {
            (pair_step + 1, (split - within) * base)
        } else {
            (pair_step + 2, (2.0 - within) * base)
        };
        self.current_step = step % STEPS_PER_PATTERN;
        // `tick` counts down before it fires: one more than the samples to
        // wait fires on the first tick at or after the step's time.
        self.samples_until_next = until + 1.0;
    }

    /// Advance clock by one sample. Returns `Some(step_index)` if a
    /// new step should trigger this sample.
    pub fn tick(&mut self) -> Option<usize> {
        self.samples_until_next -= 1.0;
        if self.samples_until_next <= 0.0 {
            let step = self.current_step;
            // The step just fired lasts until the next one: long for the
            // first of a swing pair, short for the second.
            self.samples_until_next += self.step_duration_samples(step);
            self.current_step = (step + 1) % STEPS_PER_PATTERN;
            Some(step)
        } else {
            None
        }
    }

    /// Samples in one unswung step at the current tempo.
    fn base_step_samples(&self) -> f64 {
        self.sample_rate * 60.0 / (self.bpm * self.division.steps_per_beat())
    }

    /// The duration in samples from `step` to the step after it.
    ///
    /// Swing works by adjusting the timing of step pairs. For each pair
    /// of steps (0-1, 2-3, 4-5, ...), the total duration equals two
    /// base step periods. The swing ratio determines how that duration
    /// is split: at 50% both are equal (straight), at 66% the first step
    /// gets 2/3 and the second 1/3 (triplet feel).
    fn step_duration_samples(&self, step: usize) -> f64 {
        let pair_duration = self.base_step_samples() * 2.0;
        let swing_ratio = self.swing / 100.0;

        if step % 2 == 0 {
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

        for total_samples in 0..100_000_u64 {
            if let Some(step) = clock.tick() {
                step_positions.push((step, total_samples));
            }
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
    fn non_finite_tempo_and_swing_are_ignored() {
        let mut clock = SequencerClock::new(44100.0);
        clock.set_bpm(133.0);
        clock.set_swing(60.0);
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            clock.set_bpm(bad);
            clock.set_swing(bad);
        }
        assert!((clock.bpm() - 133.0).abs() < f64::EPSILON);
        assert!((clock.swing() - 60.0).abs() < f64::EPSILON);

        // The clock still fires steps afterwards.
        clock.reset();
        let fired = (0..20_000).filter_map(|_| clock.tick()).count();
        assert!(fired >= 3, "clock must keep running, fired {fired}");
    }

    #[test]
    fn clock_swing_alters_timing() {
        let mut clock = SequencerClock::new(44100.0);
        clock.set_bpm(120.0);
        clock.set_swing(66.6);

        let mut step_positions = Vec::new();

        for total_samples in 0..50_000_u64 {
            if let Some(step) = clock.tick() {
                step_positions.push((step, total_samples));
            }
        }

        assert!(
            step_positions.len() >= 3,
            "need at least 3 steps to compare swing intervals, got {}",
            step_positions.len()
        );
        assert_eq!(step_positions[0].0, 0);
        let int_0_1 = (step_positions[1].1 - step_positions[0].1) as f64;
        let int_1_2 = (step_positions[2].1 - step_positions[1].1) as f64;
        // Swing delays the off-beat: the on-beat step lasts longer.
        assert!(
            int_0_1 > int_1_2 + 100.0,
            "swing should delay the off-beat: {int_0_1} vs {int_1_2}"
        );
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

    /// Samples at which steps fire over `samples` ticks.
    fn fires(clock: &mut SequencerClock, samples: u64) -> Vec<(usize, u64)> {
        (0..samples)
            .filter_map(|sample| clock.tick().map(|step| (step, sample)))
            .collect()
    }

    #[test]
    fn the_downbeat_fires_on_the_first_tick() {
        let mut clock = SequencerClock::new(48_000.0);
        clock.set_bpm(120.0);
        clock.reset();
        // 120 BPM sixteenths at 48 kHz: 6 000 samples apart.
        assert_eq!(
            fires(&mut clock, 13_000),
            vec![(0, 0), (1, 6_000), (2, 12_000)]
        );
    }

    #[test]
    fn seeking_lands_on_the_grid_a_clock_from_beat_zero_keeps() {
        for swing in [50.0, 58.0, 66.6] {
            let mut from_start = SequencerClock::new(48_000.0);
            from_start.set_bpm(97.0);
            from_start.set_swing(swing);
            from_start.reset();
            let whole = fires(&mut from_start, 200_000);

            // Join at an arbitrary position part-way through.
            let join_at = 77_777_u64;
            let mut joined = SequencerClock::new(48_000.0);
            joined.set_bpm(97.0);
            joined.set_swing(swing);
            joined.seek(join_at as f64 * 97.0 / (60.0 * 48_000.0));
            let later: Vec<(usize, u64)> = fires(&mut joined, 200_000 - join_at)
                .into_iter()
                .map(|(step, sample)| (step, sample + join_at))
                .collect();
            let expected: Vec<(usize, u64)> = whole
                .into_iter()
                .filter(|(_, sample)| *sample >= join_at)
                .collect();
            assert_eq!(later, expected, "swing {swing}");
        }
    }

    #[test]
    fn a_step_exactly_on_the_position_fires_at_once() {
        let mut clock = SequencerClock::new(48_000.0);
        clock.set_bpm(120.0);
        clock.seek(1.0);
        assert_eq!(clock.tick(), Some(4));
        clock.seek(f64::NAN);
        assert_eq!(clock.tick(), Some(0));
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
