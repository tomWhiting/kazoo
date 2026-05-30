//! 16-step acid bassline sequencer.

/// Number of steps in one classic one-bar bassline pattern.
pub const STEPS_PER_PATTERN: usize = 16;
const MIN_NOTE: i8 = 24;
const MAX_NOTE: i8 = 72;

/// One 303-style sequencer step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub active: bool,
    pub note: i8,
    pub accent: bool,
    pub slide: bool,
}

impl Default for Step {
    fn default() -> Self {
        Self {
            active: false,
            note: 36,
            accent: false,
            slide: false,
        }
    }
}

impl Step {
    #[must_use]
    pub fn note_name(self) -> String {
        note_name(self.note)
    }
}

#[derive(Debug, Clone)]
pub struct Pattern {
    pub name: String,
    pub steps: [Step; STEPS_PER_PATTERN],
}

impl Default for Pattern {
    fn default() -> Self {
        let mut pattern = Self {
            name: String::from("ACID 1"),
            steps: [Step::default(); STEPS_PER_PATTERN],
        };
        for (idx, note) in [36, 36, 39, 43, 36, 46, 43, 39].into_iter().enumerate() {
            let step = idx * 2;
            pattern.steps[step] = Step {
                active: true,
                note,
                accent: matches!(step, 4 | 10),
                slide: matches!(step, 2 | 10),
            };
        }
        pattern
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TriggerEvent {
    pub step_index: usize,
    pub note: i8,
    pub accent: bool,
    pub slide: bool,
}

#[derive(Debug)]
pub struct Sequencer {
    pub clock: SequencerClock,
    pattern: Pattern,
    playing: bool,
}

impl Sequencer {
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        Self {
            clock: SequencerClock::new(sample_rate),
            pattern: Pattern::default(),
            playing: false,
        }
    }

    pub const fn play(&mut self) {
        self.playing = true;
    }

    pub fn stop(&mut self) {
        self.playing = false;
        self.clock.reset();
    }

    #[must_use]
    pub const fn current_pattern(&self) -> &Pattern {
        &self.pattern
    }

    pub fn toggle_step(&mut self, step: usize) {
        if let Some(step_data) = self.pattern.steps.get_mut(step) {
            step_data.active = !step_data.active;
            if !step_data.active {
                step_data.accent = false;
                step_data.slide = false;
            }
        }
    }

    pub fn toggle_accent(&mut self, step: usize) {
        if let Some(step_data) = self.pattern.steps.get_mut(step) {
            if step_data.active {
                step_data.accent = !step_data.accent;
            }
        }
    }

    pub fn toggle_slide(&mut self, step: usize) {
        if let Some(step_data) = self.pattern.steps.get_mut(step) {
            if step_data.active {
                step_data.slide = !step_data.slide;
            }
        }
    }

    pub fn transpose_step(&mut self, step: usize, semitones: i8) {
        if let Some(step_data) = self.pattern.steps.get_mut(step) {
            step_data.note = step_data.note.saturating_add(semitones).clamp(MIN_NOTE, MAX_NOTE);
            step_data.active = true;
        }
    }

    pub fn randomize_acid(&mut self) {
        // Deterministic pseudo-randomness: no RNG dependency and repeatable demos.
        let scale = [0, 3, 5, 7, 10, 12, 15, 17];
        for step in 0..STEPS_PER_PATTERN {
            let seed = step.wrapping_mul(73).wrapping_add(19);
            let active = step % 4 == 0 || seed % 5 < 3;
            let degree = scale[seed % scale.len()];
            self.pattern.steps[step] = Step {
                active,
                note: 36 + degree,
                accent: active && seed % 7 == 0,
                slide: active && step < STEPS_PER_PATTERN - 1 && seed % 4 == 0,
            };
        }
    }

    pub fn tick(&mut self) -> Option<TriggerEvent> {
        if !self.playing {
            return None;
        }
        let step_index = self.clock.tick()?;
        let step = self.pattern.steps[step_index];
        if step.active {
            Some(TriggerEvent {
                step_index,
                note: step.note,
                accent: step.accent,
                slide: step.slide,
            })
        } else {
            Some(TriggerEvent {
                step_index,
                note: -1,
                accent: false,
                slide: false,
            })
        }
    }
}

/// Sample-accurate sixteenth-note clock.
#[derive(Debug)]
pub struct SequencerClock {
    sample_rate: f64,
    bpm: f64,
    samples_until_next: f64,
    current_step: usize,
}

impl SequencerClock {
    pub const DEFAULT_BPM: f64 = 132.0;

    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let mut clock = Self {
            sample_rate: f64::from(sample_rate.max(1.0)),
            bpm: Self::DEFAULT_BPM,
            samples_until_next: 0.0,
            current_step: 0,
        };
        clock.samples_until_next = clock.step_duration_samples();
        clock
    }

    pub const fn set_bpm(&mut self, bpm: f64) {
        self.bpm = bpm.clamp(40.0, 240.0);
    }

    #[must_use]
    pub const fn bpm(&self) -> f64 {
        self.bpm
    }

    pub fn reset(&mut self) {
        self.current_step = 0;
        self.samples_until_next = self.step_duration_samples();
    }

    pub fn tick(&mut self) -> Option<usize> {
        self.samples_until_next -= 1.0;
        if self.samples_until_next <= 0.0 {
            let step = self.current_step;
            self.current_step = (self.current_step + 1) % STEPS_PER_PATTERN;
            self.samples_until_next += self.step_duration_samples();
            Some(step)
        } else {
            None
        }
    }

    fn step_duration_samples(&self) -> f64 {
        self.sample_rate * 60.0 / (self.bpm * 4.0)
    }
}

#[must_use]
pub fn note_name(note: i8) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];
    if note < 0 {
        return String::from("---");
    }
    let octave = i16::from(note) / 12 - 1;
    let name = NAMES[note.rem_euclid(12) as usize];
    format!("{name}{octave}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_pattern_has_no_samples_just_steps() {
        let seq = Sequencer::new(44_100.0);
        assert!(seq.current_pattern().steps.iter().any(|step| step.active));
    }

    #[test]
    fn transpose_activates_and_clamps() {
        let mut seq = Sequencer::new(44_100.0);
        seq.transpose_step(1, 100);
        assert!(seq.current_pattern().steps[1].active);
        assert_eq!(seq.current_pattern().steps[1].note, MAX_NOTE);
    }
}
