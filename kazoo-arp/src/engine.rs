//! Core arpeggiator engine.
//!
//! Pure note scheduling logic — no DSP, no audio, no allocation in the tick path.
//! Usable as a library by other instrument crates (kazoo-mini, kazoo-cs80).

/// Arpeggiator pattern mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArpMode {
    /// Ascending pitch order, wraps at top.
    Up,
    /// Descending pitch order, wraps at bottom.
    Down,
    /// Ascending then descending — endpoints play once (exclusive).
    UpDown,
    /// Random note from the expanded pool each tick.
    Random,
    /// Notes in the order they were pressed, not sorted by pitch.
    AsPlayed,
}

impl ArpMode {
    /// All modes in display order.
    pub const ALL: [Self; 5] = [
        Self::Up,
        Self::Down,
        Self::UpDown,
        Self::Random,
        Self::AsPlayed,
    ];

    /// Human-readable label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Up => "Up",
            Self::Down => "Down",
            Self::UpDown => "Up/Down",
            Self::Random => "Random",
            Self::AsPlayed => "As-Played",
        }
    }

    /// Next mode in the cycle.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Up => Self::Down,
            Self::Down => Self::UpDown,
            Self::UpDown => Self::Random,
            Self::Random => Self::AsPlayed,
            Self::AsPlayed => Self::Up,
        }
    }

    /// Previous mode in the cycle.
    #[must_use]
    pub const fn prev(self) -> Self {
        match self {
            Self::Up => Self::AsPlayed,
            Self::Down => Self::Up,
            Self::UpDown => Self::Down,
            Self::Random => Self::UpDown,
            Self::AsPlayed => Self::Random,
        }
    }
}

/// Direction state for `UpDown` mode traversal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Forward,
    Backward,
}

/// A held note with MIDI pitch and velocity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeldNote {
    pub midi_note: u8,
    pub velocity: u8,
}

/// Note event emitted by the arpeggiator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteEvent {
    /// Trigger a note.
    NoteOn { midi_note: u8, velocity: u8 },
    /// Release a note.
    NoteOff { midi_note: u8 },
}

/// Core arpeggiator state machine.
///
/// Manages note pools, pattern index, octave spanning, and gate tracking.
/// All per-tick methods are O(1) — no allocation, no locks.
///
/// Two note pools are maintained:
/// - `pitch_sorted`: ascending by MIDI note (used by Up, Down, `UpDown`, Random).
/// - `insertion_order`: press order (used by `AsPlayed`).
///
/// With `octave_range` > 1, the pool is virtually expanded with transposed
/// copies (+12, +24, +36 semitones) via index arithmetic — no allocation.
#[derive(Debug, Clone)]
pub struct Arpeggiator {
    pitch_sorted: Vec<HeldNote>,
    insertion_order: Vec<HeldNote>,

    /// Linear position in the expanded (octave-spanning) pool.
    position: usize,
    /// Direction for `UpDown` mode.
    direction: Direction,

    /// Pattern mode.
    pub mode: ArpMode,
    /// Octave range (1–4). 1 = base notes only.
    pub octave_range: u8,
    /// Gate length as fraction of step duration (0.1–1.0).
    pub gate_pct: f32,
    /// Latch mode: released keys persist in pool.
    pub latch: bool,
    /// Random mode: avoid playing the same note twice in a row.
    pub no_repeat: bool,

    /// The MIDI note currently sounding (for gate-off).
    current_note: Option<u8>,
    /// Xorshift64 PRNG state for Random mode.
    rng_state: u64,
}

impl Arpeggiator {
    const DEFAULT_SEED: u64 = 0xB16B_00B5_CAFE_D00D;

