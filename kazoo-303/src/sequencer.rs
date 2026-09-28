//! 16-step acid bassline sequencer on the song's beat grid.

use kazoo_core::ipc::follow::beats_in;

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
    /// MIDI note, or -1 for a rest.
    pub note: i8,
    pub accent: bool,
    pub slide: bool,
}

/// What one sample of the sequencer asks of the synth, in order: close the
/// gate first, then play the step.
#[derive(Debug, Clone, Copy, Default)]
pub struct SequencerTick {
    /// The sounding note's gate closes on this sample.
    pub gate_off: bool,
    /// A step lands on this sample.
    pub trigger: Option<TriggerEvent>,
}

/// Fraction of a step the gate stays open, unless the next step slides.
pub const GATE_FRACTION: f64 = 0.5;

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

    /// Play from the top of the song.
    pub fn play(&mut self) {
        self.play_from(0.0);
    }

    /// Play from song position `beat`, on the grid a sequencer started on
    /// beat 0 keeps.
    pub fn play_from(&mut self, beat: f64) {
        self.clock.seek(beat);
        self.playing = true;
    }

    /// Stop and return to the top. The caller releases the sounding note.
    pub fn stop(&mut self) {
        self.playing = false;
        self.clock.clear_gate();
        self.clock.seek(0.0);
    }

    #[must_use]
    pub const fn is_playing(&self) -> bool {
        self.playing
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
            step_data.note = step_data
                .note
                .saturating_add(semitones)
                .clamp(MIN_NOTE, MAX_NOTE);
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

    /// Advance one sample. Real-time safe.
    pub fn tick(&mut self) -> SequencerTick {
        if !self.playing {
            return SequencerTick::default();
        }
        let clock = self.clock.tick();
        let mut out = SequencerTick {
            gate_off: clock.gate_off,
            trigger: None,
        };
        let Some(song_step) = clock.step else {
            return out;
        };
        // Always below STEPS_PER_PATTERN, so the cast is lossless.
        let step_index = (song_step % STEPS_PER_PATTERN as u64) as usize;
        let step = self.pattern.steps[step_index];
        if step.active {
            let next = self.pattern.steps[(step_index + 1) % STEPS_PER_PATTERN];
            if next.active && next.slide {
                // Tied: the gate stays open and the next step glides in.
                self.clock.clear_gate();
            } else {
                let start = self.clock.step_beat(song_step);
                let length = self.clock.step_beat(song_step + 1) - start;
                self.clock
                    .set_gate_off(GATE_FRACTION.mul_add(length, start));
            }
            out.trigger = Some(TriggerEvent {
                step_index,
                note: step.note,
                accent: step.accent,
                slide: step.slide,
            });
        } else {
            // A rest closes whatever was still sounding.
            self.clock.clear_gate();
            out.gate_off = true;
            out.trigger = Some(TriggerEvent {
                step_index,
                note: -1,
                accent: false,
                slide: false,
            });
        }
        out
    }
}

/// Sixteenth notes: steps in one beat.
const STEPS_PER_BEAT: f64 = 4.0;

/// Tolerance, in samples, for an event that falls exactly on a sample: float
/// rounding in the song position must not push it one sample late.
const EPSILON_SAMPLES: f64 = 1e-4;

/// What one clock sample produced.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClockTick {
    /// Song step (counted from beat 0) that lands on this sample.
    pub step: Option<u64>,
    /// The gate scheduled with [`SequencerClock::set_gate_off`] closes on
    /// this sample.
    pub gate_off: bool,
}

