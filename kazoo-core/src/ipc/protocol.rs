//! Wire protocol: binary framed messages over Unix domain sockets.
//!
//! Frame format (little-endian):
//!
//! ```text
//! +--------+--------+--------+---------------------------+
//! | Type   | Length | Seq    | Payload                   |
//! | 1 byte | 4 byte | 4 byte | variable                  |
//! +--------+--------+--------+---------------------------+
//! ```
//!
//! All multi-byte integers are little-endian. The Length field contains
//! the payload size in bytes (not including the 9-byte header).

use std::io::{self, Read, Write};

/// Size of the frame header in bytes: 1 (type) + 4 (length) + 4 (seq).
pub const HEADER_SIZE: usize = 9;

/// Maximum payload size in bytes (64 KiB).
pub const MAX_PAYLOAD_SIZE: usize = 65_536;

/// Maximum total frame size (header + payload).
pub const MAX_FRAME_SIZE: usize = HEADER_SIZE + MAX_PAYLOAD_SIZE;

// ---------------------------------------------------------------------------
// Frame header
// ---------------------------------------------------------------------------

/// Parsed frame header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    /// Message type tag (see [`super::types`] constants).
    pub msg_type: u8,
    /// Payload length in bytes (excluding the 9-byte header).
    pub payload_len: u32,
    /// Monotonically increasing sequence number for ordering.
    pub seq: u32,
}

/// Encode a frame header into the first [`HEADER_SIZE`] bytes of `buf`.
///
/// # Panics
///
/// Panics if `buf.len() < HEADER_SIZE`.
pub fn encode_header(header: &FrameHeader, buf: &mut [u8]) {
    buf[0] = header.msg_type;
    buf[1..5].copy_from_slice(&header.payload_len.to_le_bytes());
    buf[5..9].copy_from_slice(&header.seq.to_le_bytes());
}

/// Decode a frame header from the first [`HEADER_SIZE`] bytes of `buf`.
///
/// # Panics
///
/// Panics if `buf.len() < HEADER_SIZE`.
#[must_use]
pub fn decode_header(buf: &[u8]) -> FrameHeader {
    FrameHeader {
        msg_type: buf[0],
        payload_len: u32::from_le_bytes([buf[1], buf[2], buf[3], buf[4]]),
        seq: u32::from_le_bytes([buf[5], buf[6], buf[7], buf[8]]),
    }
}

// ---------------------------------------------------------------------------
// Non-blocking read state machine
// ---------------------------------------------------------------------------

/// Read state machine for non-blocking frame reads.
///
/// Tracks partial reads across multiple `try_read_frame` calls so that
/// the IPC polling loop can resume where it left off when a socket
/// returns `WouldBlock`.
#[derive(Debug)]
enum ReadState {
    /// Reading the 9-byte header. `filled` bytes have been read so far.
    Header { filled: usize },
    /// Header is complete; reading the payload. `filled` payload bytes read.
    Payload { header: FrameHeader, filled: usize },
}

// ---------------------------------------------------------------------------
// FrameBuffer
// ---------------------------------------------------------------------------

/// Pre-allocated buffer for reading and writing IPC frames.
///
/// Handles partial reads on non-blocking sockets by maintaining an
/// internal state machine across calls to [`try_read_frame`](Self::try_read_frame).
///
/// One `FrameBuffer` is allocated per connection (one for reading, one
/// for writing). All allocations happen at construction time — the hot
/// path (audio send/receive) does zero allocations.
#[derive(Debug)]
pub struct FrameBuffer {
    /// Raw byte buffer, sized to [`MAX_FRAME_SIZE`].
    buf: Vec<u8>,
    /// Current non-blocking read state.
    read_state: ReadState,
}

impl FrameBuffer {
    /// Create a new pre-allocated frame buffer.
    #[must_use]
    pub fn new() -> Self {
        Self {
            buf: vec![0u8; MAX_FRAME_SIZE],
            read_state: ReadState::Header { filled: 0 },
        }
    }

