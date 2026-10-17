//! Shared desk connection, callback event drain, and bounded teardown.

use crate::ipc::link::ToAudio;
use crate::ipc::types::MAX_STREAMS;
use std::io;
use std::mem;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use crate::ipc::link::{HubLink, HubLinkAudio, LinkConfig, start_link};
use ringbuf::traits::Consumer;

mod registration;

pub use crate::ipc::client::HubMessage as HubEvent;
pub use crate::ipc::link::VoiceRx;
pub type AudioTx = HubLinkAudio;

const RETRY_DELAY: Duration = Duration::from_millis(250);
const EVENT_LIMIT: usize = 64;

#[derive(Debug, Clone, Copy)]
pub struct LinkSetup {
    pub name: &'static str,
    pub channels: u16,
    pub rate: u32,
    pub block: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no desk answers")]
    NoDesk,
    #[error("invalid desk link setup: {0}")]
    InvalidSetup(&'static str),
    #[error("desk link failed: {0}")]
    Io(#[from] io::Error),
}

pub struct EventRx {
    receiver: Option<ringbuf::HeapCons<ToAudio>>,
    pub(crate) stream_updates: Arc<[AtomicU64; MAX_STREAMS]>,
}

impl std::fmt::Debug for EventRx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventRx").finish_non_exhaustive()
    }
}

impl EventRx {
    pub(crate) fn new(receiver: ringbuf::HeapCons<ToAudio>) -> Self {
        Self {
            receiver: Some(receiver),
            stream_updates: Arc::new(std::array::from_fn(|_| AtomicU64::new(0))),
        }
    }

    pub(crate) fn try_recv(&mut self) -> Option<HubEvent> {
        loop {
            match self.receiver.as_mut()?.try_pop()? {
                ToAudio::Hub(event) => return Some(event),
                ToAudio::Stream { stream, channels } => {
                    let slot = &self.stream_updates[usize::from(stream)];
                    let before = slot.load(Ordering::Relaxed);
                    let generation = (before >> 16).wrapping_add(u64::from(channels.is_none()));
                    slot.store(
                        (generation << 16) | u64::from(channels.unwrap_or(0)),
                        Ordering::Release,
                    );
                }
            }
        }
    }

    const fn empty(stream_updates: Arc<[AtomicU64; MAX_STREAMS]>) -> Self {
        Self {
            receiver: None,
            stream_updates,
        }
    }
}

#[derive(Debug)]
pub struct Linked {
    control: HubLink,
    pub tx: AudioTx,
    pub events: EventRx,
}

impl Linked {
    fn new(control: HubLink, mut tx: AudioTx) -> Self {
        let empty = EventRx::empty(Arc::clone(&tx.events.stream_updates));
        let events = mem::replace(&mut tx.events, empty);
        Self {
            control,
            tx,
            events,
        }
    }

    #[must_use]
    pub fn into_parts(self) -> (HubLink, HubLinkAudio) {
        let Self {
            control,
            mut tx,
            events,
        } = self;
        tx.events = events;
        (control, tx)
    }
}

fn config(setup: &LinkSetup) -> Result<LinkConfig, Error> {
    let channels = u8::try_from(setup.channels)
        .map_err(|_| Error::InvalidSetup("channels must be mono or stereo"))?;
    let block = u32::try_from(setup.block)
        .map_err(|_| Error::InvalidSetup("block exceeds the wire frame count"))?;
    let config = LinkConfig::new(setup.name, channels, setup.rate, block);
    validate(&config)?;
    Ok(config)
}

fn validate(config: &LinkConfig) -> Result<(), Error> {
    if config.name.is_empty() || config.name.len() > 32 {
        return Err(Error::InvalidSetup("name must contain 1 to 32 bytes"));
    }
    if !(1..=2).contains(&config.channels) {
        return Err(Error::InvalidSetup("channels must be mono or stereo"));
    }
    if config.sample_rate == 0 || config.max_block_frames == 0 {
        return Err(Error::InvalidSetup("rate and block must be positive"));
    }
    Ok(())
}

