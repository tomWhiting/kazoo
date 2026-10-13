//! Named stream descriptions and live stream control payloads.

/// Maximum number of streams on one instrument link.
pub const MAX_STREAMS: usize = 16;
/// A stream's stable identifier within its link.
pub type StreamId = u16;
/// Fixed-width UTF-8 stream name, padded with NUL bytes.
pub const STREAM_NAME_LEN: usize = 24;
/// Size of one registration stream entry.
pub const STREAM_ENTRY_SIZE: usize = 1 + STREAM_NAME_LEN;
/// No desk strip was assigned.
pub const NO_STRIP: u8 = 0xff;
/// Instrument -> hub: audio for a named stream.
pub const MSG_STREAM_AUDIO: u8 = 0x11;
/// Instrument -> hub: add a stream while connected.
pub const MSG_STREAM_ADD: u8 = 0x12;
/// Instrument -> hub: remove a stream while connected.
pub const MSG_STREAM_REMOVE: u8 = 0x13;
/// Hub -> instrument: stream strip assignment or refusal.
pub const MSG_STREAM_READY: u8 = 0x14;
/// Maximum UTF-8 refusal reason length in bytes.
pub const STREAM_REFUSAL_LEN: usize = 256;

/// One named mono or stereo stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamSpec {
    /// 1 for mono, 2 for stereo.
    pub channels: u8,
    /// UTF-8 name, NUL padded.
    pub name: [u8; STREAM_NAME_LEN],
}

impl StreamSpec {
    /// Construct a description, truncating only at a UTF-8 character boundary.
    #[must_use]
    pub fn new(name: &str, channels: u8) -> Self {
        let mut end = name.len().min(STREAM_NAME_LEN);
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        let mut bytes = [0; STREAM_NAME_LEN];
        bytes[..end].copy_from_slice(&name.as_bytes()[..end]);
        Self {
            channels,
            name: bytes,
        }
    }

    /// The name up to its first NUL byte; invalid UTF-8 reads as empty.
    #[must_use]
    pub fn name_str(&self) -> &str {
        let end = self
            .name
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(STREAM_NAME_LEN);
        std::str::from_utf8(&self.name[..end]).unwrap_or("")
    }

    pub(super) fn encode(&self, buf: &mut [u8]) {
        buf[0] = self.channels;
        buf[1..STREAM_ENTRY_SIZE].copy_from_slice(&self.name);
    }

    pub(super) fn decode(buf: &[u8]) -> Self {
        let mut name = [0; STREAM_NAME_LEN];
        name.copy_from_slice(&buf[1..STREAM_ENTRY_SIZE]);
        Self {
            channels: buf[0],
            name,
        }
    }
}

/// Add a stream with a stable link-local id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamAddMsg {
    /// Link-local stream id.
    pub stream: StreamId,
    /// Stream name and channel count.
    pub spec: StreamSpec,
}

impl StreamAddMsg {
    /// u16 id followed by a stream entry.
    pub const WIRE_SIZE: usize = 2 + STREAM_ENTRY_SIZE;
    /// Encode into at least [`Self::WIRE_SIZE`] bytes.
    pub fn encode(&self, buf: &mut [u8]) -> usize {
        buf[..2].copy_from_slice(&self.stream.to_le_bytes());
        self.spec.encode(&mut buf[2..]);
        Self::WIRE_SIZE
    }
    /// Decode at least [`Self::WIRE_SIZE`] bytes.
    #[must_use]
    pub fn decode(buf: &[u8]) -> Self {
        Self {
            stream: u16::from_le_bytes([buf[0], buf[1]]),
            spec: StreamSpec::decode(&buf[2..]),
        }
    }
}

/// Remove a link-local stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamRemoveMsg {
    /// Link-local stream id.
    pub stream: StreamId,
}

impl StreamRemoveMsg {
    /// Little-endian u16 id.
    pub const WIRE_SIZE: usize = 2;
    /// Encode into at least [`Self::WIRE_SIZE`] bytes.
    pub fn encode(&self, buf: &mut [u8]) -> usize {
        buf[..2].copy_from_slice(&self.stream.to_le_bytes());
        Self::WIRE_SIZE
    }
    /// Decode at least [`Self::WIRE_SIZE`] bytes.
    #[must_use]
    pub fn decode(buf: &[u8]) -> Self {
        Self {
            stream: u16::from_le_bytes([buf[0], buf[1]]),
        }
    }
}

/// A stream's strip, or a refusal followed by UTF-8 reason bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamReadyMsg {
    /// Link-local stream id.
    pub stream: StreamId,
    /// Assigned strip, or [`NO_STRIP`].
    pub strip: u8,
}

impl StreamReadyMsg {
    /// u16 id followed by a u8 strip; reason bytes follow this head.
    pub const WIRE_SIZE: usize = 3;
    /// Encode the head into at least [`Self::WIRE_SIZE`] bytes.
    pub fn encode(&self, buf: &mut [u8]) -> usize {
        buf[..2].copy_from_slice(&self.stream.to_le_bytes());
        buf[2] = self.strip;
        Self::WIRE_SIZE
    }
    /// Decode a validated payload and borrow its optional UTF-8 reason.
    /// The caller validates UTF-8 and the 256-byte reason limit first.
    #[must_use]
    pub fn decode(buf: &[u8]) -> (Self, &str) {
        (
            Self {
                stream: u16::from_le_bytes([buf[0], buf[1]]),
                strip: buf[2],
            },
            match std::str::from_utf8(&buf[3..]) {
                Ok(reason) => reason,
                Err(err) => panic!("stream ready reason is not UTF-8: {err}"),
            },
        )
    }
}
