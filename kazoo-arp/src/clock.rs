//! Clock division, swing, and sample-accurate tick driver for the arpeggiator.

use crate::engine::{Arpeggiator, NoteEvent};

/// Clock division relative to a quarter note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClockDivision {
    /// Whole note (1 step per 4 beats).
    Whole,
    /// Half note (1 step per 2 beats).
    Half,
    /// Quarter note (1 step per beat).
    Quarter,
    /// Eighth note (2 steps per beat).
    Eighth,
    /// Sixteenth note (4 steps per beat).
    Sixteenth,
    /// Thirty-second note (8 steps per beat).
    ThirtySecond,
    /// Eighth-note triplet (3 steps per beat).
    EighthTriplet,
    /// Sixteenth-note triplet (6 steps per beat).
    SixteenthTriplet,
}

impl ClockDivision {
    /// All divisions in display order.
    pub const ALL: [Self; 8] = [
        Self::Whole,
        Self::Half,
        Self::Quarter,
        Self::Eighth,
        Self::Sixteenth,
        Self::ThirtySecond,
        Self::EighthTriplet,
        Self::SixteenthTriplet,
    ];

    /// Steps per beat (quarter note).
    #[must_use]
    pub const fn steps_per_beat(self) -> f32 {
        match self {
            Self::Whole => 0.25,
            Self::Half => 0.5,
            Self::Quarter => 1.0,
            Self::Eighth => 2.0,
            Self::Sixteenth => 4.0,
            Self::ThirtySecond => 8.0,
            Self::EighthTriplet => 3.0,
            Self::SixteenthTriplet => 6.0,
        }
    }

    /// Period in samples for this division at the given sample rate and BPM.
    #[must_use]
    pub fn period_samples(self, sample_rate: f32, bpm: f32) -> f32 {
        if !bpm.is_finite() || bpm <= 0.0 || !sample_rate.is_finite() || sample_rate <= 0.0 {
            return f32::MAX;
        }
        sample_rate * 60.0 / (bpm * self.steps_per_beat())
    }

    /// Human-readable label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Whole => "1/1",
            Self::Half => "1/2",
            Self::Quarter => "1/4",
            Self::Eighth => "1/8",
            Self::Sixteenth => "1/16",
            Self::ThirtySecond => "1/32",
            Self::EighthTriplet => "1/8T",
            Self::SixteenthTriplet => "1/16T",
        }
    }

    /// Next division (faster).
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Whole => Self::Half,
            Self::Half => Self::Quarter,
            Self::Quarter => Self::Eighth,
            Self::Eighth => Self::Sixteenth,
            Self::Sixteenth => Self::ThirtySecond,
            Self::ThirtySecond => Self::EighthTriplet,
            Self::EighthTriplet => Self::SixteenthTriplet,
            Self::SixteenthTriplet => Self::Whole,
        }
    }

    /// Previous division (slower).
    #[must_use]
    pub const fn prev(self) -> Self {
        match self {
            Self::Whole => Self::SixteenthTriplet,
            Self::Half => Self::Whole,
            Self::Quarter => Self::Half,
            Self::Eighth => Self::Quarter,
            Self::Sixteenth => Self::Eighth,
            Self::ThirtySecond => Self::Sixteenth,
            Self::EighthTriplet => Self::ThirtySecond,
            Self::SixteenthTriplet => Self::EighthTriplet,
        }
    }
}

/// Compute the swing-adjusted period for a given step.
///
/// Steps are grouped in pairs. The first step of each pair (even index: 0, 2, 4)
/// takes `swing` fraction of the pair duration. The second step (odd: 1, 3, 5)
/// takes `1 - swing`. At `swing = 0.5` both steps equal `base_period`.
///
/// The sum of any consecutive pair always equals `2 * base_period`, preserving
/// the overall tempo regardless of swing amount.
#[must_use]
pub fn swing_period(base_period: f32, swing: f32, step_index: u64) -> f32 {
    let s = swing.clamp(0.0, 1.0);
    let pair = base_period * 2.0;
    if step_index % 2 == 0 {
        pair * s
    } else {
        pair * (1.0 - s)
    }
}

/// Events returned by a single clock tick.
#[derive(Debug, Clone, Copy, Default)]
pub struct TickEvents {
    /// Note-off for the previously sounding note (if gate ended or new step started).
    pub note_off: Option<NoteEvent>,
    /// Note-on for the new step (if a step boundary was reached).
    pub note_on: Option<NoteEvent>,
}

impl TickEvents {
    /// Whether this tick produced any events.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.note_off.is_none() && self.note_on.is_none()
    }
}

/// Sample-accurate arpeggiator clock.
///
/// Call [`tick`](Self::tick) once per audio sample. It tracks the step period,
/// swing timing, and gate duration, emitting note-on and note-off events at
/// the correct sample boundaries.
///
/// O(1) per tick. No allocation.
#[derive(Debug, Clone)]
pub struct ArpClock {
    sample_counter: u64,
    step_counter: u64,
    needs_trigger: bool,
    gate_sent: bool,
    current_period: f32,
    gate_samples: f32,