    /// Create a new arpeggiator with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self {
            pitch_sorted: Vec::with_capacity(16),
            insertion_order: Vec::with_capacity(16),
            position: 0,
            direction: Direction::Forward,
            mode: ArpMode::Up,
            octave_range: 1,
            gate_pct: 0.75,
            latch: false,
            no_repeat: true,
            current_note: None,
            rng_state: Self::DEFAULT_SEED,
        }
    }

    /// Seed the internal PRNG. A seed of 0 is replaced with 1.
    pub const fn seed_rng(&mut self, seed: u64) {
        self.rng_state = if seed == 0 { 1 } else { seed };
    }

    // ----- Note pool management -----

    /// Add a note to the pool. Re-triggering an existing note updates velocity.
    pub fn note_on(&mut self, midi_note: u8, velocity: u8) {
        let was_empty = self.pitch_sorted.is_empty();
        let note = HeldNote {
            midi_note,
            velocity,
        };

        // Remove existing instance if re-triggered.
        self.pitch_sorted.retain(|n| n.midi_note != midi_note);
        self.insertion_order.retain(|n| n.midi_note != midi_note);

        // Insert sorted by pitch.
        let pos = self
            .pitch_sorted
            .binary_search_by_key(&midi_note, |n| n.midi_note)
            .unwrap_or_else(|e| e);
        self.pitch_sorted.insert(pos, note);

        // Append in insertion order.
        self.insertion_order.push(note);

        if was_empty {
            self.reset_position();
        } else {
            self.clamp_position();
        }
    }

    /// Remove a note from the pool. If latch is enabled, the note persists.
    pub fn note_off(&mut self, midi_note: u8) {
        if self.latch {
            return;
        }
        self.pitch_sorted.retain(|n| n.midi_note != midi_note);
        self.insertion_order.retain(|n| n.midi_note != midi_note);
        if self.pitch_sorted.is_empty() {
            self.position = 0;
            self.direction = Direction::Forward;
        } else {
            self.clamp_position();
        }
    }

    /// Clear all held notes.
    pub fn clear(&mut self) {
        self.pitch_sorted.clear();
        self.insertion_order.clear();
        self.position = 0;
        self.direction = Direction::Forward;
        self.current_note = None;
    }

    /// Whether the pool has notes to arpeggiate.
    #[must_use]
    pub fn has_notes(&self) -> bool {
        !self.pitch_sorted.is_empty()
    }

    /// Number of held notes (before octave expansion).
    #[must_use]
    pub fn note_count(&self) -> usize {
        self.pitch_sorted.len()
    }

    /// Read-only access to the pitch-sorted note pool.
    #[must_use]
    pub fn pitch_sorted_pool(&self) -> &[HeldNote] {
        &self.pitch_sorted
    }

    /// Read-only access to the insertion-order pool.
    #[must_use]
    pub fn insertion_order_pool(&self) -> &[HeldNote] {
        &self.insertion_order
    }

    /// The currently sounding note, if any.
    #[must_use]
    pub const fn current_sounding_note(&self) -> Option<u8> {
        self.current_note
    }

    /// Current position in the expanded pool (for display).
    #[must_use]
    pub const fn position(&self) -> usize {
        self.position
    }

    // ----- Stepping -----

    /// Advance to the next step and return a note-on event.
    ///
    /// Returns `None` if the pool is empty. O(1), no allocation.
    #[must_use]
    pub fn step(&mut self) -> Option<NoteEvent> {
        let pool_len = self.active_pool_len();
        if pool_len == 0 {
            return None;
        }

        let expanded = expanded_len(pool_len, self.octave_range);

        // Safety clamp.
        if self.position >= expanded {
            self.position = if self.mode == ArpMode::Down {
                expanded - 1
            } else {
                0
            };
        }

        // For Random, pick position before reading.
        if self.mode == ArpMode::Random {
            self.pick_random(expanded);
        }

        // Read note at current position.
        let note_index = self.position % pool_len;
        let octave = (self.position / pool_len) as u8;
        let base = self.read_pool(note_index);
        let transposed = base.midi_note.saturating_add(octave * 12).min(127);

        self.current_note = Some(transposed);

        // Advance position (non-Random modes).
        if self.mode != ArpMode::Random {
            self.advance(expanded);
        }

        Some(NoteEvent::NoteOn {
            midi_note: transposed,
            velocity: base.velocity,
        })
    }

    /// Generate a note-off for the currently sounding note.
    #[must_use]
    pub fn gate_off(&mut self) -> Option<NoteEvent> {
        self.current_note
            .take()
            .map(|n| NoteEvent::NoteOff { midi_note: n })
    }

    // ----- Parameter setters -----

    /// Set the mode and reset position.
    pub fn set_mode(&mut self, mode: ArpMode) {
        if self.mode != mode {
            self.mode = mode;
            self.reset_position();
        }
    }

    /// Set octave range (clamped to 1–4). Resets position since the
    /// expanded pool size changes fundamentally.
    pub fn set_octave_range(&mut self, range: u8) {
        let new_range = range.clamp(1, 4);
        if new_range != self.octave_range {
            self.octave_range = new_range;
            self.reset_position();
        }
    }

    /// Set gate percentage (clamped to 0.1–1.0).
    pub const fn set_gate_pct(&mut self, pct: f32) {
        self.gate_pct = pct.clamp(0.1, 1.0);
    }

    /// Toggle latch. Turning latch off clears the pool.
    pub fn toggle_latch(&mut self) {
        self.latch = !self.latch;
        if !self.latch {
            self.clear();
        }
    }

    /// Peek at the next `count` notes without modifying state.
    ///
    /// Allocates — for TUI display only, NOT the audio path.
    #[must_use]
    pub fn peek_pattern(&self, count: usize) -> Vec<(u8, u8)> {
        if self.active_pool_len() == 0 {
            return Vec::new();
        }
        let mut shadow = self.clone();
        let mut result = Vec::with_capacity(count);
        for _ in 0..count {
            if let Some(NoteEvent::NoteOn {
                midi_note,
                velocity,
            }) = shadow.step()
            {
                result.push((midi_note, velocity));
            }
        }
        result
    }

    // ----- Internal helpers -----

    fn active_pool_len(&self) -> usize {
        if self.mode == ArpMode::AsPlayed {
            self.insertion_order.len()
        } else {
            self.pitch_sorted.len()
        }
    }

    fn read_pool(&self, index: usize) -> HeldNote {
        if self.mode == ArpMode::AsPlayed {
            self.insertion_order[index]
        } else {
            self.pitch_sorted[index]
        }
    }

    fn reset_position(&mut self) {
        self.direction = Direction::Forward;
        let pool_len = self.active_pool_len();
        let exp = expanded_len(pool_len, self.octave_range);
        self.position = if self.mode == ArpMode::Down {
            exp.saturating_sub(1)
        } else {
            0
        };
    }

    fn clamp_position(&mut self) {
        let pool_len = self.active_pool_len();
        let exp = expanded_len(pool_len, self.octave_range);
        if exp == 0 {
            self.position = 0;
            return;
        }
        if self.position >= exp {
            self.position = if self.mode == ArpMode::Down {
                exp - 1
            } else {
                self.position % exp
            };
        }
    }

    const fn advance(&mut self, expanded: usize) {
        if expanded == 0 {
            return;
        }
        match self.mode {
            ArpMode::Up | ArpMode::AsPlayed => {
                self.position += 1;
                if self.position >= expanded {
                    self.position = 0;
                }
            }
            ArpMode::Down => {
                if self.position == 0 {
                    self.position = expanded - 1;
                } else {
                    self.position -= 1;
                }
            }
            ArpMode::UpDown => {
                if expanded <= 1 {
                    return;
                }
                match self.direction {
                    Direction::Forward => {
                        if self.position >= expanded - 1 {
                            self.direction = Direction::Backward;
                            self.position -= 1;
                        } else {
                            self.position += 1;
                        }
                    }
                    Direction::Backward => {
                        if self.position == 0 {
                            self.direction = Direction::Forward;
                            self.position = 1;
                        } else {
                            self.position -= 1;
                        }
                    }
                }
            }
            ArpMode::Random => {} // Handled in `pick_random`.
        }
    }

    const fn pick_random(&mut self, expanded: usize) {
        if expanded <= 1 {
            self.position = 0;
            return;
        }
        let prev = self.position;
        self.position = self.next_random(expanded);
        if self.no_repeat {
            let mut attempts = 0;
            while self.position == prev && attempts < 10 {
                self.position = self.next_random(expanded);
                attempts += 1;
            }
        }
    }

    /// Xorshift64 PRNG: returns a value in `[0, range)`.
    const fn next_random(&mut self, range: usize) -> usize {
        let mut x = self.rng_state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng_state = x;
        (x as usize) % range
    }
}

