//! The line to the wall's daemon, on tokio.
//!
//! [`Wall`] carries the tools' requests on one connection, made when first
//! needed and made again whenever the daemon has gone away. [`follow`] keeps
//! a second, subscribed connection open for as long as this seat runs,
//! trying again every [`RECONNECT_EVERY`] while the daemon is down, and
//! hands what it hears to the channel (see [`crate::channel`]).
//!
//! Both speak the wall's protocol with its own types
//! ([`kazoo_wall::protocol`]): newline-delimited JSON, requests capped at
//! [`kazoo_wall::protocol::MAX_LINE`] and the daemon's lines at
//! [`MAX_SERVER_LINE`]. This seat never starts the daemon.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use kazoo_wall::protocol::codec::{LineRead, encode_line};
use kazoo_wall::protocol::{
    HelloResult, MAX_SERVER_LINE, Request, RequestLine, ServerLine, SubscribeResult, WallError,
};
use serde::de::DeserializeOwned;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{Mutex, mpsc, watch};

use crate::channel::Feed;

/// How long the subscription waits between tries while the daemon is down.
pub const RECONNECT_EVERY: Duration = Duration::from_secs(2);

/// How long a request waits for its answer before the connection is given
/// up.
pub const ANSWER_WITHIN: Duration = Duration::from_secs(10);

/// How long a `speak` request waits: the wall answers once the words are
/// rendered. `say` may take up to 30 s on a long phrase, and the wall takes
/// new words only while at most one render is ahead of them, so two renders
/// and a margin.
pub const SPEECH_WITHIN: Duration = Duration::from_secs(75);

/// What every tool answers while the daemon is down.
pub const NOT_RUNNING: &str = "the wall is not running (start it with `kazoo wall`)";

/// This software, as it introduces itself to the daemon.
pub const CLIENT: &str = concat!("kazoo-mcp ", env!("CARGO_PKG_VERSION"));

/// Why a request did not come back with a result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkError {
    /// The daemon's socket is not there, or nothing is listening on it.
    NotRunning,
    /// The daemon answered with an error; its message names what is valid.
    Wall(WallError),
    /// The request could not be put on the line (too long for the protocol).
    Unsendable(String),
    /// The connection failed, and nothing was changed.
    Broken(String),
    /// The connection failed after a change was sent: it may or may not
    /// have been made.
    Uncertain(String),
    /// This seat has left the wall (`wall_leave`): nothing was sent.
    Left,
}

impl fmt::Display for LinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRunning => f.write_str(NOT_RUNNING),
            Self::Wall(err) => write!(
                f,
                "the wall said no ({}): {}",
                code_name(err.code),
                err.message
            ),
            Self::Unsendable(why) => write!(f, "the request cannot be sent: {why}"),
            Self::Broken(why) => write!(f, "the line to the wall failed: {why}"),
            Self::Left => f.write_str("this seat has left that wall: nothing was sent"),
            Self::Uncertain(why) => write!(
                f,
                "the line to the wall failed before it answered ({why}), so the change may \
                 or may not have been made: check wall_log before trying again"
            ),
        }
    }
}

/// An error code's name on the wire.
#[must_use]
pub const fn code_name(code: kazoo_wall::protocol::ErrorCode) -> &'static str {
    use kazoo_wall::protocol::ErrorCode;
    match code {
        ErrorCode::NotFound => "not_found",
        ErrorCode::BadRequest => "bad_request",
        ErrorCode::NotHello => "not_hello",
        ErrorCode::BadName => "bad_name",
        ErrorCode::UnknownModule => "unknown_module",
        ErrorCode::UnknownKnob => "unknown_knob",
        ErrorCode::UnknownPort => "unknown_port",
        ErrorCode::UnknownKind => "unknown_kind",
        ErrorCode::UnknownCable => "unknown_cable",
        ErrorCode::UnknownChange => "unknown_change",
        ErrorCode::Full => "full",
        ErrorCode::UnknownSave => "unknown_save",
        ErrorCode::SaveExists => "save_exists",
        ErrorCode::BrokenSave => "broken_save",
        ErrorCode::WallReplaced => "wall_replaced",
        ErrorCode::SlowDown => "slow_down",
        ErrorCode::NotAllowed => "not_allowed",
        ErrorCode::Internal => "internal",
        ErrorCode::UnknownMoment => "unknown_moment",
        ErrorCode::MarkExists => "mark_exists",
    }
}

