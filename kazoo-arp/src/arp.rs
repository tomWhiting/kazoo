use kazoo_core::ipc::link::RequestError;
use kazoo_core::ipc::types::{NOTE_OFF, NOTE_ON, NoteEventMsg};
use std::sync::atomic::Ordering;

use crate::audio::EngineShared;

#[cfg(test)]
mod wire_tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamFrame(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoteEvent {
    pub note: u8,
    pub velocity: u8,
    pub on: bool,
    pub at: StreamFrame,
}

impl NoteEvent {
    pub const fn message(self, target: [u8; 16]) -> NoteEventMsg {
        NoteEventMsg {
            source: [0; 16],
            target,
            event_type: if self.on { NOTE_ON } else { NOTE_OFF },
            channel: 0,
            note: self.note,
            velocity: self.velocity,
        }
    }
}

#[derive(Debug, Default)]
pub struct Drive {
    pub target: [u8; 16],
    pub silent: bool,
    active: Option<u8>,
    pending: Option<NoteEvent>,
}

impl Drive {
    pub fn send(
        &mut self,
        event: NoteEvent,
        mut send: impl FnMut(NoteEventMsg) -> Result<(), RequestError>,
    ) -> Result<(), RequestError> {
        // A queued release must precede every later onset, including a
        // retrigger of the same pitch, so a late release cannot cut it off.
        // It goes first; if the link still refuses it, this event is refused
        // with the link's own reason.
        if let Some(off) = self
            .pending
            .filter(|off| event.on || off.note != event.note)
        {
            send(off.message(self.target))?;
            self.pending = None;
            if self.active == Some(off.note) {
                self.active = None;
            }
        }
        // The legacy message carries MIDI data only. Its enqueue happens
        // on the scheduler's stream frame rather than on the UI's clock.
        match send(event.message(self.target)) {
            Ok(()) => {
                if event.on {
                    self.active = Some(event.note);
                } else if self.active == Some(event.note) {
                    self.active = None;
                    self.pending = None;
                }
                Ok(())
            }
            Err(err) => {
                if !event.on && self.active == Some(event.note) {
                    self.pending = Some(event);
                }
                Err(err)
            }
        }
    }

    pub fn retry(
        &mut self,
        send: impl FnMut(NoteEventMsg) -> Result<(), RequestError>,
    ) -> Result<(), RequestError> {
        self.pending.map_or(Ok(()), |event| self.send(event, send))
    }

    pub const fn release_waiting(&self) -> bool {
        self.pending.is_some()
    }

    /// Tell the UI which note the driven synth still holds (0 for none,
    /// otherwise the note plus one) and whether its release is waiting.
    pub fn publish(&self, shared: &EngineShared) {
        shared.driven_note.store(
            self.active.map_or(0, |note| u16::from(note) + 1),
            Ordering::Release,
        );
        shared
            .releases_waiting
            .store(self.release_waiting(), Ordering::Release);
    }
}

/// Release the note the driven synth may still hold, once the audio stream
/// has stopped and no new note can start. `Ok(true)` when a release was
/// handed to the link, `Ok(false)` when nothing was held.
///
/// # Errors
///
/// The link's refusal of the release: the driven synth may still hold it.
pub fn release_on_quit(
    shared: &EngineShared,
    target: [u8; 16],
    send: impl FnOnce(NoteEventMsg) -> Result<(), RequestError>,
) -> Result<bool, RequestError> {
    let Some(note) = shared.driven_note.load(Ordering::Acquire).checked_sub(1) else {
        return Ok(false);
    };
    let note = u8::try_from(note).map_err(|_| RequestError::Invalid)?;
    let release = NoteEvent {
        note,
        velocity: 0,
        on: false,
        at: StreamFrame(0),
    };
    send(release.message(target))?;
    shared.driven_note.store(0, Ordering::Release);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{
        ArpCommand, AudioCommand, AudioEngine, COMMAND_CAPACITY, DISPLAY_CAPACITY, EngineShared,
        MAX_CALLBACK_FRAMES,
    };
    use crate::{ClockDivision, parse_args};
    use kazoo_core::host::link::open;
    use kazoo_core::ipc::link::{HubAddress, LinkConfig};
    use kazoo_core::ipc::types::{TRANSPORT_PLAYING, TransportSyncMsg};
    use ringbuf::traits::{Consumer, Split};
    use std::sync::Arc;

    fn rig(name: &str) -> (AudioEngine, crossbeam_channel::Sender<AudioCommand>) {
        let mut config = LinkConfig::new("kazoo-arp-test", 2, 48_000, MAX_CALLBACK_FRAMES as u32);
        config.address = HubAddress::Socket(std::path::PathBuf::from(format!(
            "target/kz-logs/absent-{name}-{}.sock",
            std::process::id()
        )));
        let (hub, audio) = open(config).unwrap();
        drop(hub);
        let (tx, rx) = crossbeam_channel::bounded(COMMAND_CAPACITY);
        let (display, consumer) = ringbuf::HeapRb::new(DISPLAY_CAPACITY).split();
        drop(consumer);
        let mut engine = AudioEngine::new(
            48_000,
            2,
            rx,
            audio,
            display,
            Arc::new(EngineShared::default()),
        );
        tx.send(AudioCommand::Stop).unwrap();
        tx.send(AudioCommand::Arp(ArpCommand::SetDivision(
            ClockDivision::Eighth,
        )))
        .unwrap();
        tx.send(AudioCommand::Arp(ArpCommand::SetGate(0.5)))
            .unwrap();
        for note in [60, 64, 67] {
            tx.send(AudioCommand::Arp(ArpCommand::NoteOn {
                midi_note: note,
                velocity: 100,
            }))
            .unwrap();
        }
        engine.render(&mut []);
        engine.schedule_transport(&TransportSyncMsg {
            state: TRANSPORT_PLAYING,
            bpm: 120.0,
            at_frame: 500,
            beat: 0.0,
        });
        (engine, tx)
    }

    #[test]
    fn each_step_emits_a_note_on_and_off_at_stream_frames() {
        let (mut engine, tx) = rig("steps");
        let mut notes = engine.observe_notes();
        engine.render(&mut vec![0.0; 61_002]);
        let emitted: Vec<_> = notes.pop_iter().collect();
        assert_eq!(
            emitted,
            [
                NoteEvent {
                    note: 60,
                    velocity: 100,
                    on: true,
                    at: StreamFrame(500)
                },
                NoteEvent {
                    note: 60,
                    velocity: 0,
                    on: false,
                    at: StreamFrame(6_500)
                },
                NoteEvent {
                    note: 64,
                    velocity: 100,
                    on: true,
                    at: StreamFrame(12_500)
                },
                NoteEvent {
                    note: 64,
                    velocity: 0,
                    on: false,
                    at: StreamFrame(18_500)
                },
                NoteEvent {
                    note: 67,
                    velocity: 100,
                    on: true,
                    at: StreamFrame(24_500)
                },
                NoteEvent {
                    note: 67,
                    velocity: 0,
                    on: false,
                    at: StreamFrame(30_500)
                },
            ]
        );
        tx.send(AudioCommand::Arp(ArpCommand::SetDivision(
            ClockDivision::Quarter,
        )))
        .unwrap();
        engine.render(&mut vec![0.0; 84_000]);
        let onsets: Vec<_> = notes
            .pop_iter()
            .filter(|event| event.on)
            .map(|event| event.at.0)
            .collect();
        assert_eq!(onsets, [48_500, 72_500]);
    }

    #[test]
    fn latch_holds_notes_while_keys_are_up() {
        let (mut engine, tx) = rig("latch");
        let mut notes = engine.observe_notes();
        tx.send(AudioCommand::Arp(ArpCommand::SetLatch(true)))
            .unwrap();
        for note in [60, 64, 67] {
            tx.send(AudioCommand::Arp(ArpCommand::NoteOff { midi_note: note }))
                .unwrap();
        }
        engine.render(&mut vec![0.0; 49_002]);
        let played: Vec<_> = notes
            .pop_iter()
            .filter(|event| event.on)
            .map(|event| event.note)
            .collect();
        assert_eq!(played, [60, 64, 67]);
    }

    #[test]
    fn target_flag_sets_the_destination() {
        let args = parse_args(["--target", "0102030405060708090a0b0c0d0e0f10"].map(String::from))
            .unwrap()
            .unwrap();
        let destination: [u8; 16] = core::array::from_fn(|index| index as u8 + 1);
        assert_eq!(args.target, destination);
        let event = NoteEvent {
            note: 60,
            velocity: 100,
            on: true,
            at: StreamFrame(500),
        };
        assert_eq!(event.message(destination).target, destination);
        assert_ne!(args, parse_args([]).unwrap().unwrap());
    }

    #[test]
    fn silent_mutes_audio_while_notes_keep_driving() {
        let (mut engine, tx) = rig("silent");
        drop(tx);
        engine.configure_output(parse_args(["--silent".to_owned()]).unwrap().unwrap());
        let mut notes = engine.observe_notes();
        let mut output = vec![f32::NAN; 49_002];
        engine.render(&mut output);
        assert!(output.iter().all(|sample| sample.to_bits() == 0));
        assert_eq!(notes.pop_iter().filter(|event| event.on).count(), 3);
    }

    #[test]
    fn unlatching_releases_the_driven_note() {
        let (mut engine, tx) = rig("unlatch");
        let mut notes = engine.observe_notes();
        tx.send(AudioCommand::Arp(ArpCommand::SetLatch(true)))
            .unwrap();
        engine.render(&mut vec![0.0; 1_002]);
        assert_eq!(notes.pop_iter().filter(|event| event.on).count(), 1);
        tx.send(AudioCommand::Arp(ArpCommand::SetLatch(false)))
            .unwrap();
        engine.render(&mut [0.0; 2]);
        assert_eq!(
            notes.try_pop(),
            Some(NoteEvent {
                note: 60,
                velocity: 0,
                on: false,
                at: StreamFrame(501),
            })
        );
        engine.render(&mut vec![0.0; 48_000]);
        assert!(notes.try_pop().is_none());
    }

    #[test]
    fn desk_stop_releases_on_its_stream_frame() {
        let (mut engine, tx) = rig("stop");
        drop(tx);
        let mut notes = engine.observe_notes();
        engine.schedule_transport(&TransportSyncMsg {
            state: kazoo_core::ipc::types::TRANSPORT_STOPPED,
            bpm: 120.0,
            at_frame: 1_000,
            beat: f64::NAN,
        });
        engine.render(&mut vec![0.0; 24_000]);
        assert_eq!(
            notes.pop_iter().collect::<Vec<_>>(),
            [
                NoteEvent {
                    note: 60,
                    velocity: 100,
                    on: true,
                    at: StreamFrame(500)
                },
                NoteEvent {
                    note: 60,
                    velocity: 0,
                    on: false,
                    at: StreamFrame(1_000)
                },
            ]
        );
    }

    #[test]
    fn a_full_queue_cannot_reorder_a_release_after_a_retrigger() {
        let mut drive = Drive::default();
        let on = NoteEvent {
            note: 60,
            velocity: 100,
            on: true,
            at: StreamFrame(500),
        };
        let off = NoteEvent {
            velocity: 0,
            on: false,
            at: StreamFrame(6_500),
            ..on
        };
        drive.send(on, |_| Ok(())).unwrap();
        assert_eq!(
            drive.send(off, |_| Err(RequestError::Full)),
            Err(RequestError::Full)
        );
        assert!(drive.release_waiting());
        let mut attempted = Vec::new();
        assert_eq!(
            drive.send(on, |event| {
                attempted.push(event.event_type);
                Err(RequestError::NotConnected)
            }),
            Err(RequestError::NotConnected),
            "the refusal names the real cause"
        );
        assert_eq!(attempted, [NOTE_OFF], "no onset passes a waiting release");
        assert!(drive.release_waiting());
        let mut delivered = Vec::new();
        drive
            .send(on, |event| {
                delivered.push(event.event_type);
                Ok(())
            })
            .unwrap();
        assert!(!drive.release_waiting());
        assert_eq!(delivered, [NOTE_OFF, NOTE_ON]);
    }

    #[test]
    fn quitting_releases_the_note_the_synth_still_holds() {
        let shared = EngineShared::default();
        let mut drive = Drive::default();
        let on = NoteEvent {
            note: 60,
            velocity: 100,
            on: true,
            at: StreamFrame(500),
        };
        let off = NoteEvent {
            velocity: 0,
            on: false,
            ..on
        };
        drive.send(on, |_| Ok(())).unwrap();
        assert_eq!(
            drive.send(off, |_| Err(RequestError::Full)),
            Err(RequestError::Full)
        );
        drive.publish(&shared);
        let mut sent = Vec::new();
        assert_eq!(
            release_on_quit(&shared, [7; 16], |message| {
                sent.push(message);
                Ok(())
            }),
            Ok(true)
        );
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].event_type, NOTE_OFF);
        assert_eq!((sent[0].note, sent[0].velocity), (60, 0));
        assert_eq!(sent[0].target, [7; 16]);

        drive.retry(|_| Ok(())).unwrap();
        drive.publish(&shared);
        assert_eq!(
            release_on_quit(&shared, [7; 16], |_| unreachable!("nothing is held")),
            Ok(false)
        );
    }

    #[test]
    fn targets_validate_the_wire_destination_without_broadcast_fallback() {
        assert_eq!(
            parse_args(["--target", "all"].map(String::from))
                .unwrap()
                .unwrap()
                .target,
            [0; 16]
        );
        let compact =
            parse_args(["--target", "0102030405060708090a0b0c0d0e0f10"].map(String::from));
        assert_eq!(
            parse_args(["--target", "01020304-0506-0708-090a-0b0c0d0e0f10"].map(String::from)),
            compact
        );
        for target in [
            "mini",
            "",
            "00000000000000000000000000000000",
            "0102030405060708090a0b0c0d0e0f1g",
            "01020304+0506-0708-090a-0b0c0d0e0f10",
        ] {
            assert!(
                parse_args(["--target", target].map(String::from)).is_err(),
                "{target}"
            );
        }
        assert!(parse_args(["--target".to_owned()]).is_err());
    }
}
