//! Application state for the standalone arpeggiator TUI.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ringbuf::HeapCons;
use ringbuf::traits::Consumer;

use std::sync::atomic::Ordering;

use crate::audio::{
    ArpCommand, AudioCommand, CommandSender, DEFAULT_BPM, DisplayEvent, EngineShared, SendFailure,
    apply_command,
};
use kazoo_arp::{ArpClock, ArpMode, Arpeggiator, MAX_BPM, MAX_SWING, MIN_BPM, MIN_SWING};
use kazoo_core::ipc::link::LinkStatus;

/// Velocity of notes played on the computer keyboard.
const KEY_VELOCITY: u8 = 100;
/// Swing change per key press.
const SWING_STEP: f64 = 0.05;
/// Gate change per key press.
const GATE_STEP: f32 = 0.05;

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

    const fn index(self) -> usize {
        match self {
            Self::Bpm => 0,
            Self::Mode => 1,
            Self::Division => 2,
            Self::Swing => 3,
            Self::Gate => 4,
            Self::Octave => 5,
            Self::Latch => 6,
        }
    }

    pub const fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    pub const fn prev(self) -> Self {
        Self::ALL[(self.index() + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

/// How the computer keyboard plays notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyMode {
    /// The terminal reports key releases: a note sounds while its key is
    /// held.
    Hold,
    /// The terminal cannot report key releases: a tap presses a note, the
    /// next tap releases it.
    Toggle,
}

/// Audio stream health, read from the engine each frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StreamHealth {
    /// Errors cpal reported for the output stream.
    pub errors: u64,
    /// The output device is gone: no more audio until restart.
    pub lost: bool,
    /// Display events the UI missed because it fell behind.
    pub display_dropped: u64,
}

/// Standalone TUI application state.
///
/// `arp` and `clock` are a display copy of the audio thread's: every change
/// is sent to the engine first and applied here only once it was queued,
/// and the copy's pattern cursor is restored from the engine after every
/// note it plays — never simulated. Play/stop and tempo belong to the desk
/// when plugged in, so they are never applied here: the copy follows what
/// the engine publishes ([`Self::follow_engine`]).
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
    /// Commands to the audio thread, with failed deliveries counted.
    pub commands: CommandSender,
    /// How keyboard notes behave on this terminal.
    pub key_mode: KeyMode,
    /// Why key releases are unavailable, when the terminal could not be
    /// asked.
    pub key_mode_reason: Option<String>,
    /// Output stream health, mirrored from the audio side.
    pub stream: StreamHealth,
    /// Desk link state; `None` until the first poll.
    pub hub: Option<LinkStatus>,
    /// Whether the engine's clock is running.
    pub playing: bool,
    /// Song position the engine last reported, in beats.
    pub beat: f64,
    /// Transport messages lost between the engine and the desk.
    pub desk_lost: u64,
    /// Keyboard notes currently pressed.
    held: [bool; 128],
    /// Note-offs the engine has not received yet (its queue was full);
    /// retried every frame so no note is left hanging.
    pending_off: [bool; 128],
    /// Ring buffer consumer for display events from the audio thread.
    display_cons: HeapCons<DisplayEvent>,
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
            .field("key_mode", &self.key_mode)
            .field("commands", &self.commands)
            .field("hub", &self.hub)
            .field("playing", &self.playing)
            .field("desk_lost", &self.desk_lost)
            .finish_non_exhaustive()
    }
}

impl App {
    pub fn new(
        sample_rate: f64,
        commands: CommandSender,
        display_cons: HeapCons<DisplayEvent>,
        key_mode: KeyMode,
    ) -> Self {
        Self {
            arp: Arpeggiator::new(),
            clock: ArpClock::new(sample_rate, DEFAULT_BPM),
            selected_param: Param::Bpm,
            should_quit: false,
            recent_notes: [None; 32],
            recent_head: 0,
            last_note_on: None,
            last_pattern_position: 0,
            commands,
            key_mode,
            key_mode_reason: None,
            stream: StreamHealth::default(),
            hub: None,
            // The engine starts playing on its own until the desk says
            // otherwise.
            playing: true,
            beat: 0.0,
            desk_lost: 0,
            held: [false; 128],
            pending_off: [false; 128],
            display_cons,
        }
    }