impl Default for Arpeggiator {
    fn default() -> Self {
        Self::new()
    }
}

/// Expanded pool length: base pool size times octave range.
#[inline]
const fn expanded_len(pool_len: usize, octave_range: u8) -> usize {
    let range = if octave_range == 0 {
        1
    } else {
        octave_range as usize
    };
    pool_len * range
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_arp(notes: &[(u8, u8)]) -> Arpeggiator {
        let mut arp = Arpeggiator::new();
        for &(note, vel) in notes {
            arp.note_on(note, vel);
        }
        arp
    }

    fn collect_steps(arp: &mut Arpeggiator, count: usize) -> Vec<u8> {
        (0..count)
            .filter_map(|_| match arp.step() {
                Some(NoteEvent::NoteOn { midi_note, .. }) => Some(midi_note),
                _ => None,
            })
            .collect()
    }

    // -- Up mode --

    #[test]
    fn up_three_notes() {
        let mut arp = make_arp(&[(60, 100), (64, 100), (67, 100)]);
        assert_eq!(
            collect_steps(&mut arp, 9),
            [60, 64, 67, 60, 64, 67, 60, 64, 67]
        );
    }

    #[test]
    fn up_single_note() {
        let mut arp = make_arp(&[(60, 100)]);
        assert_eq!(collect_steps(&mut arp, 3), [60, 60, 60]);
    }

    #[test]
    fn up_two_octaves() {
        let mut arp = make_arp(&[(60, 100), (64, 100), (67, 100)]);
        arp.set_octave_range(2);
        assert_eq!(
            collect_steps(&mut arp, 12),
            [60, 64, 67, 72, 76, 79, 60, 64, 67, 72, 76, 79]
        );
    }

    // -- Down mode --

    #[test]
    fn down_three_notes() {
        let mut arp = make_arp(&[(60, 100), (64, 100), (67, 100)]);
        arp.set_mode(ArpMode::Down);
        assert_eq!(
            collect_steps(&mut arp, 9),
            [67, 64, 60, 67, 64, 60, 67, 64, 60]
        );
    }

    #[test]
    fn down_single_note() {
        let mut arp = make_arp(&[(60, 100)]);
        arp.set_mode(ArpMode::Down);
        assert_eq!(collect_steps(&mut arp, 3), [60, 60, 60]);
    }

    #[test]
    fn down_two_octaves() {
        let mut arp = make_arp(&[(60, 100), (64, 100), (67, 100)]);
        arp.set_mode(ArpMode::Down);
        arp.set_octave_range(2);
        assert_eq!(
            collect_steps(&mut arp, 12),
            [79, 76, 72, 67, 64, 60, 79, 76, 72, 67, 64, 60]
        );
    }

    // -- Up/Down mode --

    #[test]
    fn up_down_three_notes() {
        let mut arp = make_arp(&[(60, 100), (64, 100), (67, 100)]);
        arp.set_mode(ArpMode::UpDown);
        // C E G E | C E G E | C E
        assert_eq!(
            collect_steps(&mut arp, 10),
            [60, 64, 67, 64, 60, 64, 67, 64, 60, 64]
        );
    }

    #[test]
    fn up_down_two_notes() {
        let mut arp = make_arp(&[(60, 100), (67, 100)]);
        arp.set_mode(ArpMode::UpDown);
        assert_eq!(collect_steps(&mut arp, 6), [60, 67, 60, 67, 60, 67]);
    }

    #[test]
    fn up_down_single_note() {
        let mut arp = make_arp(&[(60, 100)]);
        arp.set_mode(ArpMode::UpDown);
        assert_eq!(collect_steps(&mut arp, 4), [60, 60, 60, 60]);
    }

    #[test]
    fn up_down_two_octaves() {
        let mut arp = make_arp(&[(60, 100), (64, 100)]);
        arp.set_mode(ArpMode::UpDown);
        arp.set_octave_range(2);
        // Expanded: 60, 64, 72, 76
        // Pattern:  60, 64, 72, 76, 72, 64 | 60, 64, 72, 76, 72, 64
        assert_eq!(
            collect_steps(&mut arp, 12),
            [60, 64, 72, 76, 72, 64, 60, 64, 72, 76, 72, 64]
        );
    }

    // -- Random mode --

    #[test]
    fn random_produces_valid_notes() {
        let mut arp = make_arp(&[(60, 100), (64, 100), (67, 100)]);
        arp.set_mode(ArpMode::Random);
        let notes = collect_steps(&mut arp, 20);
        for &n in &notes {
            assert!(
                n == 60 || n == 64 || n == 67,
                "unexpected note {n} in random mode"
            );
        }
    }

    #[test]
    fn random_no_immediate_repeat() {
        let mut arp = make_arp(&[(60, 100), (64, 100), (67, 100)]);
        arp.set_mode(ArpMode::Random);
        arp.no_repeat = true;
        let notes = collect_steps(&mut arp, 50);
        for window in notes.windows(2) {
            assert_ne!(
                window[0], window[1],
                "random should not repeat consecutively: {notes:?}"
            );
        }
    }

    #[test]
    fn random_single_note_repeats() {
        let mut arp = make_arp(&[(60, 100)]);
        arp.set_mode(ArpMode::Random);
        assert_eq!(collect_steps(&mut arp, 3), [60, 60, 60]);
    }

    // -- As-Played mode --

    #[test]
    fn as_played_preserves_insertion_order() {
        let mut arp = Arpeggiator::new();
        arp.set_mode(ArpMode::AsPlayed);
        arp.note_on(67, 100); // G first
        arp.note_on(60, 100); // C second
        arp.note_on(64, 100); // E third
        assert_eq!(collect_steps(&mut arp, 6), [67, 60, 64, 67, 60, 64]);
    }

    #[test]
    fn as_played_two_octaves() {
        let mut arp = Arpeggiator::new();
        arp.set_mode(ArpMode::AsPlayed);
        arp.set_octave_range(2);
        arp.note_on(67, 100);
        arp.note_on(60, 100);
        // Expanded: 67, 60, 79, 72
        assert_eq!(collect_steps(&mut arp, 8), [67, 60, 79, 72, 67, 60, 79, 72]);
    }

    // -- Pool management --

    #[test]
    fn empty_pool_returns_none() {
        let mut arp = Arpeggiator::new();
        assert!(arp.step().is_none());
    }

    #[test]
    fn note_on_retrigger_updates_velocity() {
        let mut arp = Arpeggiator::new();
        arp.note_on(60, 50);
        arp.note_on(60, 100);
        assert_eq!(arp.note_count(), 1);
        if let Some(NoteEvent::NoteOn { velocity, .. }) = arp.step() {
            assert_eq!(velocity, 100);
        } else {
            panic!("expected NoteOn");
        }
    }

    #[test]
    fn note_off_removes_from_pool() {
        let mut arp = make_arp(&[(60, 100), (64, 100), (67, 100)]);
        arp.note_off(64);
        assert_eq!(arp.note_count(), 2);
        assert_eq!(collect_steps(&mut arp, 4), [60, 67, 60, 67]);
    }

    // -- Gate --

    #[test]
    fn gate_off_returns_current_note() {
        let mut arp = make_arp(&[(60, 100)]);
        let _ = arp.step();
        assert_eq!(arp.gate_off(), Some(NoteEvent::NoteOff { midi_note: 60 }));
    }

    #[test]
    fn gate_off_when_no_note_active() {
        let mut arp = Arpeggiator::new();
        assert!(arp.gate_off().is_none());
    }

    #[test]
    fn gate_off_clears_after_first_call() {
        let mut arp = make_arp(&[(60, 100)]);
        let _ = arp.step();
        let _ = arp.gate_off();
        assert!(arp.gate_off().is_none());
    }

    // -- Latch --

    #[test]
    fn latch_retains_notes() {
        let mut arp = Arpeggiator::new();
        arp.latch = true;
        arp.note_on(60, 100);
        arp.note_on(64, 100);
        arp.note_off(60);
        arp.note_off(64);
        assert!(arp.has_notes());
        assert_eq!(collect_steps(&mut arp, 4), [60, 64, 60, 64]);
    }

    #[test]
    fn toggle_latch_off_clears_pool() {
        let mut arp = Arpeggiator::new();
        arp.latch = true;
        arp.note_on(60, 100);
        arp.note_off(60);
        assert!(arp.has_notes());
        arp.toggle_latch();
        assert!(!arp.has_notes());
    }

    // -- MIDI clamping --

    #[test]
    fn octave_transposition_clamps_to_127() {
        let mut arp = make_arp(&[(120, 100)]);
        arp.set_octave_range(4);
        let notes = collect_steps(&mut arp, 4);
        assert_eq!(notes[0], 120);
        assert!(notes[1..].iter().all(|&n| n <= 127));
    }

    // -- Peek --

    #[test]
    fn peek_does_not_modify_state() {
        let mut arp = make_arp(&[(60, 100), (64, 100), (67, 100)]);
        let _ = arp.step(); // position advances
        let pos_before = arp.position();
        let peeked = arp.peek_pattern(6);
        assert_eq!(arp.position(), pos_before);
        assert_eq!(peeked.len(), 6);
    }

    // -- Octave range setter --

    #[test]
    fn set_octave_range_clamps() {
        let mut arp = Arpeggiator::new();
        arp.set_octave_range(0);
        assert_eq!(arp.octave_range, 1);
        arp.set_octave_range(10);
        assert_eq!(arp.octave_range, 4);
    }

    // -- Gate pct setter --

    #[test]
    fn set_gate_pct_clamps() {
        let mut arp = Arpeggiator::new();
        arp.set_gate_pct(0.0);
        assert!((arp.gate_pct - 0.1).abs() < f32::EPSILON);
        arp.set_gate_pct(2.0);
        assert!((arp.gate_pct - 1.0).abs() < f32::EPSILON);
    }

    // -- Mode cycling --

    #[test]
    fn mode_next_cycles() {
        let mut m = ArpMode::Up;
        for _ in 0..5 {
            m = m.next();
        }
        assert_eq!(m, ArpMode::Up);
    }

    #[test]
    fn mode_prev_cycles() {
        let mut m = ArpMode::Up;
        for _ in 0..5 {
            m = m.prev();
        }
        assert_eq!(m, ArpMode::Up);
    }
}
