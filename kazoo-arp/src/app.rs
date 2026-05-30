//! Application state for the standalone arpeggiator TUI.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ringbuf::HeapCons;
use ringbuf::traits::Consumer;

use kazoo_arp::{ArpClock, ArpMode, Arpeggiator};

/// Note event snapshot pushed from the audio thread for display.
///
/// The audio callback pushes one of these on each note-on event.
/// The UI drains them each frame to update display state.
#[derive(Debug, Clone, Copy)]
pub struct DisplayEvent {
    /// MIDI note number that was triggered.
    pub midi_note: u8,
}

/// Which parameter the cursor is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Param {
    Bpm,
    Mode,
    Division,
    Swing,
    Gate,
    Octave,
    Latch,
}

impl Param {
    const ALL: [Self; 7] = [
        Self::Bpm,
        Self::Mode,
        Self::Division,
        Self::Swing,
        Self::Gate,
        Self::Octave,
        Self::Latch,
    ];

    pub fn next(self) -> Self {
        let idx = Self::ALL.iter().position(|&p| p == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }

    pub fn prev(self) -> Self {
        let idx = Self::ALL.iter().position(|&p| p == self).unwrap_or(0);
        Self::ALL[(idx + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

/// Standalone TUI application state.
///
/// Display state (current note, recent history) is driven by events from the
/// audio thread via a lock-free SPSC ring buffer — NOT a parallel simulation.
pub struct App {
    pub arp: Arpeggiator,
    pub clock: ArpClock,
    pub selected_param: Param,
    pub should_quit: bool,
    /// Recent note-on events for display (circular buffer).
    pub recent_notes: [Option<u8>; 32],
    pub recent_head: usize,
    /// Last note-on for highlight — persists until the NEXT note fires
    /// (not cleared on gate-off, preventing flicker).
    pub last_note_on: Option<u8>,
    /// Pattern position of the most recently played note (for step numbering).
    pub last_pattern_position: usize,
    /// Ring buffer consumer for display events from the audio thread.
    display_cons: HeapCons<DisplayEvent>,
    /// Hub connection state (shared with audio thread).
    hub_connected: Arc<AtomicBool>,
}

// HeapCons is !Debug; implement manually.
impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("arp", &self.arp)
            .field("clock", &self.clock)
            .field("selected_param", &self.selected_param)
            .field("should_quit", &self.should_quit)
            .field("last_note_on", &self.last_note_on)
            .field("hub_connected", &self.hub_connected())
            .finish_non_exhaustive()
    }
}

impl App {
    pub fn new(
        sample_rate: f32,
        bpm: f32,
        display_cons: HeapCons<DisplayEvent>,
        hub_connected: Arc<AtomicBool>,
    ) -> Self {
        let mut clock = ArpClock::new(sample_rate, bpm);
        clock.start();
        Self {
            arp: Arpeggiator::new(),
            clock,
            selected_param: Param::Bpm,
            should_quit: false,
            recent_notes: [None; 32],
            recent_head: 0,
            last_note_on: None,
            last_pattern_position: 0,
            display_cons,
            hub_connected,
        }
    }

    /// Drain display events from the audio thread's ring buffer.
    ///
    /// Updates recent note history, current note highlight, and pattern
    /// position. The UI arp is stepped in sync to keep `peek_pattern` accurate.
    pub fn drain_display_events(&mut self) {
        while let Some(event) = self.display_cons.try_pop() {
            // Record note in recent history.
            self.recent_notes[self.recent_head] = Some(event.midi_note);
            self.recent_head = (self.recent_head + 1) % self.recent_notes.len();

            // Persist highlight until next note (fixes flicker on gate-off).
            self.last_note_on = Some(event.midi_note);

            // Track pattern position before stepping.
            self.last_pattern_position = self.arp.position();

            // Step UI arp to keep peek_pattern in sync with the audio thread.
            if self.arp.has_notes() {
                let _ = self.arp.step();
            }
        }
    }

    /// Increment the currently selected parameter.
    ///
    /// `shift`: when true, BPM adjusts in 10.0 steps instead of 1.0.
    pub fn increment_param(&mut self, shift: bool) {
        match self.selected_param {
            Param::Bpm => {
                let step = if shift { 10.0 } else { 1.0 };
                self.clock.set_bpm((self.clock.bpm + step).min(300.0));
            }
            Param::Mode => self.arp.set_mode(self.arp.mode.next()),
            Param::Division => self.clock.set_division(self.clock.division.next()),
            Param::Swing => self.clock.set_swing((self.clock.swing + 0.05).min(1.0)),
            Param::Gate => self.arp.set_gate_pct(self.arp.gate_pct + 0.05),
            Param::Octave => self.arp.set_octave_range(self.arp.octave_range + 1),
            Param::Latch => {
                // Up = enable latch only. Space is toggle.
                if !self.arp.latch {
                    self.arp.toggle_latch();
                }
            }
        }
    }

    /// Decrement the currently selected parameter.
    ///
    /// `shift`: when true, BPM adjusts in 10.0 steps instead of 1.0.
    pub fn decrement_param(&mut self, shift: bool) {
        match self.selected_param {
            Param::Bpm => {
                let step = if shift { 10.0 } else { 1.0 };
                self.clock.set_bpm((self.clock.bpm - step).max(1.0));
            }
            Param::Mode => self.arp.set_mode(self.arp.mode.prev()),
            Param::Division => self.clock.set_division(self.clock.division.prev()),
            Param::Swing => self.clock.set_swing((self.clock.swing - 0.05).max(0.0)),
            Param::Gate => self.arp.set_gate_pct(self.arp.gate_pct - 0.05),
            Param::Octave => self
                .arp
                .set_octave_range(self.arp.octave_range.saturating_sub(1)),
            Param::Latch => {
                // Down = disable latch only. Space is toggle.
                if self.arp.latch {
                    self.arp.toggle_latch();
                }
            }
        }
    }

    /// Map a keyboard character to a MIDI note (computer piano layout).
    /// Returns `None` if the key isn't mapped.
    #[must_use]
    pub const fn key_to_note(ch: char) -> Option<u8> {
        match ch {
            'z' => Some(60), // C4
            's' => Some(61), // C#4
            'x' => Some(62), // D4
            'd' => Some(63), // D#4
            'c' => Some(64), // E4
            'v' => Some(65), // F4
            'g' => Some(66), // F#4
            'b' => Some(67), // G4
            'h' => Some(68), // G#4
            'n' => Some(69), // A4
            'j' => Some(70), // A#4
            'm' => Some(71), // B4
            ',' => Some(72), // C5
            _ => None,
        }
    }

    /// Ordered list of recent note-on MIDI values for pattern display.
    pub fn recent_note_list(&self) -> Vec<u8> {
        let len = self.recent_notes.len();
        let mut out = Vec::with_capacity(len);
        for i in 0..len {
            let idx = (self.recent_head + len - 1 - i) % len;
            if let Some(n) = self.recent_notes[idx] {
                out.push(n);
            }
        }
        out.reverse();
        out
    }

    /// Whether the hub IPC connection is active.
    #[must_use]
    pub fn hub_connected(&self) -> bool {
        self.hub_connected.load(Ordering::Acquire)
    }

    /// Expanded pool length (base notes x octave range).
    #[must_use]
    pub fn expanded_pool_len(&self) -> usize {
        let base = if self.arp.mode == ArpMode::AsPlayed {
            self.arp.insertion_order_pool().len()
        } else {
            self.arp.pitch_sorted_pool().len()
        };
        base * self.arp.octave_range as usize
    }
}
