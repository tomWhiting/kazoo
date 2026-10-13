//! UI stream changes and registration snapshots; never used by the callback.

use super::{
    Arc, Duration, HubAddress, HubIpcClient, HubLink, HubMessage, LinkConfig, LinkWorker,
    MAX_LINK_BLOCK_FRAMES, MAX_RETRY_INTERVAL, MAX_STREAMS, NO_STRIP, Ordering, Producer,
    RETRY_INTERVAL, RegisterMsg, Request, RequestError, STREAM_NAME_LEN, SYNC_NOW, StreamAddMsg,
    StreamId, StreamSpec, ToAudio, TransportSyncMsg, discovery, io,
};
use std::sync::Mutex;

/// One named output configured before the link starts.
#[derive(Debug, Clone)]
pub struct StreamConfig {
    /// Non-empty UTF-8 name of at most 24 bytes.
    pub name: String,
    /// 1 for mono, 2 for stereo.
    pub channels: u8,
}

impl StreamConfig {
    fn spec(&self) -> StreamSpec {
        StreamSpec::new(&self.name, self.channels)
    }
}

impl LinkConfig {
    pub(crate) fn resolved_streams(&self) -> io::Result<[Option<StreamConfig>; MAX_STREAMS]> {
        let configured = if self.streams.is_empty() {
            vec![StreamConfig {
                name: "main".to_string(),
                channels: self.channels,
            }]
        } else {
            self.streams.clone()
        };
        if configured.len() > MAX_STREAMS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "too many streams",
            ));
        }
        let mut slots = std::array::from_fn(|_| None);
        for (index, stream) in configured.into_iter().enumerate() {
            if !valid_name(&stream.name, stream.channels)
                || slots[..index]
                    .iter()
                    .flatten()
                    .any(|other: &StreamConfig| other.name == stream.name)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid or duplicate stream",
                ));
            }
            slots[index] = Some(stream);
        }
        Ok(slots)
    }

    pub(crate) fn register(&self) -> io::Result<RegisterMsg> {
        let streams = self.resolved_streams()?;
        Ok(self.register_streams(&streams))
    }

    fn register_streams(&self, streams: &[Option<StreamConfig>; MAX_STREAMS]) -> RegisterMsg {
        let mut register = RegisterMsg::new(
            &self.name,
            self.channels,
            self.sample_rate,
            self.max_block_frames.min(MAX_LINK_BLOCK_FRAMES),
        );
        register.pace_lead_frames = self.pace_lead_frames;
        register.streams = streams
            .iter()
            .take_while(|slot| slot.is_some())
            .flatten()
            .map(StreamConfig::spec)
            .collect();
        register.channel_count = register.streams[0].channels;
        register
    }
}

fn valid_name(name: &str, channels: u8) -> bool {
    !name.is_empty()
        && name.len() <= STREAM_NAME_LEN
        && !name.as_bytes().contains(&0)
        && (1..=2).contains(&channels)
}

impl HubLink {
    /// Add a stream using the lowest available nonzero id. UI thread only.
    pub fn add_stream(&self, name: &str, channels: u8) -> Result<StreamId, RequestError> {
        if !valid_name(name, channels) {
            return Err(RequestError::Invalid);
        }
        let mut streams = self
            .state
            .streams
            .lock()
            .map_err(|_| RequestError::Invalid)?;
        if streams.iter().flatten().any(|stream| stream.name == name) {
            return Err(RequestError::Invalid);
        }
        let id = (1..MAX_STREAMS)
            .find(|id| streams[*id].is_none())
            .ok_or(RequestError::Full)?;
        let stream = id as StreamId;
        self.state.stream_live.fetch_or(1 << id, Ordering::Release);
        if self.is_connected() {
            if let Err(err) = self.request(Request::StreamAdd {
                stream,
                spec: StreamSpec::new(name, channels),
            }) {
                self.state
                    .stream_live
                    .fetch_and(!(1 << id), Ordering::Release);
                drop(streams);
                return Err(err);
            }
        }
        streams[id] = Some(StreamConfig {
            name: name.to_string(),
            channels,
        });
        drop(streams);
        Ok(stream)
    }

    /// Remove a nonzero stream. UI thread only.
    pub fn remove_stream(&self, stream: StreamId) -> Result<(), RequestError> {
        let id = usize::from(stream);
        if id == 0 || id >= MAX_STREAMS {
            return Err(RequestError::Invalid);
        }
        let mut streams = self
            .state
            .streams
            .lock()
            .map_err(|_| RequestError::Invalid)?;
        if streams[id].is_none() {
            return Err(RequestError::Invalid);
        }
        if self.is_connected() {
            self.request(Request::StreamRemove { stream })?;
        }
        self.state
            .stream_live
            .fetch_and(!(1 << id), Ordering::Release);
        self.state.strips[id].store(NO_STRIP, Ordering::Release);
        streams[id] = None;
        drop(streams);
        Ok(())
    }