/// How an exchange on an open connection failed.
#[derive(Debug)]
struct Failure {
    /// Whether the request had been written in full.
    sent: bool,
    /// What went wrong, for people.
    why: String,
}

impl Failure {
    fn unsent(why: impl fmt::Display) -> Self {
        Self {
            sent: false,
            why: why.to_string(),
        }
    }

    fn sent(why: impl fmt::Display) -> Self {
        Self {
            sent: true,
            why: why.to_string(),
        }
    }
}

/// One connection to the daemon, after hello.
#[derive(Debug)]
struct Connection {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    next_id: u64,
    /// Whether the wall keeps comments (see [`HelloResult::comments`]).
    comments: bool,
    notes: bool,
}

impl Connection {
    /// Connect to `socket` and say hello as `seat`, or as a watcher (never
    /// listed among the seats, never announced, and changing nothing).
    async fn open(
        socket: &Path,
        seat: &str,
        watcher: bool,
    ) -> Result<(Self, HelloResult), LinkError> {
        let stream = UnixStream::connect(socket).await.map_err(|err| {
            if matches!(
                err.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) {
                LinkError::NotRunning
            } else {
                LinkError::Broken(format!("{} cannot be reached: {err}", socket.display()))
            }
        })?;
        let (read, writer) = stream.into_split();
        let mut connection = Self {
            reader: BufReader::new(read),
            writer,
            next_id: 1,
            comments: false,
            notes: false,
        };
        let hello = Request::Hello {
            seat: seat.to_string(),
            client: CLIENT.to_string(),
            console: false,
            watcher,
        };
        let line = line_for(&mut connection.next_id, hello, (None, None))?;
        let answered = tokio::time::timeout(ANSWER_WITHIN, connection.exchange(&line)).await;
        let value = match answered {
            Ok(Ok(Ok(value))) => value,
            Ok(Ok(Err(err))) => return Err(LinkError::Wall(err)),
            Ok(Err(failure)) => return Err(LinkError::Broken(failure.why)),
            Err(_) => return Err(LinkError::Broken(timed_out(ANSWER_WITHIN))),
        };
        let hello: HelloResult = decode(value).map_err(LinkError::Broken)?;
        connection.comments = hello.comments;
        connection.notes = hello.notes;
        Ok((connection, hello))
    }

    /// Write one request line and read until its response. The outer
    /// result is the connection's; the inner one is the daemon's answer.
    async fn exchange(
        &mut self,
        line: &Outgoing,
    ) -> Result<Result<serde_json::Value, WallError>, Failure> {
        self.writer
            .write_all(&line.bytes)
            .await
            .map_err(Failure::unsent)?;
        self.writer.flush().await.map_err(Failure::unsent)?;
        loop {
            match self.read().await.map_err(Failure::sent)? {
                // Not subscribed, so none are expected; one that comes is
                // not an answer and not a fault in the line.
                ServerLine::Event(_) => {}
                ServerLine::Response(response) if response.id == line.id => {
                    return if response.ok {
                        Ok(Ok(response.result.unwrap_or(serde_json::Value::Null)))
                    } else {
                        response.error.map(Err).ok_or_else(|| {
                            Failure::sent("the wall answered with a failure but no error")
                        })
                    };
                }
                ServerLine::Response(response) => {
                    return Err(Failure::sent(format!(
                        "the wall answered request {} while this seat waited for {}",
                        response.id, line.id
                    )));
                }
            }
        }
    }