    /// Access the payload portion of the buffer (after the header).
    ///
    /// Valid after a successful [`read_frame`](Self::read_frame) or
    /// [`try_read_frame`](Self::try_read_frame) call. The first
    /// `header.payload_len` bytes contain the message payload.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.buf[HEADER_SIZE..]
    }

    /// Mutable access to the payload portion for encoding outgoing messages.
    ///
    /// Write payload data here before calling [`write_frame`](Self::write_frame).
    pub fn payload_mut(&mut self) -> &mut [u8] {
        &mut self.buf[HEADER_SIZE..]
    }

    // -----------------------------------------------------------------------
    // Writing
    // -----------------------------------------------------------------------

    /// Write a complete frame to the given writer (blocking).
    ///
    /// The caller must have already written payload data into
    /// `self.payload_mut()[..payload_len]` before calling this method.
    /// This encodes the header and writes `header + payload` in one
    /// `write_all` call.
    pub fn write_frame<W: Write>(
        &mut self,
        msg_type: u8,
        seq: u32,
        payload_len: usize,
        writer: &mut W,
    ) -> io::Result<()> {
        let header = FrameHeader {
            msg_type,
            payload_len: u32::try_from(payload_len).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "payload too large for u32")
            })?,
            seq,
        };
        encode_header(&header, &mut self.buf);
        writer.write_all(&self.buf[..HEADER_SIZE + payload_len])
    }

    // -----------------------------------------------------------------------
    // Blocking read
    // -----------------------------------------------------------------------

    /// Read a complete frame from the given reader (blocking).
    ///
    /// Blocks until the full header + payload is received. On success
    /// the payload is available via `self.payload()[..header.payload_len]`.
    pub fn read_frame<R: Read>(&mut self, reader: &mut R) -> io::Result<FrameHeader> {
        reader.read_exact(&mut self.buf[..HEADER_SIZE])?;
        let header = decode_header(&self.buf);

        let payload_len = header.payload_len as usize;
        if payload_len > MAX_PAYLOAD_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("payload too large: {payload_len} > {MAX_PAYLOAD_SIZE}"),
            ));
        }

        if payload_len > 0 {
            reader.read_exact(&mut self.buf[HEADER_SIZE..HEADER_SIZE + payload_len])?;
        }

        // Reset non-blocking state (in case it was partially used).
        self.read_state = ReadState::Header { filled: 0 };
        Ok(header)
    }

    // -----------------------------------------------------------------------
    // Non-blocking read
    // -----------------------------------------------------------------------

    /// Try to read a complete frame from a non-blocking reader.
    ///
    /// Returns `Ok(Some(header))` when a complete frame has been read
    /// (payload in `self.payload()`). Returns `Ok(None)` when no data
    /// is available yet — the partial read state is preserved and the
    /// next call will resume where it left off.
    ///
    /// Returns `Err` on genuine I/O errors (broken pipe, EOF, etc.).
    pub fn try_read_frame<R: Read>(&mut self, reader: &mut R) -> io::Result<Option<FrameHeader>> {
        loop {
            match &mut self.read_state {
                ReadState::Header { filled } => {
                    if *filled >= HEADER_SIZE {
                        // Header complete — decode and transition to payload.
                        let header = decode_header(&self.buf);
                        let payload_len = header.payload_len as usize;
                        if payload_len > MAX_PAYLOAD_SIZE {
                            self.read_state = ReadState::Header { filled: 0 };
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                format!("payload too large: {payload_len}"),
                            ));
                        }
                        if payload_len == 0 {
                            // No payload — frame is complete.
                            self.read_state = ReadState::Header { filled: 0 };
                            return Ok(Some(header));
                        }
                        self.read_state = ReadState::Payload { header, filled: 0 };
                        continue;
                    }

                    match reader.read(&mut self.buf[*filled..HEADER_SIZE]) {
                        Ok(0) => {
                            return Err(io::Error::new(
                                io::ErrorKind::UnexpectedEof,
                                "connection closed during header read",
                            ));
                        }
                        Ok(n) => {
                            *filled += n;
                            // Loop to check if header is now complete.
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            return Ok(None);
                        }
                        Err(e) => return Err(e),
                    }
                }

                ReadState::Payload { header, filled } => {
                    let payload_len = header.payload_len as usize;
                    if *filled >= payload_len {
                        // Frame complete.
                        let h = *header;
                        self.read_state = ReadState::Header { filled: 0 };
                        return Ok(Some(h));
                    }

                    let start = HEADER_SIZE + *filled;
                    let end = HEADER_SIZE + payload_len;
                    match reader.read(&mut self.buf[start..end]) {
                        Ok(0) => {
                            return Err(io::Error::new(
                                io::ErrorKind::UnexpectedEof,
                                "connection closed during payload read",
                            ));
                        }
                        Ok(n) => {
                            *filled += n;
                            // Loop to check if payload is now complete.
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            return Ok(None);
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
        }
    }

    /// Reset the read state machine.
    ///
    /// Call this after a connection error to discard any partial frame
    /// data and start reading fresh.
    pub const fn reset_read_state(&mut self) {
        self.read_state = ReadState::Header { filled: 0 };
    }
}

impl Default for FrameBuffer {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Audio payload helpers
// ---------------------------------------------------------------------------

/// Size of the audio payload header (`frame_count` field).
pub const AUDIO_PAYLOAD_HEADER: usize = 4;

/// Encode audio samples into a buffer's payload area.
///
/// Writes a 4-byte `frame_count` followed by each sample as a
/// little-endian f32. NaN/Inf samples are replaced with `0.0`.
///
/// Returns the total payload size in bytes.
pub fn encode_audio_payload(frame_count: u32, samples: &[f32], buf: &mut [u8]) -> usize {
    buf[0..4].copy_from_slice(&frame_count.to_le_bytes());
    let mut offset = AUDIO_PAYLOAD_HEADER;
    for &sample in samples {
        let s = if sample.is_finite() { sample } else { 0.0 };
        buf[offset..offset + 4].copy_from_slice(&s.to_le_bytes());
        offset += 4;
    }
    offset
}

/// Decode the frame count from an audio payload.
#[must_use]
pub fn decode_audio_frame_count(buf: &[u8]) -> u32 {
    u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]])
}

