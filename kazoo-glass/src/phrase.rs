//! Looping `kazoo-play` notation phrase, placed on the studio's song clock
//! and driven sample-accurately from the audio callback.
//!
//! Every event is held in beats from the start of the phrase, and the phrase
//! is aligned to song beat 0: at song position `b` it is at `b` modulo its
//! length. The clock keeps an anchor (the song beat at its tick 0) and counts
//! whole ticks from it; each event's tick is computed from the anchor in one
//! step, so nothing accumulates and the loop never drifts off the grid. A
//! tempo change re-anchors at the current position.
//!
//! Parsing and allocation happen once in [`Phrase::parse`] on the UI thread.
//! [`Phrase::advance`] is called once per sample and never allocates.

use kazoo_core::ipc::follow::beats_in;
use kazoo_core::notation::{DEFAULT_NOTATION_VELOCITY, NotationError, parse_notation};

/// Slowest tempo the phrase follows, in beats per minute (the desk's range).
pub const MIN_BPM: f64 = 20.0;
/// Fastest tempo the phrase follows, in beats per minute (the desk's range).
pub const MAX_BPM: f64 = 300.0;

/// Tolerance, in samples, for an event that lands on a whole tick: float
/// rounding in the anchor must not push it a tick late.
const EPSILON_SAMPLES: f64 = 1e-4;

/// Largest gap, in samples, between the phrase's own position and a position
/// the desk hands it for it still to count as the same place (a tempo change,
/// not a jump).
const SAME_PLACE_SAMPLES: f64 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Step {
    /// Beats from the start of the phrase.
    beat: f64,
    note: u8,
    /// MIDI velocity for note-on; `None` for note-off.
    velocity: Option<u8>,
}

/// A parsed phrase that loops forever on the song clock.
#[derive(Debug)]
pub struct Phrase {
    /// Sorted by beat; at the same beat, note-offs come first so a repeated
    /// note retriggers.
    steps: Vec<Step>,
    /// Loop length in beats, trailing rests included. Always positive.
    length: f64,
    sample_rate: f64,
    bpm: f64,
    playing: bool,
    /// Song beat at tick 0.
    anchor_beat: f64,
    /// Ticks rendered since the anchor.
    ticks: u64,
    /// Loop the next event is in (loop `n` starts on song beat `n * length`).
    lap: u64,
    /// Index of the next event in `steps`.
    cursor: usize,
    /// Tick the next event fires on.
    fire_tick: u64,
}

impl Phrase {
    /// Parse notation to play at `bpm` (until the desk says otherwise) on a
    /// stream at `sample_rate`.
    ///
    /// # Errors
    ///
    /// The notation's own error, or an error for text with no notes or rests
    /// (a phrase with no length cannot loop).
    pub fn parse(text: &str, bpm: f64, sample_rate: u32) -> Result<Self, NotationError> {
        let events = parse_notation(text)?;
        let velocity = velocity_to_midi(DEFAULT_NOTATION_VELOCITY);
        let mut steps = Vec::new();
        for event in &events {
            let end = event.start_beats + event.duration_beats;
            for &note in &event.notes {
                steps.push(Step {
                    beat: event.start_beats,
                    note,
                    velocity: Some(velocity),
                });
                steps.push(Step {
                    beat: end,
                    note,
                    velocity: None,
                });
            }
        }
        // Stable: chord notes keep their written order.
        steps.sort_by(|a, b| {
            a.beat
                .total_cmp(&b.beat)
                .then_with(|| a.velocity.is_some().cmp(&b.velocity.is_some()))
        });

        // Loop length comes from the notation itself, so trailing rests count.
        let length = events
            .iter()
            .map(|e| e.start_beats + e.duration_beats)
            .fold(0.0_f64, f64::max);
        if !(length.is_finite() && length > 0.0) {
            return Err(NotationError {
                token_index: 0,
                message: "a phrase needs at least one note or rest".to_owned(),
            });
        }

        let mut phrase = Self {
            steps,
            length,
            sample_rate: f64::from(sample_rate.max(1)),
            bpm: clamp_bpm(bpm).unwrap_or(120.0),
            playing: false,
            anchor_beat: 0.0,
            ticks: 0,
            lap: 0,
            cursor: 0,
            fire_tick: 0,
        };
        phrase.seek(0.0);
        Ok(phrase)
    }