    /// The next line from the daemon.
    async fn read(&mut self) -> Result<ServerLine, String> {
        match read_line_capped(&mut self.reader, MAX_SERVER_LINE).await {
            Ok(LineRead::Line(bytes)) => {
                let text = String::from_utf8(bytes)
                    .map_err(|_| "the wall sent a line that is not UTF-8".to_string())?;
                ServerLine::parse(&text)
                    .map_err(|err| format!("the wall sent a line this seat cannot read: {err}"))
            }
            Ok(LineRead::End) => Err("the wall closed the connection".to_string()),
            Ok(LineRead::TooLong) => {
                Err("the wall sent a line longer than the protocol allows".to_string())
            }
            Err(err) => Err(format!("the socket failed: {err}")),
        }
    }

    /// Whether the daemon still has this connection open: nothing is
    /// waiting to be read and the socket has not reached its end. A
    /// connection whose daemon has gone reads its end at once.
    fn looks_alive(&self) -> bool {
        if !self.reader.buffer().is_empty() {
            return false;
        }
        let mut probe = [0_u8; 1];
        match self.reader.get_ref().try_read(&mut probe) {
            Err(err) => err.kind() == io::ErrorKind::WouldBlock,
            Ok(_) => false,
        }
    }
}

/// Why a comment cannot go to a wall older than comments.
const OLD_WALL: &str = "the wall running is older than comments and would drop them; nothing \
                        was sent. Send it again without comments, or ask Tom to restart the wall \
                        (kazoo wall stop, then kazoo wall) to keep them";

/// Whether `request`, with `comment`, carries a comment: its own, or one
/// on a step of a batch.
fn commented(request: &Request, comment: Option<&str>) -> bool {
    comment.is_some() || matches!(request, Request::Batch { comments, .. } if !comments.is_empty())
}

/// A request encoded as its line, with the id it carries.
#[derive(Debug)]
struct Outgoing {
    id: u64,
    bytes: Vec<u8>,
}

/// Number `request` with the next id and encode it, for `wall` (see
/// [`RequestLine::wall`]) with `comment` (see [`RequestLine::comment`]).
fn line_for(
    next_id: &mut u64,
    request: Request,
    (wall, comment): (Option<u64>, Option<&str>),
) -> Result<Outgoing, LinkError> {
    let id = *next_id;
    *next_id += 1;
    let bytes = encode_line(&RequestLine {
        id,
        wall,
        comment: comment.map(str::to_string),
        request,
    })
    .map_err(|err| LinkError::Unsendable(err.to_string()))?;
    Ok(Outgoing { id, bytes })
}

/// Read one line of at most `max` bytes (newline excluded), as
/// [`kazoo_wall::protocol::codec::read_line_capped`] does for blocking
/// readers.
async fn read_line_capped(
    reader: &mut BufReader<OwnedReadHalf>,
    max: usize,
) -> io::Result<LineRead> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(if line.is_empty() {
                LineRead::End
            } else {
                LineRead::Line(line)
            });
        }
        let (taken, done) = available
            .iter()
            .position(|&byte| byte == b'\n')
            .map_or((available.len(), false), |at| (at, true));
        if line.len() + taken > max {
            return Ok(LineRead::TooLong);
        }
        line.extend_from_slice(&available[..taken]);
        reader.consume(if done { taken + 1 } else { taken });
        if done {
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return Ok(LineRead::Line(line));
        }
    }
}

/// A result as the type it should be.
fn decode<T: DeserializeOwned>(value: serde_json::Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|err| {
        format!(
            "the wall's answer could not be read ({err}); kazoo-mcp and kazoo-wall should be \
             built from the same tree"
        )
    })
}

fn timed_out(within: Duration) -> String {
    format!(
        "the wall did not answer within {} seconds",
        within.as_secs()
    )
}

