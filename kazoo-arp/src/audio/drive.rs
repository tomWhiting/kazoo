use super::{AudioEngine, NoteEvent, Ordering, RequestError};
use crate::arp::{NoteEvent as DrivenNote, StreamFrame};

impl AudioEngine {
    pub(crate) const fn configure_output(&mut self, args: crate::Args) {
        self.drive.target = args.target;
        self.drive.silent = args.silent;
    }

    pub(super) fn emit_note(&mut self, event: NoteEvent, at: u64) {
        let event = match event {
            NoteEvent::NoteOn {
                midi_note,
                velocity,
            } => DrivenNote {
                note: midi_note,
                velocity,
                on: true,
                at: StreamFrame(at),
            },
            NoteEvent::NoteOff { midi_note } => DrivenNote {
                note: midi_note,
                velocity: 0,
                on: false,
                at: StreamFrame(at),
            },
        };
        #[cfg(test)]
        if let Some(outgoing) = &mut self.outgoing {
            use ringbuf::traits::Producer;
            assert!(outgoing.try_push(event).is_ok(), "test observer full");
        }
        let result = self
            .drive
            .send(event, |message| self.hub.send_note(message));
        self.record_note_result(result);
    }

    pub(super) fn retry_release(&mut self) {
        let result = self.drive.retry(|message| self.hub.send_note(message));
        self.record_note_result(result);
    }

    fn record_note_result(&self, result: Result<(), RequestError>) {
        if let Err(err) = result {
            self.shared.notes_lost.fetch_add(1, Ordering::Relaxed);
            let failure = match err {
                RequestError::NotConnected => 1,
                RequestError::Full => 2,
                RequestError::Invalid => 3,
                RequestError::Unsupported => 4,
            };
            self.shared.note_failure.store(failure, Ordering::Relaxed);
        }
        self.drive.publish(&self.shared);
    }
}
