//! Patching sources into the running engine.
//!
//! The engine lives inside the audio callback, so sources can only be plugged
//! in or pulled out from there. The patchbay is the lock-free path for that:
//! the hub thread queues [`PatchCommand`]s, the callback applies them between
//! renders, and every command comes back as exactly one [`PatchEvent`]
//! carrying anything the callback must not free (a replaced or refused ring),
//! so no memory is ever released on the audio thread.
//!
//! Both queues are fixed-capacity SPSC rings allocated up front. The hub side
//! never has more commands in flight than the event ring can hold, and the
//! callback only takes a command when it has room to answer it, so an answer
//! can never be refused.
//!
//! The same path carries the other way every transport change the callback
//! schedules ([`SongAnchor`]), for the hub to pass on to the instruments.

use kazoo_core::audio_transport::AudioBlockConsumer;
use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Observer, Producer, Split};

use crate::engine::{MixerEngine, MixerEngineError, NAME_BYTES};
use crate::song::SongAnchor;

/// Transport changes the callback can have waiting for the hub. The callback
/// schedules at most one per device buffer and the hub polls far more often.
pub const SONG_BACKLOG: usize = 64;

/// A change to what feeds the desk.
#[derive(Debug)]
pub enum PatchCommand {
    /// Plug a source into a strip.
    Attach {
        /// Desk strip.
        slot: usize,
        /// Name shown on the strip.
        name: [u8; NAME_BYTES],
        /// The source's ring.
        consumer: AudioBlockConsumer,
    },
    /// Pull whatever feeds a strip.
    Detach {
        /// Desk strip.
        slot: usize,
    },
}

/// The callback's answer to one [`PatchCommand`].
#[derive(Debug)]
pub enum PatchEvent {
    /// The source is playing on `slot`.
    Attached {
        /// Desk strip.
        slot: usize,
        /// The ring the strip had before, now unplugged.
        previous: Option<AudioBlockConsumer>,
    },
    /// The engine refused the source.
    Refused {
        /// Desk strip.
        slot: usize,
        /// Why.
        error: MixerEngineError,
        /// The ring that was offered, handed back.
        consumer: AudioBlockConsumer,
    },
    /// The strip is empty.
    Detached {
        /// Desk strip.
        slot: usize,
        /// The ring that was unplugged, if any; `Err` if the slot does not
        /// exist.
        consumer: Result<Option<AudioBlockConsumer>, MixerEngineError>,
    },
}

/// Create a patchbay that can have `capacity` commands in flight.
#[must_use]
pub fn patchbay(capacity: usize) -> (PatchbayHub, PatchbayCallback) {
    let capacity = capacity.max(1);
    let (command_tx, command_rx) = HeapRb::<PatchCommand>::new(capacity).split();
    let (event_tx, event_rx) = HeapRb::<PatchEvent>::new(capacity).split();
    let (song_tx, song_rx) = HeapRb::<SongAnchor>::new(SONG_BACKLOG).split();
    (
        PatchbayHub {
            commands: command_tx,
            events: event_rx,
            songs: song_rx,
            in_flight: 0,
            capacity,
        },
        PatchbayCallback {
            commands: command_rx,
            events: event_tx,
            songs: song_tx,
        },
    )
}

/// The hub's end: queue commands, collect answers.
pub struct PatchbayHub {
    commands: ringbuf::HeapProd<PatchCommand>,
    events: ringbuf::HeapCons<PatchEvent>,
    songs: ringbuf::HeapCons<SongAnchor>,
    in_flight: usize,
    capacity: usize,
}

impl std::fmt::Debug for PatchbayHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PatchbayHub")
            .field("in_flight", &self.in_flight)
            .field("capacity", &self.capacity)
            .finish_non_exhaustive()
    }
}

impl PatchbayHub {
    /// Queue a command for the callback.
    ///
    /// # Errors
    ///
    /// Hands the command back if `capacity` commands are already awaiting
    /// answers; collect events and try again.
    pub fn send(&mut self, command: PatchCommand) -> Result<(), PatchCommand> {
        if self.in_flight >= self.capacity {
            return Err(command);
        }
        self.commands.try_push(command)?;
        self.in_flight += 1;
        Ok(())
    }

    /// Take the next answer from the callback, if one has arrived.
    pub fn next_event(&mut self) -> Option<PatchEvent> {
        let event = self.events.try_pop()?;
        self.in_flight = self.in_flight.saturating_sub(1);
        Some(event)
    }

    /// Take the next transport change the callback scheduled, if any.
    pub fn next_song(&mut self) -> Option<SongAnchor> {
        self.songs.try_pop()
    }

    /// Commands sent and not yet answered.
    #[must_use]
    pub const fn in_flight(&self) -> usize {
        self.in_flight
    }
}

/// The callback's end: apply queued commands to the engine.
pub struct PatchbayCallback {
    commands: ringbuf::HeapCons<PatchCommand>,
    events: ringbuf::HeapProd<PatchEvent>,
    songs: ringbuf::HeapProd<SongAnchor>,
}