/// Decode audio samples from a payload buffer into a pre-allocated slice.
///
/// Reads `count` f32 values starting after the 4-byte `frame_count` header.
/// NaN/Inf values are sanitized to `0.0`.
pub fn decode_audio_samples(buf: &[u8], samples: &mut [f32], count: usize) {
    let mut offset = AUDIO_PAYLOAD_HEADER;
    for sample in samples.iter_mut().take(count) {
        if offset + 4 <= buf.len() {
            let raw = f32::from_le_bytes([
                buf[offset],
                buf[offset + 1],
                buf[offset + 2],
                buf[offset + 3],
            ]);
            *sample = if raw.is_finite() { raw } else { 0.0 };
            offset += 4;
        } else {
            *sample = 0.0;
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_roundtrip() {
        let header = FrameHeader {
            msg_type: 0x10,
            payload_len: 1024,
            seq: 42,
        };
        let mut buf = [0u8; HEADER_SIZE];
        encode_header(&header, &mut buf);
        let decoded = decode_header(&buf);
        assert_eq!(header, decoded);
    }

    #[test]
    fn header_zero_values() {
        let header = FrameHeader {
            msg_type: 0,
            payload_len: 0,
            seq: 0,
        };
        let mut buf = [0u8; HEADER_SIZE];
        encode_header(&header, &mut buf);
        let decoded = decode_header(&buf);
        assert_eq!(header, decoded);
    }

    #[test]
    fn header_max_values() {
        let header = FrameHeader {
            msg_type: 0xFF,
            payload_len: u32::MAX,
            seq: u32::MAX,
        };
        let mut buf = [0u8; HEADER_SIZE];
        encode_header(&header, &mut buf);
        let decoded = decode_header(&buf);
        assert_eq!(header, decoded);
    }

    #[test]
    fn frame_buffer_blocking_roundtrip() {
        let mut write_buf = FrameBuffer::new();
        let payload = b"hello, kazoo!";
        write_buf.payload_mut()[..payload.len()].copy_from_slice(payload);

        let mut pipe = Vec::new();
        write_buf
            .write_frame(0x01, 7, payload.len(), &mut pipe)
            .unwrap();

        assert_eq!(pipe.len(), HEADER_SIZE + payload.len());

        let mut read_buf = FrameBuffer::new();
        let header = read_buf.read_frame(&mut pipe.as_slice()).unwrap();

        assert_eq!(header.msg_type, 0x01);
        assert_eq!(header.payload_len, payload.len() as u32);
        assert_eq!(header.seq, 7);
        assert_eq!(&read_buf.payload()[..payload.len()], payload);
    }

    #[test]
    fn frame_buffer_empty_payload() {
        let mut write_buf = FrameBuffer::new();
        let mut pipe = Vec::new();
        write_buf.write_frame(0xFF, 0, 0, &mut pipe).unwrap();

        let mut read_buf = FrameBuffer::new();
        let header = read_buf.read_frame(&mut pipe.as_slice()).unwrap();
        assert_eq!(header.msg_type, 0xFF);
        assert_eq!(header.payload_len, 0);
    }

    #[test]
    fn audio_payload_roundtrip() {
        let samples = [0.5_f32, -0.25, 1.0, 0.0, f32::NAN, f32::INFINITY];
        let mut buf = vec![0u8; AUDIO_PAYLOAD_HEADER + samples.len() * 4];
        let len = encode_audio_payload(3, &samples, &mut buf);
        assert_eq!(len, AUDIO_PAYLOAD_HEADER + samples.len() * 4);

        let frame_count = decode_audio_frame_count(&buf);
        assert_eq!(frame_count, 3);

        let mut decoded = [0.0f32; 6];
        decode_audio_samples(&buf, &mut decoded, 6);

        assert!((decoded[0] - 0.5).abs() < f32::EPSILON);
        assert!((decoded[1] - (-0.25)).abs() < f32::EPSILON);
        assert!((decoded[2] - 1.0).abs() < f32::EPSILON);
        assert!((decoded[3] - 0.0).abs() < f32::EPSILON);
        // NaN and Inf should be sanitized to 0.0.
        assert!((decoded[4] - 0.0).abs() < f32::EPSILON);
        assert!((decoded[5] - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn audio_payload_short_buffer_fills_zero() {
        let buf = [0u8; AUDIO_PAYLOAD_HEADER]; // No sample data after header.
        let mut decoded = [1.0f32; 4];
        decode_audio_samples(&buf, &mut decoded, 4);
        for &s in &decoded {
            assert!((s - 0.0).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn frame_buffer_rejects_oversized_payload() {
        let mut buf = FrameBuffer::new();
        let header = FrameHeader {
            msg_type: 0x10,
            payload_len: MAX_PAYLOAD_SIZE as u32 + 1,
            seq: 0,
        };
        let mut wire = vec![0u8; HEADER_SIZE];
        encode_header(&header, &mut wire);
        let result = buf.read_frame(&mut wire.as_slice());
        assert!(result.is_err());
    }
}
