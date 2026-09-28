//! The control socket: accepting connections, and a reader and a writer
//! thread for each.
//!
//! Readers turn lines into requests for the control thread; writers send
//! whatever the control thread queues, in order. Nothing here touches the
//! wall's state: the control thread owns it all.

use std::io::{self, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, sync_channel};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::protocol::codec::{self, LineRead};
use crate::protocol::{ErrorCode, MAX_LINE, MAX_SERVER_LINE, RequestLine, Response, WallError};

/// Lines queued for one connection before it counts as stuck and is closed.
pub const OUT_BACKLOG: usize = 1_024;

/// How long a write may block before the client counts as stuck.
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

/// How often the accept loop looks for new connections and for the stop
/// flag.
const ACCEPT_POLL: Duration = Duration::from_millis(20);

/// Something for a connection's writer.
#[derive(Debug)]
pub enum Outgoing {
    /// A line to send, newline included.
    Line(Vec<u8>),
    /// Send everything before this, then close the connection.
    Close,
}

/// What the socket threads tell the control thread.
#[derive(Debug)]
pub enum ToControl {
    /// A connection opened.
    Opened {
        /// Its number.
        conn: u64,
        /// Its writer's queue.
        out: SyncSender<Outgoing>,
        /// The socket, to shut it down from outside.
        stream: UnixStream,
        /// The writer thread.
        writer: JoinHandle<()>,
    },
    /// A request line arrived; `Err` is the response to a line that was
    /// not a request.
    Line {
        /// The connection.
        conn: u64,
        /// The request, or the error to answer with.
        line: Result<RequestLine, Response>,
    },
    /// A line longer than [`MAX_LINE`] arrived: the connection must close.
    TooLong {
        /// The connection.
        conn: u64,
    },
    /// The connection closed.
    Closed {
        /// The connection.
        conn: u64,
    },
}

/// Encode a response or event as a line.
pub fn line(value: &impl serde::Serialize) -> Vec<u8> {
    match serde_json::to_vec(value) {
        Ok(mut bytes) if bytes.len() <= MAX_SERVER_LINE => {
            bytes.push(b'\n');
            bytes
        }
        Ok(_) => internal_line("the answer is too long to send"),
        Err(err) => internal_line(&format!("the answer could not be encoded: {err}")),
    }
}

fn internal_line(message: &str) -> Vec<u8> {
    let mut bytes = format!(
        "{{\"id\":0,\"ok\":false,\"error\":{{\"code\":\"internal\",\"message\":{}}}}}",
        serde_json::Value::String(message.to_string())
    )
    .into_bytes();
    bytes.push(b'\n');
    bytes
}

/// Accept connections on `listener` until `stop` is set.
///
/// # Errors
///
/// Fails if the listener cannot be made non-blocking or the thread cannot
/// start.
pub fn spawn_accept(
    listener: UnixListener,
    control: Sender<ToControl>,
    stop: Arc<AtomicBool>,
) -> io::Result<JoinHandle<()>> {
    listener.set_nonblocking(true)?;
    thread::Builder::new()
        .name("kazoo-wall-accept".to_string())
        .spawn(move || {
            let next = AtomicU64::new(1);
            while !stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let conn = next.fetch_add(1, Ordering::Relaxed);
                        match open(stream, conn, &control) {
                            // A caller only asking whether a wall is playing
                            // (`is_running`) hangs up as soon as it is
                            // accepted: that is its answer, not a fault.
                            Ok(Opened::Started | Opened::HungUp) => {}
                            Err(err) => {
                                eprintln!("kazoo-wall: connection {conn} could not start: {err}");
                            }
                        }
                    }
                    Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(ACCEPT_POLL);
                    }
                    Err(err) => {
                        eprintln!("kazoo-wall: accepting a connection failed: {err}");
                        thread::sleep(ACCEPT_POLL);
                    }
                }
            }
        })
}

/// Start a connection's writer and reader.
/// How a newly accepted connection turned out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Opened {
    /// Its reader and writer are running.
    Started,
    /// The caller had already hung up, so there was nothing to start.
    HungUp,
}

/// Set up a newly accepted connection: blocking, with a write timeout.
fn configure(stream: &UnixStream) -> io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))
}

/// Whether the far end of `stream` has already closed: a read that cannot
/// wait finds the end of the stream. (macOS refuses socket options on such
/// a stream with `EINVAL`, which is how a hang-up first shows.) Asked only
/// of a connection that could not be set up, so a byte this read takes
/// from a live caller is never wanted.
fn hung_up(mut stream: &UnixStream) -> bool {
    stream.set_nonblocking(true).is_ok() && matches!(stream.read(&mut [0u8; 1]), Ok(0))
}

fn open(stream: UnixStream, conn: u64, control: &Sender<ToControl>) -> io::Result<Opened> {
    if let Err(err) = configure(&stream) {
        return if hung_up(&stream) {
            Ok(Opened::HungUp)
        } else {
            Err(err)
        };
    }
    let (out, queue) = sync_channel(OUT_BACKLOG);
    let write_half = stream.try_clone()?;
    let writer = thread::Builder::new()
        .name(format!("kazoo-wall-write-{conn}"))
        .spawn(move || write_loop(write_half, &queue))?;
    let read_half = stream.try_clone()?;
    let reader_control = control.clone();
    let reader_out = out.clone();
    if control
        .send(ToControl::Opened {
            conn,
            out,
            stream,
            writer,
        })
        .is_err()
    {
        // The daemon is stopping: turn the connection away.
        refuse(&reader_out, 0);
        return Ok(Opened::Started);
    }
    thread::Builder::new()
        .name(format!("kazoo-wall-read-{conn}"))
        .spawn(move || read_loop(read_half, conn, &reader_control, &reader_out))?;
    Ok(Opened::Started)
}