/// Sample-accurate sixteenth-note clock on the song's beat grid.
///
/// Every step has a fixed song position in beats (step `n` of a swing pair
/// `p = n / 2` sits at `2p` sixteenths, its off-beat `2 * swing` sixteenths
/// later), so the grid never depends on when playback started. The clock
/// converts those positions to samples from an anchor (the song position of
/// its tick 0) with one multiplication each: nothing accumulates, so it
/// cannot drift, and each event fires on the first sample at or after its
/// exact time.
#[derive(Debug)]
pub struct SequencerClock {
    sample_rate: f64,
    bpm: f64,
    /// Share of a swing pair the on-beat step lasts: 0.5 straight, up to
    /// 0.75 heavy swing (the off-beat is delayed).
    swing: f64,
    /// Song position, in beats, of tick 0.
    anchor_beat: f64,
    /// Samples ticked since the anchor.
    ticks: u64,
    /// Song step that fires next.
    next_step: u64,
    /// Tick it fires on.
    next_fire: u64,
    /// Song position the gate closes at, when one is open.
    gate_beat: Option<f64>,
    /// Tick the gate closes on.
    gate_fire: u64,
}

impl SequencerClock {
    pub const DEFAULT_BPM: f64 = 132.0;
    /// Slowest tempo: the desk's range.
    pub const MIN_BPM: f64 = 20.0;
    /// Fastest tempo: the desk's range.
    pub const MAX_BPM: f64 = 300.0;
    /// Straight time.
    pub const MIN_SWING: f64 = 0.5;
    /// Heaviest swing.
    pub const MAX_SWING: f64 = 0.75;

    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let sample_rate = if sample_rate.is_finite() {
            f64::from(sample_rate.max(1.0))
        } else {
            1.0
        };
        let mut clock = Self {
            sample_rate,
            bpm: Self::DEFAULT_BPM,
            swing: Self::MIN_SWING,
            anchor_beat: 0.0,
            ticks: 0,
            next_step: 0,
            next_fire: 0,
            gate_beat: None,
            gate_fire: 0,
        };
        clock.seek(0.0);
        clock
    }

    /// Set the tempo, clamped to the desk's range. A non-finite value (for
    /// example a corrupt message) is ignored. Mid-play, the clock carries on
    /// from where it is: the next step stays next, at the new tempo.
    pub fn set_bpm(&mut self, bpm: f64) {
        if !bpm.is_finite() {
            return;
        }
        let bpm = bpm.clamp(Self::MIN_BPM, Self::MAX_BPM);
        if bpm.to_bits() == self.bpm.to_bits() {
            return;
        }
        let position = self.position();
        self.bpm = bpm;
        self.reanchor(position);
    }

    #[must_use]
    pub const fn bpm(&self) -> f64 {
        self.bpm
    }

    /// Set swing (0.5 straight to 0.75 heavy). A non-finite value is ignored.
    /// The next step is found again on the new grid.
    pub fn set_swing(&mut self, swing: f64) {
        if !swing.is_finite() {
            return;
        }
        let position = self.position();
        self.swing = swing.clamp(Self::MIN_SWING, Self::MAX_SWING);
        self.next_step = self.first_step_at_or_after(position);
        self.reanchor(position);
    }

    #[must_use]
    pub const fn swing(&self) -> f64 {
        self.swing
    }

    /// Song position of the next sample, in beats.
    #[must_use]
    pub fn position(&self) -> f64 {
        // Tick counts stay far below 2^53: exact as f64.
        self.anchor_beat + beats_in(self.ticks as f64, self.bpm, self.sample_rate)
    }

    /// Move to song position `beat`: the clock is then where a clock started
    /// on beat 0 at this tempo and swing would be. A step exactly on `beat`
    /// fires on the next tick. A non-finite or negative position counts as
    /// beat 0. An open gate that the jump leaves behind closes at once.
    pub fn seek(&mut self, beat: f64) {
        let beat = if beat.is_finite() { beat.max(0.0) } else { 0.0 };
        self.next_step = self.first_step_at_or_after(beat);
        if let Some(gate) = self.gate_beat {
            // No gate lasts longer than a step; one outside that reach
            // belongs to where the song was, not where it is.
            if gate < beat || gate > beat + 2.0 / STEPS_PER_BEAT {
                self.gate_beat = Some(beat);
            }
        }
        self.reanchor(beat);
    }

    /// Song position of song step `step`, in beats.
    #[must_use]
    pub fn step_beat(&self, step: u64) -> f64 {
        // Song steps stay far below 2^53: exact as f64.
        let pair_start = (step / 2 * 2) as f64;
        let offbeat = if step % 2 == 1 { 2.0 * self.swing } else { 0.0 };
        (pair_start + offbeat) / STEPS_PER_BEAT
    }

    /// Close the gate at song position `beat`.
    pub fn set_gate_off(&mut self, beat: f64) {
        self.gate_beat = Some(beat);
        self.gate_fire = self.fire_tick(beat);
    }

    /// Forget any scheduled gate close.
    pub const fn clear_gate(&mut self) {
        self.gate_beat = None;
    }

    /// Advance one sample. Real-time safe: integer compares only.
    pub fn tick(&mut self) -> ClockTick {
        let now = self.ticks;
        let mut out = ClockTick::default();
        if self.gate_beat.is_some() && self.gate_fire <= now {
            self.gate_beat = None;
            out.gate_off = true;
        }
        if self.next_fire <= now {
            out.step = Some(self.next_step);
            self.next_step += 1;
            self.next_fire = self.fire_tick(self.step_beat(self.next_step));
        }
        self.ticks += 1;
        out
    }

    fn samples_per_beat(&self) -> f64 {
        60.0 * self.sample_rate / self.bpm
    }

    /// Make `beat` tick 0 and place the pending step and gate on it.
    fn reanchor(&mut self, beat: f64) {
        self.anchor_beat = beat;
        self.ticks = 0;
        self.next_fire = self.fire_tick(self.step_beat(self.next_step));
        if let Some(gate) = self.gate_beat {
            self.gate_fire = self.fire_tick(gate);
        }
    }

    /// First tick at or after song position `beat`.
    fn fire_tick(&self, beat: f64) -> u64 {
        let samples = (beat - self.anchor_beat).mul_add(self.samples_per_beat(), -EPSILON_SAMPLES);
        if samples > 0.0 {
            samples.ceil() as u64
        } else {
            0
        }
    }

    /// The first song step at or after `beat`.
    fn first_step_at_or_after(&self, beat: f64) -> u64 {
        let tolerance = EPSILON_SAMPLES / self.samples_per_beat();
        // Song positions are finite and non-negative here.
        let pair = (beat * STEPS_PER_BEAT / 2.0).floor() as u64;
        let first = pair * 2;
        if self.step_beat(first) >= beat - tolerance {
            first
        } else if self.step_beat(first + 1) >= beat - tolerance {
            first + 1
        } else {
            first + 2
        }
    }
}

