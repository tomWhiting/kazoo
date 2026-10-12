//! Shared desk connection, callback event drain, and bounded teardown.

use std::io;
use std::mem;
use std::thread;
use std::time::Duration;

use crate::ipc::client::HubIpcClient;
use crate::ipc::discovery;
use crate::ipc::follow::TransportFollower;
use crate::ipc::link::{HubAddress, HubLink, HubLinkAudio, LinkConfig, start_link};
use crate::ipc::types::RegisterMsg;
use ringbuf::traits::{Consumer, Split};

pub use crate::ipc::client::HubMessage as HubEvent;
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
    receiver: ringbuf::HeapCons<HubEvent>,
}

impl std::fmt::Debug for EventRx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventRx").finish_non_exhaustive()
    }
}

impl EventRx {
    pub(crate) const fn new(receiver: ringbuf::HeapCons<HubEvent>) -> Self {
        Self { receiver }
    }

    pub(crate) fn try_recv(&mut self) -> Option<HubEvent> {
        self.receiver.try_pop()
    }

    fn empty() -> Self {
        let (sender, receiver) = ringbuf::HeapRb::new(1).split();
        drop(sender);
        Self::new(receiver)
    }
}

#[derive(Debug)]
pub struct Linked {
    pub tx: AudioTx,
    pub events: EventRx,
    pub transport: TransportFollower,
    control: HubLink,
}

impl Linked {
    fn new(control: HubLink, mut tx: AudioTx, rate: u32) -> Self {
        let events = mem::replace(&mut tx.events, EventRx::empty());
        Self {
            tx,
            events,
            transport: TransportFollower::new(rate),
            control,
        }
    }

    // Existing engines keep their combined callback handle and UI handle.
    // Replacements are built here, before either render thread starts.
    pub fn into_parts(mut self) -> Result<(HubLink, HubLinkAudio), Error> {
        let (replacement_control, replacement_audio) =
            start_link(LinkConfig::new("detached", 1, 1, 1), None, false)?;
        let control = mem::replace(&mut self.control, replacement_control);
        let mut audio = mem::replace(&mut self.tx, replacement_audio);
        audio.events = mem::replace(&mut self.events, EventRx::empty());
        Ok((control, audio))
    }
}

impl Drop for Linked {
    fn drop(&mut self) {
        self.control.shutdown();
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
    let mut register = RegisterMsg::new(
        &config.name,
        config.channels,
        config.sample_rate,
        config
            .max_block_frames
            .min(crate::ipc::link::MAX_LINK_BLOCK_FRAMES),
    );
    register.pace_lead_frames = config.pace_lead_frames;
    for _ in 0..3 {
        let socket = match &config.address {
            HubAddress::Discover => discovery::hub_socket(),
            HubAddress::Socket(path) => Ok(path.clone()),
        };
        let result = socket
            .and_then(|path| HubIpcClient::register_with_timeout(&path, &register, RETRY_DELAY));
        match result {
            Ok(client) => {
                let rate = config.sample_rate;
                let (control, audio) = start_link(config, Some(client), true)?;
                return Ok(Linked::new(control, audio, rate));
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
                sleep(RETRY_DELAY);
            }
            Err(err) => return Err(Error::Io(err)),
        }
    }
    Err(Error::NoDesk)
}

pub fn connect_or_direct(setup: &LinkSetup) -> Result<(HubLink, HubLinkAudio), Error> {
    match connect(setup) {
        Ok(linked) => linked.into_parts(),
        Err(Error::NoDesk) => {
            eprintln!("{}: no desk; playing direct", setup.name);
            start_link(config(setup)?, None, false).map_err(Error::Io)
        }
        Err(err) => Err(err),
    }
}

pub fn connect_config_or_direct(config: LinkConfig) -> Result<(HubLink, HubLinkAudio), Error> {
    match connect_config(config.clone()) {
        Ok(linked) => linked.into_parts(),
        Err(Error::NoDesk) => {
            eprintln!("{}: no desk; playing direct", config.name);
            start_link(config, None, false).map_err(Error::Io)
        }
        Err(err) => Err(err),
    }
}

// Asynchronous openings let socket fixtures accept registration after setup.
pub fn open(config: LinkConfig) -> io::Result<(HubLink, HubLinkAudio)> {
    crate::ipc::link::hub_link(config)
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

#[cfg(test)]
mod tests;