    /// Sample rate in Hz.
    pub sample_rate: f32,
    /// Tempo in BPM.
    pub bpm: f32,
    /// Clock division.
    pub division: ClockDivision,
    /// Swing amount (0.0–1.0, 0.5 = straight).
    pub swing: f32,
    /// Whether the clock is running.
    pub running: bool,
}

impl ArpClock {
    /// Create a new clock with the given sample rate and BPM.
    #[must_use]
    pub fn new(sample_rate: f32, bpm: f32) -> Self {
        let division = ClockDivision::Sixteenth;
        let base = division.period_samples(sample_rate, bpm);
        Self {
            sample_counter: 0,
            step_counter: 0,
            needs_trigger: true,
            gate_sent: false,
            current_period: base,
            gate_samples: base * 0.75,
            sample_rate,
            bpm,
            division,
            swing: 0.5,
            running: false,
        }
    }

    /// Start the clock. The first tick will immediately trigger a step.
    pub const fn start(&mut self) {
        self.running = true;
        self.needs_trigger = true;
        self.sample_counter = 0;
        self.step_counter = 0;
    }

    /// Stop the clock.
    pub const fn stop(&mut self) {
        self.running = false;
    }

    /// Reset timing state without changing parameters.
    pub const fn reset(&mut self) {
        self.sample_counter = 0;
        self.step_counter = 0;
        self.needs_trigger = true;
        self.gate_sent = false;
    }

    /// Set BPM and recalculate the current period.
    pub fn set_bpm(&mut self, bpm: f32) {
        self.bpm = bpm.max(1.0);
        self.recalculate_period();
    }

    /// Set the clock division and recalculate the current period.
    pub fn set_division(&mut self, division: ClockDivision) {
        self.division = division;
        self.recalculate_period();
    }

    /// Set swing amount (clamped to 0.0–1.0).
    pub fn set_swing(&mut self, swing: f32) {
        self.swing = swing.clamp(0.0, 1.0);
        self.recalculate_period();
    }

    /// Process one audio sample. Returns any note events that should fire.
    ///
    /// O(1), no allocation.
    pub fn tick(&mut self, arp: &mut Arpeggiator) -> TickEvents {
        let mut events = TickEvents::default();

        if !self.running || !arp.has_notes() {
            return events;
        }

        let counter_f = self.sample_counter as f32;

        // Step trigger: first tick or period elapsed.
        if self.needs_trigger || counter_f >= self.current_period {
            self.needs_trigger = false;
            self.sample_counter = 0;

            // Gate-off for previous note.
            events.note_off = arp.gate_off();

            // New step.
            events.note_on = arp.step();

            // Recalculate period with swing for the new step.
            let base = self.division.period_samples(self.sample_rate, self.bpm);
            self.current_period = swing_period(base, self.swing, self.step_counter);
            self.gate_samples = self.current_period * arp.gate_pct;
            self.step_counter += 1;
            self.gate_sent = false;

            return events;
        }

        // Gate-off within the step.
        if !self.gate_sent && counter_f >= self.gate_samples {
            self.gate_sent = true;
            events.note_off = arp.gate_off();
        }

        self.sample_counter += 1;
        events
    }