#[must_use]
pub fn note_name(note: i8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B",
    ];
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
    /// Samples at which steps fire over `samples` ticks.
    fn fires(clock: &mut SequencerClock, samples: u64) -> Vec<(u64, u64)> {
        (0..samples)
            .filter_map(|sample| clock.tick().step.map(|step| (step, sample)))
            .collect()
    }

    #[test]
    fn the_downbeat_fires_on_the_first_tick() {
        let mut clock = SequencerClock::new(48_000.0);
        clock.set_bpm(120.0);
        clock.seek(0.0);
        // 120 BPM sixteenths at 48 kHz: 6 000 samples apart.
        assert_eq!(
            fires(&mut clock, 13_000),
            vec![(0, 0), (1, 6_000), (2, 12_000)]
        );
    }

    #[test]
    fn seeking_lands_on_the_grid_a_clock_from_beat_zero_keeps() {
        for swing in [0.5, 0.58, 0.666, 0.75] {
            let mut from_start = SequencerClock::new(48_000.0);
            from_start.set_bpm(97.0);
            from_start.set_swing(swing);
            from_start.seek(0.0);
            let whole = fires(&mut from_start, 300_000);

            for join_at in [1_u64, 7_421, 77_777, 150_001] {
                let mut joined = SequencerClock::new(48_000.0);
                joined.set_bpm(97.0);
                joined.set_swing(swing);
                joined.seek(beats_in(join_at as f64, 97.0, 48_000.0));
                let later: Vec<(u64, u64)> = fires(&mut joined, 300_000 - join_at)
                    .into_iter()
                    .map(|(step, sample)| (step, sample + join_at))
                    .collect();
                let expected: Vec<(u64, u64)> = whole
                    .iter()
                    .copied()
                    .filter(|(_, sample)| *sample >= join_at)
                    .collect();
                assert_eq!(later, expected, "swing {swing}, joined at {join_at}");
            }
        }
    }

    #[test]
    fn swing_delays_the_off_beat_and_keeps_the_pair() {
        let mut clock = SequencerClock::new(48_000.0);
        clock.set_bpm(120.0);
        clock.set_swing(0.75);
        clock.seek(0.0);
        // A pair is 12 000 samples: the off-beat lands three quarters in.
        assert_eq!(
            fires(&mut clock, 24_001),
            vec![(0, 0), (1, 9_000), (2, 12_000), (3, 21_000), (4, 24_000)]
        );
    }

    #[test]
    fn no_drift_over_thousands_of_steps() {
        // 97 BPM at 48 kHz: a sixteenth is 720 000 / 97 samples, a fraction
        // that never comes out even. Step n is due at exactly n * 720 000 / 97
        // samples, and fires on the first whole sample at or after it.
        let mut clock = SequencerClock::new(48_000.0);
        clock.set_bpm(97.0);
        clock.seek(0.0);
        let steps = 3_000_u64;
        let samples = (steps * 720_000).div_ceil(97) + 1;
        let fired = fires(&mut clock, samples);
        assert_eq!(fired.len() as u64, steps + 1);
        for (step, sample) in fired {
            let exact_ceiling = (step * 720_000).div_ceil(97);
            assert_eq!(sample, exact_ceiling, "step {step}");
        }
    }

    #[test]
    fn half_sample_steps_alternate_without_rounding_up() {
        // The old clock rounded 5 512.5 up to 5 513 every step and fell off
        // the grid; step 1 000 is due at exactly 5 512 500.
        let mut clock = SequencerClock::new(44_100.0);
        clock.set_bpm(120.0);
        clock.seek(0.0);
        let fired = fires(&mut clock, 5_512_501);
        assert_eq!(fired.last(), Some(&(1_000, 5_512_500)));
        assert_eq!(fired[1], (1, 5_513));
        assert_eq!(fired[2], (2, 11_025));
    }

    #[test]
    fn a_tempo_change_keeps_the_next_step_and_its_beat() {
        let mut clock = SequencerClock::new(48_000.0);
        clock.set_bpm(120.0);
        clock.seek(0.0);
        // Two steps (0 and 1), then half-way to step 2 (beat 0.5).
        assert_eq!(fires(&mut clock, 9_000).len(), 2);
        assert!((clock.position() - 0.375).abs() < 1e-12);
        clock.set_bpm(60.0);
        // Step 2 is at beat 0.5: 0.125 beats away, 6 000 samples at 60 BPM.
        let after = fires(&mut clock, 18_001);
        assert_eq!(after, vec![(2, 6_000), (3, 18_000)]);
    }

    #[test]
    fn a_step_exactly_on_the_position_fires_at_once() {
        let mut clock = SequencerClock::new(48_000.0);
        clock.set_bpm(120.0);
        clock.seek(1.0);
        assert_eq!(clock.tick().step, Some(4));
        clock.seek(f64::NAN);
        assert_eq!(clock.tick().step, Some(0));
        clock.seek(-3.0);
        assert_eq!(clock.tick().step, Some(0));
    }

    #[test]
    fn non_finite_tempo_and_swing_are_ignored_and_tempo_is_the_desks_range() {
        let mut clock = SequencerClock::new(48_000.0);
        clock.set_bpm(133.0);
        clock.set_swing(0.6);
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            clock.set_bpm(bad);
            clock.set_swing(bad);
        }
        assert!((clock.bpm() - 133.0).abs() < f64::EPSILON);
        assert!((clock.swing() - 0.6).abs() < f64::EPSILON);
        clock.set_bpm(5.0);
        assert!((clock.bpm() - 20.0).abs() < f64::EPSILON);
        clock.set_bpm(900.0);
        assert!((clock.bpm() - 300.0).abs() < f64::EPSILON);
    }

    /// Samples at which the sequencer triggers and closes gates.
    fn sequence(seq: &mut Sequencer, samples: u64) -> (Vec<(usize, u64)>, Vec<u64>) {
        let mut triggers = Vec::new();
        let mut gates = Vec::new();
        for sample in 0..samples {
            let tick = seq.tick();
            if tick.gate_off {
                gates.push(sample);
            }
            if let Some(trigger) = tick.trigger {
                triggers.push((trigger.step_index, sample));
            }
        }
        (triggers, gates)
    }

    fn one_note_pattern(seq: &mut Sequencer) {
        for step in 0..STEPS_PER_PATTERN {
            if seq.current_pattern().steps[step].active {
                seq.toggle_step(step);
            }
        }
        seq.toggle_step(0);
    }

    #[test]
    fn a_note_gates_for_half_its_step() {
        let mut seq = Sequencer::new(48_000.0);
        seq.clock.set_bpm(120.0);
        one_note_pattern(&mut seq);
        seq.play();
        let (triggers, gates) = sequence(&mut seq, 6_001);
        assert_eq!(triggers, vec![(0, 0), (1, 6_000)]);
        // The gate closes at 3 000, and the rest on step 1 closes it again.
        assert_eq!(gates, vec![3_000, 6_000]);
    }

    #[test]
    fn a_slide_into_the_next_step_ties_the_gate() {
        let mut seq = Sequencer::new(48_000.0);
        seq.clock.set_bpm(120.0);
        one_note_pattern(&mut seq);
        seq.toggle_step(1);
        seq.toggle_slide(1);
        seq.play();
        let (triggers, gates) = sequence(&mut seq, 12_001);
        assert_eq!(triggers, vec![(0, 0), (1, 6_000), (2, 12_000)]);
        // No gate close at 3 000: step 0 is tied into step 1, whose own
        // gate closes half-way through it.
        assert_eq!(gates, vec![9_000, 12_000]);
    }

    #[test]
    fn the_sequencer_keeps_the_bar_across_the_pattern_and_stops_at_the_top() {
        let mut seq = Sequencer::new(48_000.0);
        seq.clock.set_bpm(120.0);
        // Join in the second bar, on beat 5 (step 20 of the song: pattern
        // step 4).
        seq.play_from(5.0);
        let (triggers, _) = sequence(&mut seq, 1);
        assert_eq!(triggers, vec![(4, 0)]);
        seq.stop();
        assert!(!seq.is_playing());
        assert!(seq.tick().trigger.is_none(), "stopped plays nothing");
        seq.play();
        let (triggers, _) = sequence(&mut seq, 1);
        assert_eq!(triggers, vec![(0, 0)]);
    }

    #[test]
    fn a_seek_away_from_an_open_gate_closes_it() {
        let mut seq = Sequencer::new(48_000.0);
        seq.clock.set_bpm(120.0);
        one_note_pattern(&mut seq);
        seq.play();
        let (triggers, _) = sequence(&mut seq, 1);
        assert_eq!(triggers, vec![(0, 0)]);
        // Relocated far ahead, mid-step: the old note's gate closes now.
        seq.play_from(10.1);
        let tick = seq.tick();
        assert!(tick.gate_off);
    }
}