/// The tools' line to the wall.
#[derive(Debug)]
pub struct Wall {
    socket: PathBuf,
    seat: String,
    connection: Mutex<Option<Connection>>,
    /// A connection of its own for `speak`, whose answer waits for the
    /// words to render: nothing else waits behind it.
    speaking: Mutex<Option<Connection>>,
    /// The wall this seat last looked at ([`NOT_LOOKED`] before its first
    /// look): sent with every request, so a change meant for a wall that
    /// has since been replaced is refused rather than landing on the new
    /// wall's modules.
    seen: AtomicU64,
    /// Whether this seat has left the wall: nothing more is sent, and a
    /// request on its way is cut off (see [`Self::close`]).
    closed: watch::Sender<bool>,
}

/// [`Wall::seen`] before the seat has looked.
const NOT_LOOKED: u64 = u64::MAX;

/// A result, and whether it could be read as the type asked for.
#[derive(Debug)]
pub enum Answer<T> {
    /// The result.
    Read(T),
    /// The daemon answered, but in a shape this build cannot read: why, and
    /// the answer as it came.
    Unread(String, serde_json::Value),
}

impl Wall {
    /// The line for `seat` to the daemon on `socket`. Nothing connects until
    /// the first request.
    #[must_use]
    pub fn new(socket: PathBuf, seat: String) -> Self {
        Self {
            socket,
            seat,
            connection: Mutex::const_new(None),
            speaking: Mutex::const_new(None),
            seen: AtomicU64::new(NOT_LOOKED),
            closed: watch::Sender::new(false),
        }
    }

    /// Say hello on the tools' connection, opening it now, and answer the
    /// wall's hello.
    ///
    /// # Errors
    ///
    /// [`LinkError::NotRunning`] while the daemon is down, [`LinkError::Wall`]
    /// for a refused hello (a name taken for the wall's own use, say), and
    /// [`LinkError::Left`] once this seat has left.
    pub async fn hello(&self) -> Result<HelloResult, LinkError> {
        if self.is_closed() {
            return Err(LinkError::Left);
        }
        let (connection, hello) = Connection::open(&self.socket, &self.seat, false).await?;
        *self.connection.lock().await = Some(connection);
        Ok(hello)
    }

    /// Leave the wall: a request on its way is cut off, nothing more is
    /// sent, and both connections are closed, so the wall sees the seat go.
    pub async fn close(&self) {
        self.closed.send_replace(true);
        // Each lock waits for the request using it, which the flag has
        // just cut off.
        self.connection.lock().await.take();
        self.speaking.lock().await.take();
    }