pub fn connect(setup: &LinkSetup) -> Result<Linked, Error> {
    connect_config(config(setup)?)
}

// The wall retains its explicit socket, pacing, and scheduling settings;
// the jam player retains its user-supplied stream name.
pub fn connect_config(config: LinkConfig) -> Result<Linked, Error> {
    connect_with(config, thread::sleep)
}

fn connect_with(config: LinkConfig, mut sleep: impl FnMut(Duration)) -> Result<Linked, Error> {
    validate(&config)?;
    let register = config.register()?;
    for attempt in 0..3 {
        let result = registration::connect(&config.address, &register, RETRY_DELAY);
        match result {
            Ok(client) => {
                let (control, audio) = start_link(config, Some(client), true)?;
                return Ok(Linked::new(control, audio));
            }
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::NotFound
                        | io::ErrorKind::ConnectionRefused
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::WouldBlock
                ) =>
            {
                if attempt < 2 {
                    sleep(RETRY_DELAY);
                }
            }
            Err(err) => return Err(Error::Io(err)),
        }
    }
    Err(Error::NoDesk)
}

pub fn connect_or_direct(setup: &LinkSetup) -> Result<(HubLink, HubLinkAudio), Error> {
    connect_config_or_direct(config(setup)?)
}

/// Read after every [`HubLink`] this process made has dropped, because shutdown
/// updates these counts during drop, after the instrument's screen has gone.
#[must_use]
pub fn exit_report() -> Option<String> {
    exit_report_from(
        crate::ipc::link::stuck_shutdowns(),
        crate::ipc::link::undelivered_completions(),
    )
}

/// The exit line for these counts, or `None` when every link stopped in time.
#[must_use]
pub(crate) fn exit_report_from(stuck: u64, late: u64) -> Option<String> {
    if stuck == 0 && late == 0 {
        return None;
    }
    let plural = if stuck == 1 { "" } else { "s" };
    let report = format!("kazoo: {stuck} desk link{plural} did not stop within 500 ms");
    Some(if late > 0 {
        format!("{report}; {late} finished later")
    } else {
        report
    })
}

// Playing direct keeps the link thread looking, so a desk started after the
// instrument still picks it up, as the link always has.
pub fn connect_config_or_direct(config: LinkConfig) -> Result<(HubLink, HubLinkAudio), Error> {
    match connect_config(config.clone()) {
        Ok(linked) => Ok(linked.into_parts()),
        Err(Error::NoDesk) => {
            let (link, audio) = start_link(config, None, true)?;
            link.report_start_refusal("no desk; playing direct".to_string());
            Ok((link, audio))
        }
        // A desk that refuses the name, or answers badly, is retried by the
        // link with its own backoff and shown as the link's status.
        Err(Error::Io(err)) => {
            let (link, audio) = start_link(config, None, true)?;
            link.report_start_refusal(format!(
                "the desk did not take the link ({err}); playing direct"
            ));
            Ok((link, audio))
        }
        Err(err) => Err(err),
    }
}

pub fn drain_events(rx: &mut EventRx, mut on: impl FnMut(HubEvent)) {
    for _ in 0..EVENT_LIMIT {
        match rx.try_recv() {
            Some(event) => on(event),
            None => break,
        }
    }
}

// Borrow the queue only while collecting so handlers can mutate their engine.
// The batch lives on the stack and never frees or allocates on the callback.
pub fn take_events(rx: &mut EventRx) -> impl Iterator<Item = HubEvent> + use<> {
    let mut events = [const { None }; EVENT_LIMIT];
    let mut count = 0;
    drain_events(rx, |event| {
        events[count] = Some(event);
        count += 1;
    });
    events.into_iter().flatten()
}

/// Visit at most one ring's worth of voice frames in order, without allocating.
pub fn drain_voice(rx: &mut VoiceRx, mut on: impl FnMut(crate::voice::VoiceFrame)) {
    for _ in 0..crate::ipc::link::VOICE_RING {
        match rx.try_recv() {
            Some(frame) => on(frame),
            None => break,
        }
    }
}

#[cfg(test)]
mod tests;
