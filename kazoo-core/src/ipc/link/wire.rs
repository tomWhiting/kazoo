//! Writes borrowed ring bytes while preserving the boundary of each frame.

use std::io;
use std::time::Instant;

use ringbuf::traits::Consumer;

use super::{AUDIO_PAYLOAD_HEADER, HubIpcClient, LINK_BACKLOG_MS, STUCK_LIMIT};
use crate::ipc::protocol::{FrameHeader, HEADER_SIZE, decode_header};

pub(super) fn link_ring_bytes(sample_rate: u32, channels: u16, block_frames: u32) -> usize {
    let frames = u64::from(sample_rate) * LINK_BACKLOG_MS / 1000;
    let headers = frames.div_ceil(64) * (HEADER_SIZE + AUDIO_PAYLOAD_HEADER) as u64;
    let bytes = frames * u64::from(channels) * 4 + headers;
    let largest =
        HEADER_SIZE + AUDIO_PAYLOAD_HEADER + block_frames as usize * usize::from(channels) * 4;
    (bytes as usize).max(2 * largest)
}

pub(super) fn check_stuck(stuck_since: &mut Option<Instant>) -> Result<(), String> {
    let now = Instant::now();
    let since = *stuck_since.get_or_insert(now);
    if now.duration_since(since) > STUCK_LIMIT {
        return Err("the desk stopped taking audio for over 500 ms".to_string());
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