    /// Whether this seat has left the wall.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        *self.closed.borrow()
    }

    /// This seat has looked at wall `wall` (from `wall_look`): its changes
    /// from now on are for that wall.
    pub fn saw(&self, wall: u64) {
        self.seen.store(wall, Ordering::Release);
    }

    /// The wall this seat last looked at, if it has looked.
    fn seen(&self) -> Option<u64> {
        let wall = self.seen.load(Ordering::Acquire);
        (wall != NOT_LOOKED).then_some(wall)
    }

    /// Ask the wall, and read the result as `T`.
    ///
    /// # Errors
    ///
    /// As [`Self::ask_raw`].
    pub async fn ask<T: DeserializeOwned>(&self, request: Request) -> Result<Answer<T>, LinkError> {
        self.ask_commented(request, None).await
    }

    /// Ask the wall, with the sender's `comment` on the change it makes
    /// (see [`RequestLine::comment`]), and read the result as `T`.
    ///
    /// # Errors
    ///
    /// As [`Self::ask_raw`].
    pub async fn ask_commented<T: DeserializeOwned>(
        &self,
        request: Request,
        comment: Option<&str>,
    ) -> Result<Answer<T>, LinkError> {
        let value = self.ask_raw(request, comment).await?;
        Ok(match serde_json::from_value(value.clone()) {
            Ok(read) => Answer::Read(read),
            Err(err) => Answer::Unread(err.to_string(), value),
        })
    }

    /// Ask the wall, with the sender's `comment` on the change it makes,
    /// connecting first if there is no live connection.
    ///
    /// A connection that fails before the request was written, or during a
    /// request that changes nothing, is made again and the request tried
    /// once more; a change whose answer was lost is never sent twice.
    ///
    /// # Errors
    ///
    /// [`LinkError::NotRunning`] while the daemon is down,
    /// [`LinkError::Wall`] for the daemon's refusal, and the others for a
    /// line that failed.
    async fn ask_raw(
        &self,
        request: Request,
        comment: Option<&str>,
    ) -> Result<serde_json::Value, LinkError> {
        // One request at a time on each connection: the lock is held for
        // the whole exchange. Words wait on their own.
        let connection = if matches!(request, Request::Speak { .. }) {
            &self.speaking
        } else {
            &self.connection
        };
        let mut slot = connection.lock().await;
        let mut closed = self.closed.subscribe();
        if *closed.borrow_and_update() {
            slot.take();
            drop(slot);
            return Err(LinkError::Left);
        }
        let changes = request.is_change();
        let answered = tokio::select! {
            answered = self.ask_on(&mut slot, &request, comment) => Some(answered),
            _ = closed.wait_for(|closed| *closed) => None,
        };
        let answered = answered.unwrap_or_else(|| {
            // Cut off mid-request: the connection is out of step.
            slot.take();
            Err(if changes {
                LinkError::Uncertain(
                    "this seat left the wall while the change was on its way".to_string(),
                )
            } else {
                LinkError::Left
            })
        });
        drop(slot);
        answered
    }

    /// [`Self::ask_raw`] on the connection in `slot`.
    async fn ask_on(
        &self,
        slot: &mut Option<Connection>,
        request: &Request,
        comment: Option<&str>,
    ) -> Result<serde_json::Value, LinkError> {
        let changes = request.is_change();
        let mut tried_again = false;
        loop {
            let (connection, reused) = match slot.take() {
                Some(connection) if connection.looks_alive() => (slot.insert(connection), true),
                Some(_) | None => {
                    let (connection, _) = Connection::open(&self.socket, &self.seat, false).await?;
                    (slot.insert(connection), false)
                }
            };
            if !connection.notes
                && matches!(request, Request::Batch {description, attachments, ..} if description.is_some() || !attachments.is_empty())
            {
                return Err(LinkError::Unsendable("this wall is older than batch notes; ask Tom to restart the wall. Nothing was sent".to_string()));
            }
            // A wall older than comments would drop them without a word.
            if !connection.comments && commented(request, comment) {
                return Err(LinkError::Unsendable(OLD_WALL.to_string()));
            }
            let line = line_for(
                &mut connection.next_id,
                request.clone(),
                (self.seen(), comment),
            )?;
            let within = if matches!(request, Request::Speak { .. }) {
                SPEECH_WITHIN
            } else {
                ANSWER_WITHIN
            };
            let answered = tokio::time::timeout(within, connection.exchange(&line)).await;
            let failure = match answered {
                Ok(Ok(Ok(value))) => return Ok(value),
                Ok(Ok(Err(err))) => return Err(LinkError::Wall(err)),
                Ok(Err(failure)) => failure,
                Err(_) => Failure::sent(timed_out(within)),
            };
            // The connection is out of step with the daemon either way.
            *slot = None;
            let safe_to_repeat = !failure.sent || !changes;
            if reused && safe_to_repeat && !tried_again {
                tried_again = true;
                continue;
            }
            return Err(if failure.sent && changes {
                LinkError::Uncertain(failure.why)
            } else {
                LinkError::Broken(failure.why)
            });
        }
    }
}