    /// Follow the transport the engine publishes: its play state, tempo
    /// (the desk's, when plugged in), song position and lost desk messages.
    pub fn follow_engine(&mut self, shared: &EngineShared) {
        self.playing = shared.playing.load(Ordering::Acquire);
        self.clock
            .set_bpm(f64::from_bits(shared.bpm.load(Ordering::Acquire)));
        self.beat = f64::from_bits(shared.beat.load(Ordering::Acquire));
        self.desk_lost = shared.desk_lost.load(Ordering::Relaxed);
    }

    /// Drain display events from the audio thread's ring buffer.
    ///
    /// Updates recent note history, the current note highlight and the
    /// pattern position, and moves the display copy's cursor to where the
    /// engine's is, keeping `peek_pattern` exact.
    pub fn drain_display_events(&mut self) {
        while let Some(event) = self.display_cons.try_pop() {
            match event {
                DisplayEvent::Played { midi_note, cursor } => {
                    self.recent_notes[self.recent_head] = Some(midi_note);
                    self.recent_head = (self.recent_head + 1) % self.recent_notes.len();
                    // Persist highlight until next note (fixes flicker on gate-off).
                    self.last_note_on = Some(midi_note);
                    self.arp.restore_cursor(cursor);
                    self.last_pattern_position = self.arp.played_position();
                }
                DisplayEvent::HubNoteOn {
                    midi_note,
                    velocity,
                } => self.arp.note_on(midi_note, velocity),
                DisplayEvent::HubNoteOff { midi_note } => self.arp.note_off(midi_note),
            }
        }
    }

    /// Re-send note-offs the engine's full queue refused earlier.
    pub fn retry_pending_note_offs(&mut self) {
        for note in 0..=127_u8 {
            let idx = usize::from(note);
            if !self.pending_off[idx] {
                continue;
            }
            // Delivered, or the engine is gone and holds no notes: done.
            // Still full: the failure was counted when it first happened;
            // try again next frame.
            let outcome = self
                .commands
                .deliver(AudioCommand::Arp(ArpCommand::NoteOff { midi_note: note }));
            self.pending_off[idx] = outcome == Err(SendFailure::QueueFull);
        }
    }

    /// Notes whose release has not reached the engine yet.
    #[must_use]
    pub fn pending_note_offs(&self) -> usize {
        self.pending_off.iter().filter(|&&p| p).count()
    }

    /// Send an arpeggiator change; apply it to the display copy only once
    /// queued.
    fn send_and_apply(&mut self, cmd: ArpCommand) {
        if self.commands.send(AudioCommand::Arp(cmd)) {
            apply_command(cmd, &mut self.arp, &mut self.clock);
        }
    }

    /// Handle a key event.
    pub fn handle_key(&mut self, key: KeyEvent) {
        match key.kind {
            KeyEventKind::Press | KeyEventKind::Repeat => self.handle_press(key),
            KeyEventKind::Release => {
                if let KeyCode::Char(ch) = key.code {
                    if let Some(note) = Self::key_to_note(ch) {
                        self.release_note(note);
                    }
                }
            }
        }
    }

