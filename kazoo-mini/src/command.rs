//! UI → audio-callback command queue.
//!
//! The audio callback drains a bounded lock-free channel. When that channel
//! is full (the callback has stalled) or gone (the stream died), a command
//! cannot be delivered. [`CommandLink`] never loses such a command silently:
//!
//! - commands whose *latest* state matters (note-off, parameter snapshots,
//!   all-notes-off) are remembered and re-sent by [`CommandLink::flush`], so
//!   a stuck note or a stale parameter heals as soon as the callback catches
//!   up;
//! - a note-on that cannot be delivered is counted as dropped, because
//!   replaying it late would sound a note the player has already let go of.
//!
//! [`CommandLink::status`] reports both so the UI can show them.

use crossbeam_channel::{Sender, TrySendError};

use crate::params::MiniParams;

/// Number of MIDI notes.
pub const NOTE_COUNT: usize = 128;

/// Commands sent from the UI thread to the audio callback. No variant owns
/// heap memory.
#[derive(Debug)]
pub enum AudioCommand {
    NoteOn {
        note: u8,
    },
    NoteOff {
        note: u8,
    },
    UpdateParams(MiniParams),
    /// Silence the voice immediately and forget held notes (panic).
    AllNotesOff,
}

/// Delivery state of the command queue, for display.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueueStatus {
    /// Note-ons that could not be delivered and were abandoned.
    pub dropped: u64,
    /// Commands waiting to be re-sent.
    pub pending: usize,
    /// The audio callback is gone; nothing can be delivered any more.
    pub disconnected: bool,
}

/// Sender side of the command queue with retry of state-carrying commands.
#[derive(Debug)]
pub struct CommandLink {
    tx: Sender<AudioCommand>,
    pending_note_off: [bool; NOTE_COUNT],
    params_pending: bool,
    all_notes_off_pending: bool,
    dropped: u64,
    disconnected: bool,
}

impl CommandLink {
    /// Wrap the sending half of the audio command channel.
    #[must_use]
    pub const fn new(tx: Sender<AudioCommand>) -> Self {
        Self {
            tx,
            pending_note_off: [false; NOTE_COUNT],
            params_pending: false,
            all_notes_off_pending: false,
            dropped: 0,
            disconnected: false,
        }
    }

    /// Try to hand one command to the audio callback.
    fn send(&mut self, cmd: AudioCommand) -> bool {
        match self.tx.try_send(cmd) {
            Ok(()) => true,
            // The callback has stalled; the caller decides whether to retry.
            Err(TrySendError::Full(_)) => false,
            Err(TrySendError::Disconnected(_)) => {
                self.disconnected = true;
                false
            }
        }
    }

    /// Deliver anything that must reach the engine before a new note on
    /// `note`: a pending panic, and a pending release of the same note (which
    /// would otherwise cut the new note short when it is finally delivered).
    ///
    /// `note` must be below [`NOTE_COUNT`].
    fn clear_way_for(&mut self, note: u8) -> bool {
        if self.all_notes_off_pending {
            if !self.send(AudioCommand::AllNotesOff) {
                return false;
            }
            self.all_notes_off_pending = false;
        }
        let idx = usize::from(note);
        if self.pending_note_off[idx] {
            if !self.send(AudioCommand::NoteOff { note }) {
                return false;
            }
            self.pending_note_off[idx] = false;
        }
        true
    }

    /// Start a note. Counted as dropped if it cannot be delivered now.
    pub fn note_on(&mut self, note: u8) {
        if usize::from(note) >= NOTE_COUNT {
            self.dropped += 1;
            return;
        }
        if !(self.clear_way_for(note) && self.send(AudioCommand::NoteOn { note })) {
            self.dropped += 1;
        }
    }

    /// Release a note. Retried by [`Self::flush`] until delivered.
    pub fn note_off(&mut self, note: u8) {
        let idx = usize::from(note);
        if idx >= NOTE_COUNT {
            return;
        }
        if !self.send(AudioCommand::NoteOff { note }) {
            self.pending_note_off[idx] = true;
        }
    }

    /// Send a parameter snapshot. Retried by [`Self::flush`] with the
    /// then-current parameters until delivered.
    pub fn params(&mut self, params: &MiniParams) {
        self.params_pending = !self.send(AudioCommand::UpdateParams(params.clone()));
    }

    /// Silence everything. Supersedes any pending note-off.
    pub fn all_notes_off(&mut self) {
        self.pending_note_off = [false; NOTE_COUNT];
        self.all_notes_off_pending = !self.send(AudioCommand::AllNotesOff);
    }