/// Keep a subscribed connection to the daemon for as long as `feed` is
/// open, trying again every [`RECONNECT_EVERY`] while the daemon is down,
/// and hand it every event, and each time the line comes up or goes down.
pub async fn follow(socket: PathBuf, seat: String, feed: mpsc::UnboundedSender<Feed>) {
    let mut been_up = false;
    let mut said_waiting = false;
    loop {
        match subscribe(&socket, &seat).await {
            Ok((mut connection, hello, subscribed)) => {
                eprintln!(
                    "kazoo-mcp: {seat} is on the wall at {} ({} seats here)",
                    socket.display(),
                    subscribed.seats.len()
                );
                said_waiting = false;
                let up = Feed::Up {
                    seats: subscribed.seats,
                    revision: hello.revision,
                    again: been_up,
                };
                been_up = true;
                if feed.send(up).is_err() {
                    return;
                }
                let why = loop {
                    match connection.read().await {
                        Ok(ServerLine::Event(event)) => {
                            if feed.send(Feed::Event(event)).is_err() {
                                return;
                            }
                        }
                        Ok(ServerLine::Response(response)) => eprintln!(
                            "kazoo-mcp: the wall answered request {} on the event line, \
                             which asked nothing; ignored",
                            response.id
                        ),
                        Err(why) => break why,
                    }
                };
                eprintln!("kazoo-mcp: the line to the wall dropped ({why}); trying again");
                if feed.send(Feed::Down).is_err() {
                    return;
                }
            }
            Err(LinkError::NotRunning) => {
                if !said_waiting {
                    eprintln!(
                        "kazoo-mcp: the wall is not running at {}; trying every {} seconds",
                        socket.display(),
                        RECONNECT_EVERY.as_secs()
                    );
                    said_waiting = true;
                }
            }
            Err(err) => eprintln!("kazoo-mcp: the event line could not be opened: {err}"),
        }
        if feed.is_closed() {
            return;
        }
        tokio::time::sleep(RECONNECT_EVERY).await;
    }
}

/// The name a look at a wall says hello with: a watcher's, never listed.
const PEEK_AS: &str = "kazoo-mcp";

/// Look at the wall on `socket` without joining it: say hello as a watcher
/// and go. The seats in its answer are the others, never this look.
///
/// # Errors
///
/// [`LinkError::NotRunning`] while the daemon is down, and the others for a
/// line that failed or a hello refused.
pub async fn peek(socket: &Path) -> Result<HelloResult, LinkError> {
    Connection::open(socket, PEEK_AS, true)
        .await
        .map(|(_, hello)| hello)
}

