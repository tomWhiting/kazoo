//! Fixed-size callback stream clocks and audio wire encoding.

use super::super::protocol::{AUDIO_STREAM_PAYLOAD_HEADER, encode_audio_stream_head};
use super::{
    AUDIO_PAYLOAD_HEADER, FrameHeader, HEADER_SIZE, HubLinkAudio, MAX_STREAMS, MSG_AUDIO,
    MSG_STREAM_AUDIO, Observer, Ordering, Producer, StreamId, encode_header, sanitize_sample,
};

impl HubLinkAudio {
    pub(super) fn update_streams(&mut self) {
        for id in 0..MAX_STREAMS {
            let value = self.stream_updates[id].load(Ordering::Acquire);
            let version = value >> 16;
            if version != self.stream_versions[id] {
                self.next_frames[id] = 0;
                self.stream_versions[id] = version;
            }
            let channels = value as u16;
            self.stream_channels[id] = (channels != 0).then_some(channels);
        }
    }

    /// A stream's next frame; unknown ids have no rendered frames.
    #[must_use]
    pub fn stream_frame_of(&self, stream: StreamId) -> u64 {
        self.next_frames
            .get(usize::from(stream))
            .copied()
            .unwrap_or(0)
    }

    /// Queue named stream audio without allocation, locking or socket I/O.
    /// Unknown streams return false without advancing their clock.
    pub fn send_stream_audio(&mut self, stream: StreamId, frames: u32, samples: &[f32]) -> bool {
        self.update_streams();
        let id = usize::from(stream);
        let Some(channels) = self.stream_channels.get(id).copied().flatten() else {
            return false;
        };
        self.send_block(stream, channels, frames, samples, true)
    }

    /// Hand the link a rendered stream 0 block, retaining its original wire layout.
    pub fn send_audio(&mut self, frames: u32, samples: &[f32]) -> bool {
        self.send_block(0, self.channels, frames, samples, false)
    }

    fn send_block(
        &mut self,
        stream: StreamId,
        channels: u16,
        frames: u32,
        samples: &[f32],
        named: bool,
    ) -> bool {
        let id = usize::from(stream);
        let start_frame = self.next_frames[id];
        self.next_frames[id] = start_frame.wrapping_add(u64::from(frames));
        if !self.is_connected() {
            return false;
        }
        let channels = usize::from(channels);
        if (frames as usize).checked_mul(channels) != Some(samples.len()) {
            self.state.blocks_dropped.fetch_add(1, Ordering::Relaxed);
            return true;
        }
        let mut done = 0;
        while done < frames {
            let count = (frames - done).min(self.block_frames);
            let start = done as usize * channels;
            let end = start + count as usize * channels;
            self.push_stream(
                stream,
                start_frame.wrapping_add(u64::from(done)),
                count,
                &samples[start..end],
                named,
            );
            done += count;
        }
        true
    }

    fn push_stream(
        &mut self,
        stream: StreamId,
        start_frame: u64,
        frames: u32,
        samples: &[f32],
        named: bool,
    ) {
        let audio_head = if named {
            AUDIO_STREAM_PAYLOAD_HEADER
        } else {
            AUDIO_PAYLOAD_HEADER
        };
        let payload = audio_head + samples.len() * 4;
        let total = HEADER_SIZE + payload;
        if self.wire.vacant_len() < total {
            self.state.blocks_dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let mut head = [0; HEADER_SIZE + AUDIO_STREAM_PAYLOAD_HEADER];
        encode_header(
            &FrameHeader {
                msg_type: if named { MSG_STREAM_AUDIO } else { MSG_AUDIO },
                payload_len: payload as u32,
                seq: self.seq,
            },
            &mut head[..HEADER_SIZE],
        );
        if named {
            encode_audio_stream_head(start_frame, frames, stream, &mut head[HEADER_SIZE..]);
        } else {
            head[HEADER_SIZE..HEADER_SIZE + 8].copy_from_slice(&start_frame.to_le_bytes());
            head[HEADER_SIZE + 8..HEADER_SIZE + AUDIO_PAYLOAD_HEADER]
                .copy_from_slice(&frames.to_le_bytes());
        }
        // One publication preserves frame boundaries across disconnects.
        self.wire.push_iter(
            head[..HEADER_SIZE + audio_head].iter().copied().chain(
                samples
                    .iter()
                    .flat_map(|sample| sanitize_sample(*sample).to_le_bytes()),
            ),
        );
        self.seq = self.seq.wrapping_add(1);
        self.state.blocks_sent.fetch_add(1, Ordering::Relaxed);
        self.waker.unpark();
    }
}
