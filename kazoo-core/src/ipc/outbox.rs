//! Buffered, non-blocking frame writes over a hub connection.
//!
//! A non-blocking socket can accept part of a frame and refuse the rest.
//! Writing frames straight to it would then leave half a frame on the wire
//! and corrupt the stream, so every outgoing frame is queued here whole and
//! flushed as the socket accepts it, byte-exact across partial writes.

use std::io::{self, Write};

use super::protocol::{FrameHeader, HEADER_SIZE, MAX_PAYLOAD_SIZE, encode_header};

/// Most bytes the other end may leave unread before it counts as stuck.
pub const OUTBOX_LIMIT: usize = 64 * 1024;

/// Why a frame could not be queued or flushed.
#[derive(Debug)]
pub enum OutboxError {
    /// The other end has stopped reading: more than [`OUTBOX_LIMIT`] bytes
    /// are waiting. The frame was not queued.
    Stuck,
    /// The payload is larger than the protocol allows.
    TooLarge,
    /// The socket failed.
    Io(io::Error),
}

impl std::fmt::Display for OutboxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stuck => write!(
                f,
                "the other end stopped reading ({OUTBOX_LIMIT} bytes waiting)"
            ),
            Self::TooLarge => write!(f, "message larger than {MAX_PAYLOAD_SIZE} bytes"),
            Self::Io(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for OutboxError {}

/// Frames waiting to be written to one connection.
#[derive(Debug, Default)]
pub struct Outbox {
    pending: Vec<u8>,
    written: usize,
    seq: u32,
}

impl Outbox {
    /// Queue one frame. Nothing is written until [`Self::flush`].
    ///
    /// # Errors
    ///
    /// [`OutboxError::TooLarge`] for an oversized payload, and
    /// [`OutboxError::Stuck`] if the other end has stopped reading. Either
    /// way nothing is queued: frames go out whole or not at all.
    pub fn queue(&mut self, msg_type: u8, payload: &[u8]) -> Result<(), OutboxError> {
        if payload.len() > MAX_PAYLOAD_SIZE {
            return Err(OutboxError::TooLarge);
        }
        if self.waiting() + HEADER_SIZE + payload.len() > OUTBOX_LIMIT {
            return Err(OutboxError::Stuck);
        }
        let header = FrameHeader {
            msg_type,
            payload_len: u32::try_from(payload.len()).map_err(|_| OutboxError::TooLarge)?,
            seq: self.seq,
        };
        self.seq = self.seq.wrapping_add(1);
        let mut encoded = [0_u8; HEADER_SIZE];
        encode_header(&header, &mut encoded);
        self.pending.extend_from_slice(&encoded);
        self.pending.extend_from_slice(payload);
        Ok(())
    }

    /// Write as much as the socket accepts now.
    ///
    /// # Errors
    ///
    /// The socket's error, other than it being momentarily full.
    pub fn flush<W: Write>(&mut self, writer: &mut W) -> Result<(), OutboxError> {
        while self.written < self.pending.len() {
            match writer.write(&self.pending[self.written..]) {
                Ok(0) => {
                    return Err(OutboxError::Io(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "socket accepted nothing",
                    )));
                }
                Ok(n) => self.written += n,
                Err(err) => match err.kind() {
                    io::ErrorKind::WouldBlock => break,
                    // Interrupted by a signal before writing anything: the
                    // loop writes again.
                    io::ErrorKind::Interrupted => {}
                    _ => return Err(OutboxError::Io(err)),
                },
            }
        }
        if self.written == self.pending.len() {
            self.pending.clear();
            self.written = 0;
        }
        Ok(())
    }

    /// Bytes queued and not yet written.
    #[must_use]
    pub fn waiting(&self) -> usize {
        self.pending.len() - self.written
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::protocol::decode_header;

    /// A writer that accepts at most `chunk` bytes per call, then refuses
    /// with `WouldBlock` every other call.
    struct Trickle {
        out: Vec<u8>,
        chunk: usize,
        refuse_next: bool,
    }

    impl Write for Trickle {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.refuse_next {
                self.refuse_next = false;
                return Err(io::ErrorKind::WouldBlock.into());
            }
            self.refuse_next = true;
            let n = buf.len().min(self.chunk);
            self.out.extend_from_slice(&buf[..n]);
            Ok(n)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn partial_writes_deliver_every_frame_byte_exact() {
        let mut outbox = Outbox::default();
        outbox.queue(0x20, &[1, 2, 3, 4, 5]).unwrap();
        outbox.queue(0x30, &[9; 40]).unwrap();
        let mut sink = Trickle {
            out: Vec::new(),
            chunk: 7,
            refuse_next: false,
        };
        let mut rounds = 0;
        while outbox.waiting() > 0 {
            outbox.flush(&mut sink).unwrap();
            rounds += 1;
            assert!(rounds < 100);
        }
        assert_eq!(sink.out.len(), 2 * HEADER_SIZE + 45);
        let first = decode_header(&sink.out);
        assert_eq!((first.msg_type, first.payload_len, first.seq), (0x20, 5, 0));
        assert_eq!(&sink.out[HEADER_SIZE..HEADER_SIZE + 5], &[1, 2, 3, 4, 5]);
        let second = decode_header(&sink.out[HEADER_SIZE + 5..]);
        assert_eq!(
            (second.msg_type, second.payload_len, second.seq),
            (0x30, 40, 1)
        );
    }

    #[test]
    fn a_reader_that_stops_is_reported_stuck() {
        let mut outbox = Outbox::default();
        let payload = [0_u8; 1024];
        let mut queued = 0;
        loop {
            match outbox.queue(0x30, &payload) {
                Ok(()) => queued += 1,
                Err(OutboxError::Stuck) => break,
                Err(other) => panic!("unexpected {other:?}"),
            }
        }
        assert!(queued * (HEADER_SIZE + 1024) <= OUTBOX_LIMIT);
        assert!(matches!(
            outbox.queue(0x30, &vec![0; MAX_PAYLOAD_SIZE + 1]),
            Err(OutboxError::TooLarge)
        ));
    }
}