/// Connect, say hello and subscribe.
async fn subscribe(
    socket: &Path,
    seat: &str,
) -> Result<(Connection, HelloResult, SubscribeResult), LinkError> {
    let (mut connection, hello) = Connection::open(socket, seat, false).await?;
    let line = line_for(&mut connection.next_id, Request::Subscribe, (None, None))?;
    let answered = tokio::time::timeout(ANSWER_WITHIN, connection.exchange(&line)).await;
    let value = match answered {
        Ok(Ok(Ok(value))) => value,
        Ok(Ok(Err(err))) => return Err(LinkError::Wall(err)),
        Ok(Err(failure)) => return Err(LinkError::Broken(failure.why)),
        Err(_) => return Err(LinkError::Broken(timed_out(ANSWER_WITHIN))),
    };
    let subscribed = decode(value).map_err(LinkError::Broken)?;
    Ok((connection, hello, subscribed))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use kazoo_wall::protocol::ErrorCode;
    use tokio::net::UnixListener;

    use super::*;

    /// A stand-in daemon: says hello, answers every request but `speak` at
    /// once with an empty object, and never answers `speak`.
    async fn stub(listener: UnixListener) {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut lines = BufReader::new(read).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let asked: serde_json::Value = serde_json::from_str(&line).unwrap();
                    let result = match asked["op"].as_str() {
                        Some("speak") => continue,
                        Some("hello") => serde_json::json!({
                            "daemon": "stub",
                            "seat": "Tom",
                            "console": false,
                            "seats": ["Tom"],
                            "revision": 0,
                        }),
                        _ => serde_json::json!({}),
                    };
                    let answer =
                        serde_json::json!({"id": asked["id"], "ok": true, "result": result});
                    write
                        .write_all(format!("{answer}\n").as_bytes())
                        .await
                        .unwrap();
                }
            });
        }
    }

    #[tokio::test]
    async fn notes_never_go_to_a_wall_without_the_capability() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("wall.sock");
        tokio::spawn(stub(UnixListener::bind(&socket).unwrap()));
        let wall = Wall::new(socket, "Tom".to_string());
        for fields in [
            serde_json::json!({"description":"why"}),
            serde_json::json!({"attachments":[{"name":"proof.md","text":"QED"}]}),
        ] {
            let mut value = serde_json::json!({"op":"batch","note":"proof","changes":[{"op":"tempo","bpm":140.0}]});
            value
                .as_object_mut()
                .unwrap()
                .extend(fields.as_object().unwrap().clone());
            let refused = wall
                .ask_raw(serde_json::from_value(value).unwrap(), None)
                .await
                .unwrap_err();
            assert!(
                matches!(&refused, LinkError::Unsendable(reason) if reason.contains("restart the wall")),
                "{refused:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_comment_is_never_sent_to_a_wall_that_would_drop_it() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("wall.sock");
        // The stub's hello says nothing of comments: a wall older than them.
        tokio::spawn(stub(UnixListener::bind(&socket).unwrap()));
        let wall = Wall::new(socket, "Tom".to_string());
        let turn = Request::turn("vcf1".to_string(), "cutoff".to_string(), 500.0, None);
        let refused = wall
            .ask_raw(turn.clone(), Some("darker"))
            .await
            .unwrap_err();
        assert_eq!(refused, LinkError::Unsendable(OLD_WALL.to_string()));
        let batch = Request::Batch {
            note: "two".to_string(),
            description: None,
            attachments: Vec::new(),
            changes: vec![turn.clone(), turn.clone()],
            comments: vec![Some("first".to_string()), None],
        };
        assert_eq!(
            wall.ask_raw(batch, None).await.unwrap_err(),
            LinkError::Unsendable(OLD_WALL.to_string())
        );
        // Without one, it goes as ever.
        assert!(wall.ask_raw(turn, None).await.is_ok());
    }

    #[tokio::test]
    async fn other_tools_never_wait_behind_words_being_rendered() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("wall.sock");
        tokio::spawn(stub(UnixListener::bind(&socket).unwrap()));
        let wall = Arc::new(Wall::new(socket, "Tom".to_string()));
        let speaking = Arc::clone(&wall);
        let speak = tokio::spawn(async move {
            speaking
                .ask_raw(
                    Request::Speak {
                        module: "speak1".to_string(),
                        text: "hello".to_string(),
                        voice: None,
                    },
                    None,
                )
                .await
        });
        // Let the speak go out first.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let look =
            tokio::time::timeout(Duration::from_secs(2), wall.ask_raw(Request::Look, None)).await;
        assert!(
            matches!(look, Ok(Ok(_))),
            "look waited behind the speak: {look:?}"
        );
        assert!(!speak.is_finished());
        speak.abort();
    }

    #[tokio::test]
    async fn leaving_cuts_off_a_request_on_its_way_and_sends_nothing_more() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("wall.sock");
        tokio::spawn(stub(UnixListener::bind(&socket).unwrap()));
        let wall = Arc::new(Wall::new(socket, "Tom".to_string()));
        assert!(wall.hello().await.is_ok());
        let speaking = Arc::clone(&wall);
        // The stand-in never answers speak: it is still on its way.
        let speak = tokio::spawn(async move {
            speaking
                .ask_raw(
                    Request::Speak {
                        module: "speak1".to_string(),
                        text: "hello".to_string(),
                        voice: None,
                    },
                    None,
                )
                .await
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        tokio::time::timeout(Duration::from_secs(2), wall.close())
            .await
            .expect("closing waits for nothing but the cut");
        let cut = tokio::time::timeout(Duration::from_secs(2), speak)
            .await
            .expect("the speak was cut off")
            .unwrap();
        assert_eq!(
            cut.unwrap_err(),
            LinkError::Uncertain(
                "this seat left the wall while the change was on its way".to_string()
            ),
            "a change cut off may have been made"
        );
        assert!(wall.connection.lock().await.is_none());
        assert!(wall.speaking.lock().await.is_none());
        assert_eq!(
            wall.ask_raw(Request::Look, None).await.unwrap_err(),
            LinkError::Left
        );
        assert_eq!(wall.hello().await.unwrap_err(), LinkError::Left);
    }

    #[test]
    fn a_timeout_says_how_long_was_waited() {
        assert!(timed_out(ANSWER_WITHIN).contains(&format!("{} seconds", ANSWER_WITHIN.as_secs())));
        assert!(timed_out(SPEECH_WITHIN).contains(&format!("{} seconds", SPEECH_WITHIN.as_secs())));
    }

    #[test]
    fn every_error_code_has_its_wire_name() {
        for code in ErrorCode::ALL {
            let wire = serde_json::to_value(code).unwrap();
            assert_eq!(wire.as_str(), Some(code_name(code)));
        }
    }

    #[test]
    fn errors_read_as_sentences() {
        assert_eq!(LinkError::NotRunning.to_string(), NOT_RUNNING);
        let refused = LinkError::Wall(WallError::new(
            ErrorCode::UnknownKnob,
            "vcf1 has no knob 'cutof'; knobs: cutoff, resonance",
        ));
        assert_eq!(
            refused.to_string(),
            "the wall said no (unknown_knob): vcf1 has no knob 'cutof'; knobs: cutoff, resonance"
        );
        assert!(
            LinkError::Uncertain("gone".to_string())
                .to_string()
                .contains("check wall_log")
        );
    }

    #[test]
    fn requests_too_long_for_the_protocol_are_not_sent() {
        let mut next = 7;
        let huge = Request::add("x".repeat(kazoo_wall::protocol::MAX_LINE), None, None, None);
        assert!(matches!(
            line_for(&mut next, huge, (None, None)),
            Err(LinkError::Unsendable(_))
        ));
        let line = line_for(&mut next, Request::Look, (None, None)).unwrap();
        assert_eq!(line.id, 8);
        assert_eq!(line.bytes, b"{\"id\":8,\"op\":\"look\"}\n");
    }

    #[tokio::test]
    async fn a_missing_socket_is_a_wall_that_is_not_running() {
        let dir = tempfile::tempdir().unwrap();
        let wall = Wall::new(dir.path().join("none.sock"), "Tom".to_string());
        assert_eq!(
            wall.ask_raw(Request::Look, None).await.unwrap_err(),
            LinkError::NotRunning
        );
    }

    #[tokio::test]
    async fn lines_are_read_whole_and_capped() {
        let (ours, mut theirs) = UnixStream::pair().unwrap();
        let (read, _write) = ours.into_split();
        let mut reader = BufReader::with_capacity(4, read);
        theirs.write_all(b"ab\r\ncdef\n0123456789\n").await.unwrap();
        drop(theirs);
        assert_eq!(
            read_line_capped(&mut reader, 8).await.unwrap(),
            LineRead::Line(b"ab".to_vec())
        );
        assert_eq!(
            read_line_capped(&mut reader, 8).await.unwrap(),
            LineRead::Line(b"cdef".to_vec())
        );
        assert_eq!(
            read_line_capped(&mut reader, 8).await.unwrap(),
            LineRead::TooLong
        );
    }
}