    fn handle_press(&mut self, key: KeyEvent) {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            KeyCode::Left => self.selected_param = self.selected_param.prev(),
            KeyCode::Right => self.selected_param = self.selected_param.next(),
            KeyCode::Up => self.adjust_param(true, shift),
            KeyCode::Down => self.adjust_param(false, shift),
            KeyCode::Char(' ') => self.set_latch(!self.arp.latch),
            // Play/stop: the whole studio when plugged into the desk.
            KeyCode::Enter => {
                let cmd = if self.playing {
                    AudioCommand::Stop
                } else {
                    AudioCommand::Play
                };
                self.commands.send(cmd);
            }
            // Mode shortcuts: 1-5.
            KeyCode::Char(c @ '1'..='5') => {
                let idx = usize::from(c as u8 - b'1');
                self.send_and_apply(ArpCommand::SetMode(ArpMode::ALL[idx]));
            }
            // Piano keys.
            KeyCode::Char(ch) => {
                if let Some(note) = Self::key_to_note(ch) {
                    let is_held = self.held[usize::from(note)];
                    match (self.key_mode, is_held) {
                        (KeyMode::Toggle, true) => self.release_note(note),
                        (KeyMode::Hold | KeyMode::Toggle, false) => self.press_note(note),
                        // Auto-repeat of a key already held.
                        (KeyMode::Hold, true) => {}
                    }
                }
            }
            _ => {}
        }
    }

    fn press_note(&mut self, note: u8) {
        let idx = usize::from(note);
        let cmd = ArpCommand::NoteOn {
            midi_note: note,
            velocity: KEY_VELOCITY,
        };
        if self.commands.send(AudioCommand::Arp(cmd)) {
            // The note-on is queued behind any undelivered note-off, which
            // would cut the new note: drop the stale release.
            self.pending_off[idx] = false;
            self.held[idx] = true;
            apply_command(cmd, &mut self.arp, &mut self.clock);
        }
    }

    fn release_note(&mut self, note: u8) {
        let idx = usize::from(note);
        if !self.held[idx] {
            return;
        }
        self.held[idx] = false;
        let cmd = ArpCommand::NoteOff { midi_note: note };
        // The key is up whatever happens, so the display copy follows it;
        // a release the engine did not get is retried until it does.
        apply_command(cmd, &mut self.arp, &mut self.clock);
        if !self.commands.send(AudioCommand::Arp(cmd))
            && self.commands.last_failure() == Some(SendFailure::QueueFull)
        {
            self.pending_off[idx] = true;
        }
    }

    fn set_latch(&mut self, enabled: bool) {
        if enabled == self.arp.latch {
            return;
        }
        let cmd = ArpCommand::SetLatch(enabled);
        if self.commands.send(AudioCommand::Arp(cmd)) {
            apply_command(cmd, &mut self.arp, &mut self.clock);
            if !enabled && self.key_mode == KeyMode::Toggle {
                // Unlatching cleared the pool; tapped notes are released.
                self.held = [false; 128];
            }
        }
    }

    /// Raise (`up`) or lower the selected parameter.
    ///
    /// `shift`: when true, BPM adjusts in 10.0 steps instead of 1.0.
    fn adjust_param(&mut self, up: bool, shift: bool) {
        let cmd = match self.selected_param {
            Param::Bpm => {
                let step = if shift { 10.0 } else { 1.0 };
                let sign: f64 = if up { 1.0 } else { -1.0 };
                let bpm = sign.mul_add(step, self.clock.bpm()).clamp(MIN_BPM, MAX_BPM);
                // The desk's to decide when plugged in: shown once the
                // engine publishes it.
                self.commands.send(AudioCommand::SetBpm(bpm));
                return;
            }
            Param::Mode => ArpCommand::SetMode(if up {
                self.arp.mode.next()
            } else {
                self.arp.mode.prev()
            }),
            Param::Division => ArpCommand::SetDivision(if up {
                self.clock.division().next()
            } else {
                self.clock.division().prev()
            }),
            Param::Swing => {
                let sign: f64 = if up { 1.0 } else { -1.0 };
                ArpCommand::SetSwing(
                    sign.mul_add(SWING_STEP, self.clock.swing())
                        .clamp(MIN_SWING, MAX_SWING),
                )
            }
            Param::Gate => {
                let sign: f32 = if up { 1.0 } else { -1.0 };
                ArpCommand::SetGate(sign.mul_add(GATE_STEP, self.arp.gate_pct))
            }
            Param::Octave => ArpCommand::SetOctaveRange(if up {
                self.arp.octave_range.saturating_add(1)
            } else {
                self.arp.octave_range.saturating_sub(1)
            }),
            Param::Latch => {
                // Up = enable latch only, Down = disable only. Space toggles.
                self.set_latch(up);
                return;
            }
        };
        self.send_and_apply(cmd);
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

    /// Whether the desk is playing this instrument.
    #[must_use]
    pub fn hub_connected(&self) -> bool {
        self.hub.as_ref().is_some_and(|hub| hub.connected)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::Receiver;
    use kazoo_arp::ArpCursor;
    use ringbuf::HeapProd;
    use ringbuf::traits::{Producer, Split};

    struct Rig {
        app: App,
        rx: Receiver<AudioCommand>,
        display: HeapProd<DisplayEvent>,
    }

    fn rig(capacity: usize, key_mode: KeyMode) -> Rig {
        let (tx, rx) = crossbeam_channel::bounded(capacity);
        let (display, cons) = ringbuf::HeapRb::new(64).split();
        Rig {
            app: App::new(48_000.0, CommandSender::new(tx), cons, key_mode),
            rx,
            display,
        }
    }

    fn key(code: KeyCode, kind: KeyEventKind) -> KeyEvent {
        KeyEvent::new_with_kind(code, KeyModifiers::NONE, kind)
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_key(key(code, KeyEventKind::Press));
    }

    fn release(app: &mut App, code: KeyCode) {
        app.handle_key(key(code, KeyEventKind::Release));
    }

    #[test]
    fn param_changes_send_only_the_changed_value() {
        let mut rig = rig(8, KeyMode::Hold);
        press(&mut rig.app, KeyCode::Up);
        assert_eq!(rig.rx.try_recv().unwrap(), AudioCommand::SetBpm(121.0));
        assert!(rig.rx.is_empty());
        // Tempo is the desk's when plugged in: shown once the engine says.
        assert!((rig.app.clock.bpm() - DEFAULT_BPM).abs() < f64::EPSILON);
        rig.app.selected_param = Param::Swing;
        press(&mut rig.app, KeyCode::Up);
        assert_eq!(
            rig.rx.try_recv().unwrap(),
            AudioCommand::Arp(ArpCommand::SetSwing(0.55))
        );
        assert!((rig.app.clock.swing() - 0.55).abs() < 1e-12);
    }

    #[test]
    fn undelivered_changes_leave_the_display_unchanged() {
        let mut rig = rig(1, KeyMode::Hold);
        press(&mut rig.app, KeyCode::Char('2')); // fills the queue
        assert_eq!(rig.app.arp.mode, ArpMode::Down);
        press(&mut rig.app, KeyCode::Char('3'));
        press(&mut rig.app, KeyCode::Char(' '));
        press(&mut rig.app, KeyCode::Char('z'));
        assert_eq!(rig.app.arp.mode, ArpMode::Down);
        assert!(!rig.app.arp.latch);
        assert!(!rig.app.arp.has_notes());
        assert_eq!(rig.app.commands.failed(), 3);
    }

    #[test]
    fn hold_mode_plays_while_held() {
        let mut rig = rig(8, KeyMode::Hold);
        press(&mut rig.app, KeyCode::Char('z'));
        press(&mut rig.app, KeyCode::Char('z')); // auto-repeat
        assert_eq!(rig.app.arp.note_count(), 1);
        release(&mut rig.app, KeyCode::Char('z'));
        assert!(!rig.app.arp.has_notes());
        let sent: Vec<_> = rig.rx.try_iter().collect();
        assert_eq!(
            sent,
            [
                AudioCommand::Arp(ArpCommand::NoteOn {
                    midi_note: 60,
                    velocity: KEY_VELOCITY
                }),
                AudioCommand::Arp(ArpCommand::NoteOff { midi_note: 60 }),
            ]
        );
    }

    #[test]
    fn toggle_mode_taps_press_and_release() {
        let mut rig = rig(8, KeyMode::Toggle);
        press(&mut rig.app, KeyCode::Char('z'));
        assert!(rig.app.arp.has_notes());
        press(&mut rig.app, KeyCode::Char('z'));
        assert!(!rig.app.arp.has_notes());
        assert_eq!(rig.rx.try_iter().count(), 2);
    }

    #[test]
    fn refused_note_off_is_retried_until_delivered() {
        let mut rig = rig(1, KeyMode::Hold);
        press(&mut rig.app, KeyCode::Char('z')); // fills the queue
        release(&mut rig.app, KeyCode::Char('z'));
        assert_eq!(rig.app.pending_note_offs(), 1);
        assert_eq!(rig.app.commands.failed(), 1);

        rig.app.retry_pending_note_offs();
        assert_eq!(rig.app.pending_note_offs(), 1, "queue still full");
        assert_eq!(rig.app.commands.failed(), 1, "retries are not recounted");

        assert!(matches!(
            rig.rx.try_recv(),
            Ok(AudioCommand::Arp(ArpCommand::NoteOn { .. }))
        ));
        rig.app.retry_pending_note_offs();
        assert_eq!(rig.app.pending_note_offs(), 0);
        assert_eq!(
            rig.rx.try_recv().unwrap(),
            AudioCommand::Arp(ArpCommand::NoteOff { midi_note: 60 })
        );
    }

    #[test]
    fn played_events_sync_the_display_cursor() {
        let mut rig = rig(8, KeyMode::Hold);
        for ch in ['z', 'c', 'b'] {
            press(&mut rig.app, KeyCode::Char(ch));
        }
        // An engine copy with the same notes plays two steps.
        let mut engine = rig.app.arp.clone();
        let mut last: Option<(u8, ArpCursor)> = None;
        for _ in 0..2 {
            if let Some(kazoo_arp::NoteEvent::NoteOn { midi_note, .. }) = engine.step() {
                last = Some((midi_note, engine.cursor()));
                rig.display
                    .try_push(DisplayEvent::Played {
                        midi_note,
                        cursor: engine.cursor(),
                    })
                    .unwrap();
            }
        }
        rig.app.drain_display_events();
        let (note, cursor) = last.unwrap();
        assert_eq!(rig.app.last_note_on, Some(note));
        assert_eq!(rig.app.arp.cursor(), cursor);
        assert_eq!(rig.app.last_pattern_position, 1);
        assert_eq!(rig.app.recent_note_list(), [60, 64]);
    }

    #[test]
    fn hub_notes_join_the_display_pool() {
        let mut rig = rig(8, KeyMode::Hold);
        rig.display
            .try_push(DisplayEvent::HubNoteOn {
                midi_note: 48,
                velocity: 90,
            })
            .unwrap();
        rig.app.drain_display_events();
        assert_eq!(rig.app.arp.note_count(), 1);
        rig.display
            .try_push(DisplayEvent::HubNoteOff { midi_note: 48 })
            .unwrap();
        rig.app.drain_display_events();
        assert!(!rig.app.arp.has_notes());
    }

    #[test]
    fn latch_keys_only_set_their_direction() {
        let mut rig = rig(8, KeyMode::Hold);
        rig.app.selected_param = Param::Latch;
        press(&mut rig.app, KeyCode::Down);
        assert!(rig.rx.is_empty(), "latch already off");
        press(&mut rig.app, KeyCode::Up);
        press(&mut rig.app, KeyCode::Up);
        assert!(rig.app.arp.latch);
        assert_eq!(rig.rx.try_iter().count(), 1);
    }

    #[test]
    fn enter_asks_to_play_or_stop_and_the_engine_decides() {
        let mut rig = rig(8, KeyMode::Hold);
        press(&mut rig.app, KeyCode::Enter);
        assert_eq!(rig.rx.try_recv().unwrap(), AudioCommand::Stop);
        // Nothing changes on screen until the engine publishes it.
        assert!(rig.app.playing);
        let shared = EngineShared::default();
        shared.bpm.store(97.5_f64.to_bits(), Ordering::Release);
        shared.beat.store(4.25_f64.to_bits(), Ordering::Release);
        shared.desk_lost.store(2, Ordering::Relaxed);
        rig.app.follow_engine(&shared);
        assert!(!rig.app.playing);
        assert!((rig.app.clock.bpm() - 97.5).abs() < f64::EPSILON);
        assert!((rig.app.beat - 4.25).abs() < f64::EPSILON);
        assert_eq!(rig.app.desk_lost, 2);
        press(&mut rig.app, KeyCode::Enter);
        assert_eq!(rig.rx.try_recv().unwrap(), AudioCommand::Play);
    }

    #[test]
    fn param_cycle_wraps() {
        assert_eq!(Param::Bpm.prev(), Param::Latch);
        assert_eq!(Param::Latch.next(), Param::Bpm);
        for param in Param::ALL {
            assert_eq!(param.next().prev(), param);
        }
    }
}
