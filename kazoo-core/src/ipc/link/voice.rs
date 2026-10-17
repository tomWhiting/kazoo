//! Dedicated voice rings, feature negotiation and reconnect wishes.

use std::fmt;
use std::sync::atomic::Ordering;
use std::time::Instant;

use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Observer, Producer, Split};

use super::{
    HubIpcClient, HubLink, HubLinkAudio, LinkState, LinkWorker, Request, RequestError, ToAudio,
    send_request,
};
use crate::ipc::client::{HubMessage, HubRead};
use crate::ipc::outbox::OutboxError;
use crate::ipc::protocol::HEADER_SIZE;
use crate::ipc::types::{FEATURE_VOICE, NO_STRIP};
use crate::ipc::voice::{VOICE_BATCH_MAX, VOICE_BATCH_WIRE, decode_batch, encode_batch};
use crate::voice::VoiceFrame;

/// Frames retained in each direction, independently of notes and transport.
pub const VOICE_RING: usize = 256;

/// The callback's receive half of the voice ring.
pub struct VoiceRx {
    receiver: ringbuf::HeapCons<VoiceFrame>,
}

impl fmt::Debug for VoiceRx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VoiceRx").finish_non_exhaustive()
    }
}

impl VoiceRx {
    pub(crate) fn try_recv(&mut self) -> Option<VoiceFrame> {
        self.receiver.try_pop()
    }
}

/// Frame counters over every connection of this link.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VoiceLinkStats {
    /// Frames in whole batches written to the desk's socket.
    pub sent: u64,
    /// Frames refused by the callback or lost before delivery.
    pub dropped: u64,
    /// Valid forwarded frames, including receive-ring overflow.
    pub received: u64,
    /// Forwarded frames that did not fit the callback's receive ring.
    pub overflow: u64,
}

struct PendingBatch {
    end: u64,
    count: u64,
}

pub(super) struct VoiceWorker {
    out: ringbuf::HeapCons<VoiceFrame>,
    incoming: ringbuf::HeapProd<VoiceFrame>,
    pending: Option<PendingBatch>,
}

pub(super) fn rings() -> (ringbuf::HeapProd<VoiceFrame>, VoiceRx, VoiceWorker) {
    let (out_tx, out) = HeapRb::new(VOICE_RING).split();
    let (incoming, receiver) = HeapRb::new(VOICE_RING).split();
    (
        out_tx,
        VoiceRx { receiver },
        VoiceWorker {
            out,
            incoming,
            pending: None,
        },
    )
}

impl HubLinkAudio {
    /// Queue one voice frame without allocating, locking or doing socket I/O.
    /// A refusal is counted and returns false. Waking the link uses `unpark`.
    pub fn send_voice(&mut self, frame: &VoiceFrame) -> bool {
        if !self.is_connected()
            || self.state.desk_features.load(Ordering::Acquire) & FEATURE_VOICE == 0
            || self.voice_out.try_push(*frame).is_err()
        {
            self.state.voice_dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        self.waker.unpark();
        true
    }

    /// Whether the desk is connected and carries the voice: only then is a
    /// frame [`Self::send_voice`] refuses a frame the desk had no room for.
    /// No allocation, lock or I/O.
    #[must_use]
    pub fn carries_voice(&self) -> bool {
        self.is_connected() && self.state.desk_features.load(Ordering::Acquire) & FEATURE_VOICE != 0
    }
}

impl HubLink {
    /// Keep the follow wish across disconnections and send it when supported.
    /// The wish is kept even when the request queue is full or the desk is old.
    pub fn follow_voice(&self, on: bool) -> Result<(), RequestError> {
        self.state.follow_voice.store(on, Ordering::Release);
        if !self.is_connected() {
            return Ok(());
        }
        if !self.desk_carries_voice() {
            return Err(RequestError::Unsupported);
        }
        self.request(Request::VoiceFollow(on))
    }

    /// Whether the connected desk announced that it carries voice.
    #[must_use]
    pub fn desk_carries_voice(&self) -> bool {
        self.is_connected() && self.state.desk_features.load(Ordering::Acquire) & FEATURE_VOICE != 0
    }

    /// Voice counters, independent of the ordinary message counters.
    #[must_use]
    pub fn voice_stats(&self) -> VoiceLinkStats {
        VoiceLinkStats {
            sent: self.state.voice_sent.load(Ordering::Relaxed),
            dropped: self.state.voice_dropped.load(Ordering::Relaxed),
            received: self.state.voice_received.load(Ordering::Relaxed),
            overflow: self.state.voice_overflow.load(Ordering::Relaxed),
        }
    }
}

impl VoiceWorker {
    fn settle(&mut self, completed: u64, state: &LinkState) {
        // The difference also works when the byte counter wraps. A bounded
        // outbox can never be half a u64 behind its last batch boundary.
        if self
            .pending
            .as_ref()
            .is_some_and(|batch| completed.wrapping_sub(batch.end) < (1_u64 << 63))
            && let Some(batch) = self.pending.take()
        {
            state.voice_sent.fetch_add(batch.count, Ordering::Relaxed);
        }
    }

    pub(super) fn discard(&mut self, client: Option<&HubIpcClient>, state: &LinkState) {
        if let Some(client) = client {
            self.settle(client.written_bytes(), state);
        }
        let mut queued = 0_u64;
        for _ in 0..VOICE_RING {
            if self.out.try_pop().is_none() {
                break;
            }
            queued += 1;
        }
        let pending = self.pending.take().map_or(0, |batch| batch.count);
        state
            .voice_dropped
            .fetch_add(queued.saturating_add(pending), Ordering::Relaxed);
    }