    /// The desk strip assigned to a stream, if seated.
    #[must_use]
    pub fn stream_strip(&self, stream: StreamId) -> Option<u8> {
        let strip = self
            .state
            .strips
            .get(usize::from(stream))?
            .load(Ordering::Acquire);
        (strip != NO_STRIP).then_some(strip)
    }

    /// The latest refusal of one stream; this does not mean the link is down.
    #[must_use]
    pub fn last_stream_refusal(&self) -> Option<(StreamId, String)> {
        match self.state.last_stream_refusal.lock() {
            Ok(reason) => reason.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

impl LinkWorker {
    pub(super) fn try_connect(&mut self) -> Duration {
        let socket = match &self.config.address {
            HubAddress::Discover => match discovery::hub_socket() {
                Ok(socket) => socket,
                Err(err) => {
                    self.set_refusal(Some(format!("cannot find the desk: {err}")));
                    return RETRY_INTERVAL;
                }
            },
            HubAddress::Socket(path) => path.clone(),
        };
        // Hold the snapshot until connected is published so UI additions cannot
        // fall between registration and the connected request queue.
        let state = Arc::clone(&self.state);
        let streams = match state.streams.lock() {
            Ok(streams) => streams,
            Err(err) => {
                self.set_refusal(Some(format!("stream table is poisoned: {err}")));
                return RETRY_INTERVAL;
            }
        };
        // add_stream and remove_stream change the table and queue their
        // request under this lock, so every stream request queued now is
        // in the registration below; sent after it, a desk would refuse an
        // add for a stream it has seated, or remove one added again since.
        self.discard_backlog();
        let register = self.config.register_streams(&streams);
        match HubIpcClient::register_with_timeout(&socket, &register, Duration::from_millis(250)) {
            Ok(mut client) => {
                // Registration implies contiguous ids. Surviving ids beyond a
                // removed slot are re-announced explicitly to preserve their ids.
                for (id, spec) in streams
                    .iter()
                    .enumerate()
                    .skip(register.streams.len())
                    .filter_map(|(id, slot)| slot.as_ref().map(|slot| (id, slot.spec())))
                {
                    if let Err(err) = client.send_stream_add(&StreamAddMsg {
                        stream: id as StreamId,
                        spec,
                    }) {
                        self.set_refusal(Some(format!("could not restore stream {id}: {err}")));
                        return RETRY_INTERVAL;
                    }
                }
                self.plug_in(client);
                for (id, slot) in streams.iter().enumerate() {
                    if self
                        .messages
                        .try_push(ToAudio::Stream {
                            stream: id as StreamId,
                            channels: slot.as_ref().map(|slot| u16::from(slot.channels)),
                        })
                        .is_err()
                    {
                        self.disconnect(
                            "callback stream queue is full while connecting".to_string(),
                        );
                        break;
                    }
                }
                drop(streams);
                RETRY_INTERVAL
            }
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) =>
            {
                self.refusal_delay = RETRY_INTERVAL;
                RETRY_INTERVAL
            }
            Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
                self.set_refusal(Some(format!(
                    "the desk refused {}: {err}",
                    self.config.name
                )));
                self.refusal_delay = (self.refusal_delay * 2).min(MAX_RETRY_INTERVAL);
                self.refusal_delay
            }
            Err(err) => {
                self.set_refusal(Some(format!(
                    "could not plug into the desk at {}: {err}",
                    socket.display()
                )));
                RETRY_INTERVAL
            }
        }
    }

    pub(super) fn plug_in(&mut self, client: HubIpcClient) {
        let registration = client.registration();
        let sync = TransportSyncMsg {
            state: registration.transport_state,
            bpm: registration.bpm,
            at_frame: SYNC_NOW,
            beat: f64::NAN,
        };
        if self
            .messages
            .try_push(ToAudio::Hub(HubMessage::TransportSync(sync)))
            .is_err()
        {
            self.state.messages_dropped.fetch_add(1, Ordering::Relaxed);
        }
        for (id, strip) in self.state.strips.iter().enumerate() {
            strip.store(
                registration.strips.get(id).copied().unwrap_or(NO_STRIP),
                Ordering::Release,
            );
        }
        self.state.connections.fetch_add(1, Ordering::Relaxed);
        self.set_refusal(None);
        self.refusal_delay = RETRY_INTERVAL;
        self.audio_frame_left = 0;
        self.stuck_since = None;
        self.client = Some(client);
        self.state.connected.store(true, Ordering::Release);
    }
}

pub(super) const fn table(
    slots: [Option<StreamConfig>; MAX_STREAMS],
) -> Mutex<[Option<StreamConfig>; MAX_STREAMS]> {
    Mutex::new(slots)
}