impl std::fmt::Debug for PatchbayCallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PatchbayCallback")
            .field("queued", &self.commands.occupied_len())
            .finish_non_exhaustive()
    }
}

impl PatchbayCallback {
    /// Apply queued commands to `engine`. Real-time safe: no allocation, no
    /// freeing, no locks.
    ///
    /// A command is only taken when its answer is sure to fit, so a queue
    /// that is momentarily full just waits for the next callback.
    pub fn service(&mut self, engine: &mut MixerEngine) -> PatchReport {
        let mut report = PatchReport::default();
        while self.events.vacant_len() > 0 {
            let Some(command) = self.commands.try_pop() else {
                break;
            };
            let event = apply(engine, command);
            // Room was checked above and this is the only producer, so the
            // push succeeds; the hub also never has more commands in flight
            // than the event ring holds.
            match self.events.try_push(event) {
                Ok(()) => report.applied += 1,
                Err(event) => {
                    // Unreachable by the invariant above; reported so the
                    // desk shows it. The ring is leaked rather than freed on
                    // the audio thread.
                    std::mem::forget(event);
                    report.stranded += 1;
                }
            }
        }
        report
    }
}

impl PatchbayCallback {
    /// Tell the hub about a scheduled transport change. Real-time safe.
    ///
    /// # Errors
    ///
    /// Hands the anchor back when [`SONG_BACKLOG`] changes are already
    /// waiting (the hub has stopped reading).
    pub fn announce(&mut self, anchor: SongAnchor) -> Result<(), SongAnchor> {
        self.songs.try_push(anchor)
    }
}

/// What one [`PatchbayCallback::service`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PatchReport {
    /// Commands applied and answered.
    pub applied: usize,
    /// Answers that could not be returned (a broken invariant, never
    /// expected).
    pub stranded: usize,
}

fn apply(engine: &mut MixerEngine, command: PatchCommand) -> PatchEvent {
    match command {
        PatchCommand::Attach {
            slot,
            name,
            consumer,
        } => match engine.attach_consumer(slot, name, consumer) {
            Ok(previous) => PatchEvent::Attached { slot, previous },
            Err(rejected) => PatchEvent::Refused {
                slot,
                error: rejected.error,
                consumer: rejected.consumer,
            },
        },
        PatchCommand::Detach { slot } => PatchEvent::Detached {
            slot,
            consumer: engine.detach_consumer(slot),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::short_name;
    use kazoo_core::audio_transport::{AudioRingConfig, audio_block_ring};
    use kazoo_core::protocol::BufferId;

    fn consumer() -> AudioBlockConsumer {
        audio_block_ring(AudioRingConfig::new(BufferId(1), 2, 64, 4)).1
    }

    #[test]
    fn attach_and_detach_round_trip_through_the_callback() {
        let (mut hub, mut callback) = patchbay(4);
        let mut engine = MixerEngine::new(2, 48_000).unwrap();
        hub.send(PatchCommand::Attach {
            slot: 1,
            name: short_name("mini"),
            consumer: consumer(),
        })
        .unwrap();
        assert!(hub.next_event().is_none());
        assert_eq!(callback.service(&mut engine).applied, 1);
        assert!(matches!(
            hub.next_event(),
            Some(PatchEvent::Attached {
                slot: 1,
                previous: None
            })
        ));
        assert!(engine.channel_snapshots()[1].connected);

        hub.send(PatchCommand::Detach { slot: 1 }).unwrap();
        assert_eq!(callback.service(&mut engine).applied, 1);
        assert!(matches!(
            hub.next_event(),
            Some(PatchEvent::Detached {
                slot: 1,
                consumer: Ok(Some(_))
            })
        ));
        assert!(!engine.channel_snapshots()[1].connected);
        assert_eq!(hub.in_flight(), 0);
    }

    #[test]
    fn refused_sources_come_back_to_the_hub() {
        let (mut hub, mut callback) = patchbay(4);
        let mut engine = MixerEngine::new(2, 48_000).unwrap();
        hub.send(PatchCommand::Attach {
            slot: 9,
            name: short_name("x"),
            consumer: consumer(),
        })
        .unwrap();
        assert_eq!(callback.service(&mut engine).applied, 1);
        assert!(matches!(
            hub.next_event(),
            Some(PatchEvent::Refused {
                slot: 9,
                error: MixerEngineError::InvalidSlot { slot: 9 },
                ..
            })
        ));
    }

    #[test]
    fn commands_beyond_capacity_wait_for_answers() {
        let (mut hub, mut callback) = patchbay(2);
        let mut engine = MixerEngine::new(4, 48_000).unwrap();
        for slot in 0..2 {
            hub.send(PatchCommand::Detach { slot }).unwrap();
        }
        assert!(matches!(
            hub.send(PatchCommand::Detach { slot: 2 }),
            Err(PatchCommand::Detach { slot: 2 })
        ));
        assert_eq!(callback.service(&mut engine).applied, 2);
        // Answers not yet collected: still no room.
        assert!(hub.send(PatchCommand::Detach { slot: 2 }).is_err());
        assert!(hub.next_event().is_some());
        hub.send(PatchCommand::Detach { slot: 2 }).unwrap();
    }
}