    /// Tempo in beats per minute.
    #[must_use]
    pub const fn bpm(&self) -> f64 {
        self.bpm
    }

    /// Whether the phrase is looping.
    #[must_use]
    pub const fn is_playing(&self) -> bool {
        self.playing
    }

    /// Loop length in beats.
    #[cfg(test)]
    #[must_use]
    pub const fn length_beats(&self) -> f64 {
        self.length
    }

    /// Song position of the next tick, in beats.
    #[must_use]
    pub fn position(&self) -> f64 {
        // Tick counts stay far below 2^53: exact as f64.
        self.anchor_beat + beats_in(self.ticks as f64, self.bpm, self.sample_rate)
    }

    /// Follow a new tempo, clamped to [`MIN_BPM`]..=[`MAX_BPM`]. A tempo
    /// that is not a positive number is ignored. The phrase keeps its place:
    /// it re-anchors at its current position.
    pub fn set_bpm(&mut self, bpm: f64) {
        let Some(bpm) = clamp_bpm(bpm) else {
            return;
        };
        if bpm.to_bits() == self.bpm.to_bits() {
            return;
        }
        let here = self.position();
        self.bpm = bpm;
        self.anchor_beat = here;
        self.ticks = 0;
        self.fire_tick = self.tick_of_next();
    }

    /// Play from song position `beat` (a non-finite or negative position
    /// counts as beat 0). An event exactly on `beat` fires on the next tick.
    ///
    /// Returns `true` when the phrase jumped (it was stopped, or `beat` is
    /// not where it already was): the caller then releases the phrase's
    /// sounding voices. When it was already playing at that position (a
    /// tempo change) it only re-anchors there, and notes ring on.
    pub fn play_from(&mut self, beat: f64) -> bool {
        let beat = if beat.is_finite() { beat.max(0.0) } else { 0.0 };
        let samples_per_beat = self.samples_per_beat();
        let same_place = self.playing
            && ((self.position() - beat) * samples_per_beat).abs() < SAME_PLACE_SAMPLES;
        self.playing = true;
        if same_place {
            self.anchor_beat = beat;
            self.ticks = 0;
            self.fire_tick = self.tick_of_next();
            false
        } else {
            self.seek(beat);
            true
        }
    }

    /// Stop. The caller releases the phrase's sounding voices.
    pub const fn stop(&mut self) {
        self.playing = false;
    }

    /// Fire every event due on this tick through `fire(note, velocity)`
    /// (`None` velocity is a note-off), then move one tick on. Does nothing
    /// while stopped. Never allocates.
    pub fn advance(&mut self, mut fire: impl FnMut(u8, Option<u8>)) {
        if !self.playing {
            return;
        }
        // At most one whole loop per tick, however short the phrase.
        for _ in 0..self.steps.len() {
            if self.fire_tick > self.ticks {
                break;
            }
            let step = self.steps[self.cursor];
            fire(step.note, step.velocity);
            self.cursor += 1;
            if self.cursor == self.steps.len() {
                self.cursor = 0;
                self.lap += 1;
            }
            self.fire_tick = self.tick_of_next();
        }
        self.ticks += 1;
    }

    /// Place the phrase at song position `beat` without changing whether it
    /// plays.
    fn seek(&mut self, beat: f64) {
        self.anchor_beat = beat;
        self.ticks = 0;
        // Song positions are small and non-negative: the conversion is exact.
        let lap = (beat / self.length).floor().max(0.0);
        self.lap = lap as u64;
        let within = beat - lap * self.length;
        let tolerance = EPSILON_SAMPLES / self.samples_per_beat();
        if let Some(index) = self
            .steps
            .iter()
            .position(|step| step.beat >= within - tolerance)
        {
            self.cursor = index;
        } else {
            // Past this loop's last event: the next is the next loop's first.
            self.cursor = 0;
            self.lap += 1;
        }
        self.fire_tick = self.tick_of_next();
    }

    fn samples_per_beat(&self) -> f64 {
        60.0 * self.sample_rate / self.bpm
    }