/// Tell a connection the daemon is going away, and close it.
fn refuse(out: &SyncSender<Outgoing>, id: u64) {
    let response = Response::failure(
        id,
        WallError::new(ErrorCode::Internal, "the wall is shutting down"),
    );
    // A full or closed queue means the connection is going anyway.
    let queued = out.try_send(Outgoing::Line(line(&response)));
    let closed = out.try_send(Outgoing::Close);
    if queued.is_err() || closed.is_err() {
        eprintln!("kazoo-wall: a connection closed before it heard the wall is stopping");
    }
}

fn write_loop(mut stream: UnixStream, queue: &Receiver<Outgoing>) {
    for item in queue {
        match item {
            Outgoing::Line(bytes) => {
                if stream
                    .write_all(&bytes)
                    .and_then(|()| stream.flush())
                    .is_err()
                {
                    break;
                }
            }
            Outgoing::Close => break,
        }
    }
    // The reader sees the end of its stream and reports the close.
    if let Err(err) = stream.shutdown(Shutdown::Both) {
        if err.kind() != io::ErrorKind::NotConnected {
            eprintln!("kazoo-wall: closing a connection failed: {err}");
        }
    }
}

fn read_loop(
    stream: UnixStream,
    conn: u64,
    control: &Sender<ToControl>,
    out: &SyncSender<Outgoing>,
) {
    let mut reader = BufReader::new(stream);
    loop {
        let message = match codec::read_line_capped(&mut reader, MAX_LINE) {
            Ok(LineRead::Line(bytes)) => ToControl::Line {
                conn,
                line: parse(&bytes),
            },
            Ok(LineRead::TooLong) => ToControl::TooLong { conn },
            Ok(LineRead::End) | Err(_) => ToControl::Closed { conn },
        };
        let last = matches!(
            message,
            ToControl::TooLong { .. } | ToControl::Closed { .. }
        );
        let id = match &message {
            ToControl::Line { line: Ok(line), .. } => line.id,
            _ => 0,
        };
        if control.send(message).is_err() {
            refuse(out, id);
            return;
        }
        if last {
            return;
        }
    }
}

/// A request from a line, or the error response for a line that is not
/// one (with the line's id when it has a readable one).
fn parse(bytes: &[u8]) -> Result<RequestLine, Response> {
    let bad = |id: u64, message: String| {
        Response::failure(id, WallError::new(ErrorCode::BadRequest, message))
    };
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|err| bad(0, format!("not a JSON line: {err}")))?;
    let id = value.get("id").and_then(serde_json::Value::as_u64);
    let Some(id) = id else {
        return Err(bad(
            0,
            "every request needs an \"id\": a whole number".to_string(),
        ));
    };
    serde_json::from_value(value).map_err(|err| bad(id, format!("not a request: {err}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_parse_into_requests_or_errors_with_their_id() {
        let ok = parse(br#"{"id":3,"op":"look"}"#).unwrap();
        assert_eq!(ok.id, 3);
        let bad = parse(br#"{"id":4,"op":"dance"}"#).unwrap_err();
        assert_eq!(bad.id, 4);
        assert_eq!(bad.error.unwrap().code, ErrorCode::BadRequest);
        assert_eq!(parse(b"nope").unwrap_err().id, 0);
        assert_eq!(parse(br#"{"op":"look"}"#).unwrap_err().id, 0);
        assert_eq!(parse(br#"{"id":-1,"op":"look"}"#).unwrap_err().id, 0);
    }

    #[test]
    fn a_stopped_daemon_answers_internal() {
        let (control, inbox) = std::sync::mpsc::channel();
        drop(inbox);
        let (ours, theirs) = UnixStream::pair().unwrap();
        open(theirs, 1, &control).unwrap();
        let mut reader = BufReader::new(ours);
        let LineRead::Line(bytes) = codec::read_line(&mut reader).unwrap() else {
            panic!("no answer");
        };
        let response: Response = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(response.error.unwrap().code, ErrorCode::Internal);
        assert_eq!(codec::read_line(&mut reader).unwrap(), LineRead::End);
    }

    /// A caller that asks only whether a wall is playing connects and hangs
    /// up at once; opening what it leaves is never an error.
    #[test]
    fn a_caller_that_hangs_up_at_once_is_not_a_fault() {
        let (control, inbox) = std::sync::mpsc::channel();
        let (ours, theirs) = UnixStream::pair().unwrap();
        drop(ours);
        let opened = open(theirs, 1, &control).unwrap();
        assert!(matches!(opened, Opened::Started | Opened::HungUp));
        drop(inbox);
    }

    #[test]
    fn a_closed_stream_reads_as_hung_up_and_an_open_one_does_not() {
        let (ours, theirs) = UnixStream::pair().unwrap();
        assert!(!hung_up(&theirs));
        drop(ours);
        assert!(hung_up(&theirs));
    }

    #[test]
    fn overlong_answers_become_internal_errors() {
        let big = "x".repeat(MAX_SERVER_LINE + 1);
        let bytes = line(&big);
        let response: Response = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(response.error.unwrap().code, ErrorCode::Internal);
        assert_eq!(bytes.last(), Some(&b'\n'));
    }
}