    pub(super) fn send(
        &mut self,
        client: &mut HubIpcClient,
        state: &LinkState,
        stuck_since: &mut Option<Instant>,
    ) -> Result<(), String> {
        self.settle(client.written_bytes(), state);
        if state.desk_features.load(Ordering::Acquire) & FEATURE_VOICE == 0
            || self.pending.is_some()
        {
            return Ok(());
        }
        // Snapshot the backlog so a busy producer cannot hold up hub reads.
        let mut left = self.out.occupied_len().min(VOICE_RING);
        let mut payload = [0; VOICE_BATCH_WIRE];
        while left > 0 {
            let Some(first) = self.out.try_pop() else {
                break;
            };
            let mut frames = [first; VOICE_BATCH_MAX];
            let mut count = 1;
            left -= 1;
            while count < VOICE_BATCH_MAX && left > 0 {
                let Some(frame) = self.out.try_pop() else {
                    break;
                };
                frames[count] = frame;
                count += 1;
                left -= 1;
            }
            let len = encode_batch(&frames[..count], &mut payload);
            let count = u64::try_from(count)
                .map_err(|err| format!("voice batch frame count failed: {err}"))?;
            if len == 0 {
                state.voice_dropped.fetch_add(count, Ordering::Relaxed);
                return Err("voice batch encoder refused its fixed buffer".to_string());
            }
            let before = client.pending_bytes();
            let bytes = u64::try_from(before + HEADER_SIZE + len)
                .map_err(|err| format!("voice batch byte count failed: {err}"))?;
            self.pending = Some(PendingBatch {
                end: client.written_bytes().wrapping_add(bytes),
                count,
            });
            match client.send_voice_batch(&payload[..len]) {
                Ok(()) => {
                    if client.pending_bytes() < before + HEADER_SIZE + len {
                        *stuck_since = None;
                    }
                    self.settle(client.written_bytes(), state);
                    // At most one voice batch waits in the outbox, leaving
                    // its remaining capacity for notes and transport requests.
                    if self.pending.is_some() {
                        break;
                    }
                }
                Err(OutboxError::Stuck) => {
                    self.pending = None;
                    state.voice_dropped.fetch_add(count, Ordering::Relaxed);
                    break;
                }
                Err(OutboxError::TooLarge) => {
                    return Err("voice batch is too large for the desk".to_string());
                }
                Err(OutboxError::Io(err)) => {
                    return Err(format!("sending voice to the desk failed: {err}"));
                }
            }
        }
        Ok(())
    }

    fn receive(&mut self, payload: &[u8], state: &LinkState) -> Result<(), String> {
        let mut overflow = 0_u64;
        let mut received = 0_u64;
        let mut full = false;
        decode_batch(payload, |frame| {
            received += 1;
            if full || self.incoming.try_push(frame).is_err() {
                full = true;
                overflow += 1;
            }
        })
        .map_err(|err| format!("the desk sent a malformed voice batch: {err}"))?;
        state.voice_received.fetch_add(received, Ordering::Relaxed);
        state.voice_overflow.fetch_add(overflow, Ordering::Relaxed);
        Ok(())
    }
}

impl LinkWorker {
    pub(super) fn read_hub(&mut self) -> Result<(), String> {
        let Some(client) = self.client.as_mut() else {
            return Ok(());
        };
        self.voice.settle(client.written_bytes(), &self.state);
        loop {
            match client.try_read() {
                Ok(Some(HubRead::Voice(payload))) => self.voice.receive(payload, &self.state)?,
                Ok(Some(HubRead::DeskFeatures)) => {
                    if let Some(features) = client.take_desk_features() {
                        self.state.desk_features.store(features, Ordering::Release);
                        if features & FEATURE_VOICE != 0
                            && self.state.follow_voice.load(Ordering::Acquire)
                        {
                            let source = *client.instrument_id();
                            send_request(
                                client,
                                Request::VoiceFollow(true),
                                source,
                                &self.state,
                                &mut self.messages,
                                &mut self.stuck_since,
                            )?;
                        }
                    }
                }
                Ok(Some(HubRead::Message(HubMessage::Shutdown))) => {
                    return Err("the desk closed".to_string());
                }
                Ok(Some(HubRead::Message(message))) => {
                    if let HubMessage::StreamReady { stream, strip } = &message {
                        let id = usize::from(*stream);
                        if self.state.stream_live.load(Ordering::Acquire) & (1 << id) != 0 {
                            self.state.strips[id]
                                .store(strip.unwrap_or(NO_STRIP), Ordering::Release);
                        }
                        if let Some(reason) = client.take_stream_refusal() {
                            match self.state.last_stream_refusal.lock() {
                                Ok(mut slot) => *slot = Some(reason),
                                Err(poisoned) => *poisoned.into_inner() = Some(reason),
                            }
                        }
                    }
                    if self.messages.try_push(ToAudio::Hub(message)).is_err() {
                        self.state.messages_dropped.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Ok(None) => {
                    if let Some(pace) = client.take_desk_pace() {
                        self.state.pace.store(Some((pace, Instant::now())));
                    }
                    if let Some(health) = client.take_desk_health() {
                        self.health_now = health;
                        self.publish_health();
                    }
                    return Ok(());
                }
                Err(err) => return Err(format!("the desk connection failed: {err}")),
            }
        }
    }
}

pub(super) fn send_follow(
    client: &mut HubIpcClient,
    state: &LinkState,
    on: bool,
) -> Result<(), OutboxError> {
    // A queued request may outlive its connection. The current desk must
    // advertise voice before it can receive this message type.
    if state.desk_features.load(Ordering::Acquire) & FEATURE_VOICE == 0 {
        return Ok(());
    }
    client.send_voice_follow(on)
}

#[cfg(test)]
mod tests;