    /// The tick the next event lands on: the first tick at or after its
    /// song position. A phrase of rests never fires.
    fn tick_of_next(&self) -> u64 {
        let Some(step) = self.steps.get(self.cursor) else {
            return u64::MAX;
        };
        let beat = (self.lap as f64).mul_add(self.length, step.beat);
        let samples = (beat - self.anchor_beat).mul_add(self.samples_per_beat(), -EPSILON_SAMPLES);
        if samples > 0.0 {
            // Saturates for absurd positions rather than wrapping.
            samples.ceil() as u64
        } else {
            0
        }
    }
}

/// A usable tempo, or `None` for one that is not a positive number.
fn clamp_bpm(bpm: f64) -> Option<f64> {
    (bpm.is_finite() && bpm > 0.0).then(|| bpm.clamp(MIN_BPM, MAX_BPM))
}

fn velocity_to_midi(velocity: f32) -> u8 {
    if velocity.is_finite() {
        let v = (velocity.clamp(0.0, 1.0) * 127.0).round() as u8;
        v.max(1)
    } else {
        100
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    /// (tick, note, velocity) for every event over `ticks` ticks.
    fn run(phrase: &mut Phrase, ticks: u64) -> Vec<(u64, u8, Option<u8>)> {
        let mut out = Vec::new();
        for tick in 0..ticks {
            phrase.advance(|note, velocity| out.push((tick, note, velocity)));
        }
        out
    }

    #[test]
    fn events_land_on_the_exact_grid_with_no_drift() {
        // Eighths at 97 BPM, 48 kHz: 1 440 000 / 97 samples a half beat,
        // never a whole number, so a rounding clock would drift.
        let mut phrase = Phrase::parse("c4/8 e4/8 g4/4", 97.0, RATE).unwrap();
        assert!(phrase.play_from(0.0));
        let ticks = 6_000_000; // about 100 loops of two beats
        let ons: Vec<(u64, u8)> = run(&mut phrase, ticks)
            .into_iter()
            .filter(|(_, _, velocity)| velocity.is_some())
            .map(|(tick, note, _)| (tick, note))
            .collect();
        // Independently, in integers: note-ons at half-beats 0, 1, 2 of
        // every four, on the first sample at or after each.
        let mut expected = Vec::new();
        for half in 0_u64.. {
            let within = half % 4;
            if within == 3 {
                continue;
            }
            let tick = (half * 1_440_000).div_ceil(97);
            if tick >= ticks {
                break;
            }
            expected.push((tick, [60, 64, 67][within as usize]));
        }
        assert!(expected.len() > 250);
        assert_eq!(ons, expected);
    }

    #[test]
    fn a_repeated_note_is_released_before_it_retriggers() {
        let mut phrase = Phrase::parse("c4/4 c4/4", 120.0, RATE).unwrap();
        phrase.play_from(0.0);
        let events = run(&mut phrase, 24_001);
        assert_eq!(
            events,
            vec![
                (0, 60, Some(102)),
                (24_000, 60, None),
                (24_000, 60, Some(102))
            ]
        );
    }

    #[test]
    fn seeking_lands_on_the_grid_a_run_from_beat_zero_keeps() {
        for text in ["c4/8 e4/8 g4/4", "[c4 e4]/8. r/16 g4/4 r/2"] {
            let mut whole = Phrase::parse(text, 97.0, RATE).unwrap();
            whole.play_from(0.0);
            let all = run(&mut whole, 400_000);

            let join_at = 177_777_u64;
            let mut joined = Phrase::parse(text, 97.0, RATE).unwrap();
            joined.play_from(beats_in(join_at as f64, 97.0, f64::from(RATE)));
            let later: Vec<_> = run(&mut joined, 400_000 - join_at)
                .into_iter()
                .map(|(tick, note, velocity)| (tick + join_at, note, velocity))
                .collect();
            let expected: Vec<_> = all
                .into_iter()
                .filter(|(tick, _, _)| *tick >= join_at)
                .collect();
            assert!(!expected.is_empty());
            assert_eq!(later, expected, "{text}");
        }
    }

    #[test]
    fn an_event_exactly_on_the_position_fires_at_once() {
        let mut phrase = Phrase::parse("c4/4 e4/4", 120.0, RATE).unwrap();
        // Beat 5 is beat 1 of the third loop: e4.
        phrase.play_from(5.0);
        let events = run(&mut phrase, 1);
        assert_eq!(events, vec![(0, 60, None), (0, 64, Some(102))]);
    }

    #[test]
    fn a_tempo_change_keeps_the_place_and_the_grid() {
        let text = "c4/8 e4/8 g4/4 r/8";
        let mut phrase = Phrase::parse(text, 120.0, RATE).unwrap();
        phrase.play_from(0.0);
        let change_at = 70_001_u64;
        let before = run(&mut phrase, change_at);
        let here = phrase.position();
        phrase.set_bpm(97.0);
        let after: Vec<_> = run(&mut phrase, 300_000)
            .into_iter()
            .map(|(tick, note, velocity)| (tick + change_at, note, velocity))
            .collect();

        // The same as a phrase at 97 BPM placed on that beat.
        let mut fresh = Phrase::parse(text, 97.0, RATE).unwrap();
        fresh.play_from(here);
        let expected: Vec<_> = run(&mut fresh, 300_000)
            .into_iter()
            .map(|(tick, note, velocity)| (tick + change_at, note, velocity))
            .collect();
        assert_eq!(after, expected);
        // Nothing skipped or played twice across the change.
        let notes: Vec<u8> = before
            .iter()
            .chain(&after)
            .filter(|(_, _, velocity)| velocity.is_some())
            .map(|&(_, note, _)| note)
            .take(9)
            .collect();
        assert_eq!(notes, [60, 64, 67, 60, 64, 67, 60, 64, 67]);
    }

    #[test]
    fn the_desk_handing_back_the_same_place_is_not_a_jump() {
        let mut phrase = Phrase::parse("c4/2", 120.0, RATE).unwrap();
        assert!(phrase.play_from(0.0));
        run(&mut phrase, 1_000);
        let here = phrase.position();
        phrase.set_bpm(140.0);
        assert!(
            !phrase.play_from(here),
            "a tempo change keeps notes ringing"
        );
        assert!(phrase.play_from(here + 1.0), "a real jump");
        phrase.stop();
        assert!(!phrase.is_playing());
        assert!(phrase.play_from(here + 1.0), "starting is always a jump");
    }

    #[test]
    fn stopped_phrases_are_silent() {
        let mut phrase = Phrase::parse("c4/4", 120.0, RATE).unwrap();
        assert!(run(&mut phrase, 1_000).is_empty());
    }

    #[test]
    fn hostile_tempo_and_positions_are_handled() {
        let mut phrase = Phrase::parse("c4/4", 120.0, RATE).unwrap();
        phrase.set_bpm(f64::NAN);
        phrase.set_bpm(0.0);
        phrase.set_bpm(-5.0);
        assert!((phrase.bpm() - 120.0).abs() < f64::EPSILON);
        phrase.set_bpm(10_000.0);
        assert!((phrase.bpm() - MAX_BPM).abs() < f64::EPSILON);
        phrase.play_from(f64::NAN);
        assert_eq!(run(&mut phrase, 1), vec![(0, 60, Some(102))]);
    }

    #[test]
    fn rests_extend_the_loop() {
        let short = Phrase::parse("c4/4", 120.0, RATE).unwrap();
        let long = Phrase::parse("c4/4 r/4 r/4", 120.0, RATE).unwrap();
        assert!((short.length_beats() - 1.0).abs() < f64::EPSILON);
        assert!((long.length_beats() - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn an_all_rest_phrase_loops_silently() {
        let mut phrase = Phrase::parse("r/4", 120.0, RATE).unwrap();
        phrase.play_from(0.0);
        assert!(run(&mut phrase, 100_000).is_empty());
    }

    #[test]
    fn bad_or_empty_notation_is_an_error() {
        assert!(Phrase::parse("not-a-note!!", 120.0, RATE).is_err());
        assert!(Phrase::parse("   ", 120.0, RATE).is_err());
    }

    #[test]
    fn velocity_mapping_is_safe() {
        assert_eq!(velocity_to_midi(f32::NAN), 100);
        assert_eq!(velocity_to_midi(0.0), 1);
        assert_eq!(velocity_to_midi(1.0), 127);
    }
}
