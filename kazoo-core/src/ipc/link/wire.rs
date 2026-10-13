//! Writes borrowed ring bytes while preserving the boundary of each frame.

use std::io;
use std::time::Instant;

use ringbuf::traits::Consumer;

use super::{HubIpcClient, LINK_BACKLOG_MS, MAX_STREAMS, STUCK_LIMIT};
use crate::ipc::protocol::{AUDIO_STREAM_PAYLOAD_HEADER, FrameHeader, HEADER_SIZE, decode_header};

/// Bytes that hold [`LINK_BACKLOG_MS`] of `streams` streams carrying
/// `channels` channels between them, with a frame head (the larger, named
/// one) for every 64 frames of each stream, and never less than two of the
/// largest blocks one stream can send.
pub(super) fn link_ring_bytes(
    sample_rate: u32,
    channels: u16,
    streams: usize,
    block_frames: u32,
) -> usize {
    let frames = u64::from(sample_rate) * LINK_BACKLOG_MS / 1000;
    let headers =
        frames.div_ceil(64) * streams as u64 * (HEADER_SIZE + AUDIO_STREAM_PAYLOAD_HEADER) as u64;
    let bytes = frames * u64::from(channels) * 4 + headers;
    let largest = HEADER_SIZE
        + AUDIO_STREAM_PAYLOAD_HEADER
        + block_frames as usize * usize::from(channels.min(2)) * 4;
    (bytes as usize).max(2 * largest)
}

/// The ring a link allocates: room for every stream it may ever carry,
/// all stereo, since streams are added while the callback runs and the
/// ring cannot grow then. What may wait in it at once is held to the
/// streams there are now ([`link_ring_bytes`] of them), so the backlog
/// stays bounded by time.
pub(super) fn link_ring_capacity(sample_rate: u32, block_frames: u32) -> usize {
    link_ring_bytes(
        sample_rate,
        2 * MAX_STREAMS as u16,
        MAX_STREAMS,
        block_frames,
    )
}

pub(super) fn check_stuck(stuck_since: &mut Option<Instant>) -> Result<(), String> {
    let now = Instant::now();
    let since = *stuck_since.get_or_insert(now);
    if now.duration_since(since) > STUCK_LIMIT {
        return Err(format!(
            "the desk stopped taking audio for over {} ms",
            STUCK_LIMIT.as_millis()
        ));
    }
    Ok(())
}

pub(super) fn write_wire(
    wire: &mut ringbuf::HeapCons<u8>,
    client: &mut HubIpcClient,
    frame_left: &mut usize,
    stuck_since: &mut Option<Instant>,
    finish_only: bool,
) -> Result<(), String> {
    loop {
        let (head, tail) = wire.as_slices();
        if head.is_empty() || (finish_only && *frame_left == 0) {
            return Ok(());
        }
        if *frame_left == 0 {
            let mut header = [0; HEADER_SIZE];
            let first = head.len().min(HEADER_SIZE);
            header[..first].copy_from_slice(&head[..first]);
            header[first..].copy_from_slice(&tail[..HEADER_SIZE - first]);
            let FrameHeader { payload_len, .. } = decode_header(&header);
            *frame_left = HEADER_SIZE + payload_len as usize;
        }
        let bytes = &head[..head.len().min(*frame_left)];
        match client.write_audio_bytes(bytes) {
            Ok(0) => return Err("the desk accepted nothing".to_string()),
            Ok(n) => {
                wire.skip(n);
                *frame_left -= n;
                *stuck_since = None;
            }
            Err(err) => {
                if err.kind() == io::ErrorKind::WouldBlock {
                    check_stuck(stuck_since)?;
                    return Ok(());
                }
                // An interrupted write consumed no bytes; retry the same slice.
                if err.kind() != io::ErrorKind::Interrupted {
                    return Err(format!("sending audio failed: {err}"));
                }
            }
        }
    }
}