    /// Re-send everything still pending, in order: panic, releases,
    /// parameters. Stops at the first failure so ordering holds.
    /// `current` is only called when a parameter snapshot is pending.
    pub fn flush(&mut self, current: impl FnOnce() -> MiniParams) {
        if self.all_notes_off_pending {
            if !self.send(AudioCommand::AllNotesOff) {
                return;
            }
            self.all_notes_off_pending = false;
        }
        for (note, pending) in (0..=u8::MAX).zip(self.pending_note_off) {
            if pending {
                if !self.send(AudioCommand::NoteOff { note }) {
                    return;
                }
                self.pending_note_off[usize::from(note)] = false;
            }
        }
        if self.params_pending {
            self.params(&current());
        }
    }

    /// Current delivery state.
    #[must_use]
    pub fn status(&self) -> QueueStatus {
        let note_offs = self.pending_note_off.iter().filter(|&&p| p).count();
        QueueStatus {
            dropped: self.dropped,
            pending: note_offs
                + usize::from(self.params_pending)
                + usize::from(self.all_notes_off_pending),
            disconnected: self.disconnected,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::MiniVoice;

    fn drain(rx: &crossbeam_channel::Receiver<AudioCommand>) -> Vec<AudioCommand> {
        rx.try_iter().collect()
    }

    fn default_params() -> MiniParams {
        MiniParams::from_voice(&MiniVoice::new(44100.0))
    }

    #[test]
    fn delivers_when_there_is_room() {
        let (tx, rx) = crossbeam_channel::bounded(8);
        let mut link = CommandLink::new(tx);
        link.note_on(60);
        link.note_off(60);
        let got = drain(&rx);
        assert!(matches!(got[0], AudioCommand::NoteOn { note: 60 }));
        assert!(matches!(got[1], AudioCommand::NoteOff { note: 60 }));
        assert_eq!(link.status(), QueueStatus::default());
    }

    #[test]
    fn note_on_on_full_queue_is_counted() {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let mut link = CommandLink::new(tx);
        link.note_on(60);
        link.note_on(62);
        assert_eq!(link.status().dropped, 1);
        assert_eq!(drain(&rx).len(), 1);
    }

    #[test]
    fn note_off_on_full_queue_is_retried() {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let mut link = CommandLink::new(tx);
        link.note_on(60);
        link.note_off(60);
        assert_eq!(link.status().pending, 1, "release must be remembered");

        assert_eq!(drain(&rx).len(), 1);
        link.flush(default_params);
        let got = drain(&rx);
        assert!(matches!(got[0], AudioCommand::NoteOff { note: 60 }));
        assert_eq!(link.status(), QueueStatus::default());
    }

    #[test]
    fn pending_note_off_is_sent_before_retrigger() {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let mut link = CommandLink::new(tx);
        link.note_on(60);
        link.note_off(60); // queue full: pending
        assert_eq!(drain(&rx).len(), 1);

        link.note_on(60);
        let got = drain(&rx);
        assert!(matches!(got[0], AudioCommand::NoteOff { note: 60 }));
        assert_eq!(link.status().dropped, 1);
        assert_eq!(link.status().pending, 0);
    }

    #[test]
    fn params_are_retried_only_when_pending() {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let mut link = CommandLink::new(tx);
        link.note_on(60);
        link.params(&default_params());
        assert_eq!(link.status().pending, 1);

        assert_eq!(drain(&rx).len(), 1);
        link.flush(default_params);
        assert!(matches!(drain(&rx)[0], AudioCommand::UpdateParams(_)));
        assert_eq!(link.status().pending, 0);

        // Nothing pending: the snapshot is not even built.
        link.flush(|| panic!("no params should be requested"));
        assert!(drain(&rx).is_empty());
    }

    #[test]
    fn all_notes_off_supersedes_pending_releases() {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let mut link = CommandLink::new(tx);
        link.note_on(60);
        link.note_off(60);
        link.all_notes_off();
        assert_eq!(link.status().pending, 1, "only the panic is pending");

        assert_eq!(drain(&rx).len(), 1);
        link.flush(default_params);
        let got = drain(&rx);
        assert_eq!(got.len(), 1);
        assert!(matches!(got[0], AudioCommand::AllNotesOff));
    }

    #[test]
    fn disconnected_engine_is_reported() {
        let (tx, rx) = crossbeam_channel::bounded(1);
        drop(rx);
        let mut link = CommandLink::new(tx);
        link.note_on(60);
        let status = link.status();
        assert!(status.disconnected);
        assert_eq!(status.dropped, 1);
    }
}