    fn recalculate_period(&mut self) {
        let base = self.division.period_samples(self.sample_rate, self.bpm);
        self.current_period = swing_period(base, self.swing, self.step_counter);
        self.gate_samples = self.current_period * 0.75; // default gate, caller overrides via arp
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::ArpMode;

    #[test]
    fn division_period_at_120bpm() {
        let sr = 44100.0;
        let bpm = 120.0;

        // Quarter note at 120 BPM = 0.5 seconds = 22050 samples.
        let p = ClockDivision::Quarter.period_samples(sr, bpm);
        assert!((p - 22050.0).abs() < 1.0, "quarter at 120 BPM: {p}");

        // Eighth note = half of quarter.
        let p8 = ClockDivision::Eighth.period_samples(sr, bpm);
        assert!((p8 - 11025.0).abs() < 1.0, "eighth at 120 BPM: {p8}");

        // Sixteenth = quarter of quarter.
        let p16 = ClockDivision::Sixteenth.period_samples(sr, bpm);
        assert!((p16 - 5512.5).abs() < 1.0, "sixteenth at 120 BPM: {p16}");
    }

    #[test]
    fn division_period_zero_bpm() {
        assert_eq!(
            ClockDivision::Quarter.period_samples(44100.0, 0.0),
            f32::MAX
        );
    }

    #[test]
    fn swing_straight() {
        let base = 1000.0;
        let even = swing_period(base, 0.5, 0);
        let odd = swing_period(base, 0.5, 1);
        assert!((even - base).abs() < f32::EPSILON);
        assert!((odd - base).abs() < f32::EPSILON);
    }

    #[test]
    fn swing_preserves_pair_duration() {
        let base = 1000.0;
        for swing_pct in [0.0, 0.25, 0.5, 0.67, 0.75, 1.0] {
            let even = swing_period(base, swing_pct, 0);
            let odd = swing_period(base, swing_pct, 1);
            let pair = even + odd;
            assert!(
                (pair - 2.0 * base).abs() < 0.01,
                "swing {swing_pct}: pair = {pair}, expected {}",
                2.0 * base
            );
        }
    }

    #[test]
    fn swing_high_delays_offbeat() {
        let base = 1000.0;
        let even = swing_period(base, 0.75, 0);
        let odd = swing_period(base, 0.75, 1);
        // Even (on-beat) is longer, odd (off-beat) is shorter.
        assert!(even > base, "even should be longer: {even}");
        assert!(odd < base, "odd should be shorter: {odd}");
    }

    #[test]
    fn clock_fires_first_tick_immediately() {
        let mut arp = Arpeggiator::new();
        arp.note_on(60, 100);
        let mut clock = ArpClock::new(44100.0, 120.0);
        clock.start();

        let events = clock.tick(&mut arp);
        assert!(events.note_on.is_some(), "first tick should fire note-on");
    }

    #[test]
    fn clock_fires_gate_off() {
        let mut arp = Arpeggiator::new();
        arp.note_on(60, 100);
        arp.set_gate_pct(0.5);
        let mut clock = ArpClock::new(44100.0, 120.0);
        clock.start();

        // First tick: note-on.
        let _ = clock.tick(&mut arp);

        // Advance until gate-off fires.
        let mut gate_off_fired = false;
        for _ in 0..100_000 {
            let events = clock.tick(&mut arp);
            if events.note_off.is_some() {
                gate_off_fired = true;
                break;
            }
        }
        assert!(gate_off_fired, "gate-off should fire before period end");
    }

    #[test]
    fn clock_cycles_through_notes() {
        let mut arp = Arpeggiator::new();
        arp.note_on(60, 100);
        arp.note_on(64, 100);
        arp.note_on(67, 100);
        let mut clock = ArpClock::new(44100.0, 120.0);
        clock.set_division(ClockDivision::Sixteenth);
        clock.start();

        let mut notes = Vec::new();
        for _ in 0..500_000 {
            let events = clock.tick(&mut arp);
            if let Some(NoteEvent::NoteOn { midi_note, .. }) = events.note_on {
                notes.push(midi_note);
                if notes.len() >= 6 {
                    break;
                }
            }
        }
        assert_eq!(notes, [60, 64, 67, 60, 64, 67]);
    }

    #[test]
    fn clock_stopped_emits_nothing() {
        let mut arp = Arpeggiator::new();
        arp.note_on(60, 100);
        let mut clock = ArpClock::new(44100.0, 120.0);
        // Not started.
        for _ in 0..1000 {
            assert!(clock.tick(&mut arp).is_empty());
        }
    }

    #[test]
    fn clock_empty_pool_emits_nothing() {
        let mut arp = Arpeggiator::new();
        let mut clock = ArpClock::new(44100.0, 120.0);
        clock.start();
        for _ in 0..1000 {
            assert!(clock.tick(&mut arp).is_empty());
        }
    }

    #[test]
    fn division_labels_unique() {
        let labels: Vec<_> = ClockDivision::ALL.iter().map(|d| d.label()).collect();
        for (i, a) in labels.iter().enumerate() {
            for (j, b) in labels.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b);
                }
            }
        }
    }

    #[test]
    fn division_next_cycles() {
        let mut d = ClockDivision::Whole;
        for _ in 0..8 {
            d = d.next();
        }
        assert_eq!(d, ClockDivision::Whole);
    }

    #[test]
    fn division_prev_cycles() {
        let mut d = ClockDivision::Whole;
        for _ in 0..8 {
            d = d.prev();
        }
        assert_eq!(d, ClockDivision::Whole);
    }

    #[test]
    fn clock_random_mode_no_repeat() {
        let mut arp = Arpeggiator::new();
        arp.set_mode(ArpMode::Random);
        arp.no_repeat = true;
        arp.note_on(60, 100);
        arp.note_on(64, 100);
        arp.note_on(67, 100);

        let mut clock = ArpClock::new(44100.0, 120.0);
        clock.set_division(ClockDivision::ThirtySecond);
        clock.start();

        let mut notes = Vec::new();
        for _ in 0..500_000 {
            let events = clock.tick(&mut arp);
            if let Some(NoteEvent::NoteOn { midi_note, .. }) = events.note_on {
                notes.push(midi_note);
                if notes.len() >= 20 {
                    break;
                }
            }
        }
        for window in notes.windows(2) {
            assert_ne!(
                window[0], window[1],
                "consecutive repeat in random: {notes:?}"
            );
        }
    }
}
