//! A blocking client for the daemon's control socket.
//!
//! [`WallClient`] says hello on connecting and then makes requests, each
//! answered by its response. [`Subscription`] is a second connection that
//! subscribes and yields [`Event`]s; a console typically runs it on its own
//! thread and forwards the events to its UI.
//!
//! ```no_run
//! use kazoo_wall::protocol::client::{Subscription, WallClient};
//!
//! let socket = kazoo_wall::paths::socket_path();
//! let mut wall = WallClient::connect(&socket, "Tom", "my-console 0.1", true)?;
//! let look = wall.look()?;
//! println!("{} modules at {} BPM", look.modules.len(), look.tempo);
//! wall.turn("vcf1", "cutoff", 800.0, Some(4.0))?;
//!
//! let (mut events, _) = Subscription::open(&socket, "Tom", "my-console 0.1", true)?;
//! let event = events.next_event()?;
//! # Ok::<(), kazoo_wall::protocol::client::ClientError>(())
//! ```

use std::collections::VecDeque;
use std::fmt;
use std::io::{self, BufReader};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use serde::de::DeserializeOwned;

use super::codec::{self, LineRead};
use super::{
    ArrangeResult, CatalogueResult, ChangeResult, Event, HelloResult, ListenResult, LogPage,
    MAX_SERVER_LINE, MonitorResult, Place, RecordResult, Request, RequestLine, Response,
    ServerLine, ShutdownResult, Snapshot, SubscribeResult, TempoResult, WallError,
};

/// Why a request did not produce a result.
#[derive(Debug)]
pub enum ClientError {
    /// The socket failed, or a read timed out (see
    /// [`ClientError::is_timeout`]).
    Io(io::Error),
    /// The daemon answered with an error.
    Wall(WallError),
    /// The daemon said something this client does not understand.
    Protocol(String),
    /// The daemon closed the connection.
    Closed,
}

impl ClientError {
    /// Whether this is a read that timed out (after
    /// [`WallClient::set_timeout`] or [`Subscription::set_timeout`]).
    #[must_use]
    pub fn is_timeout(&self) -> bool {
        matches!(self, Self::Io(err) if matches!(err.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut))
    }

    /// The daemon's error, if it answered with one.
    #[must_use]
    pub const fn wall_error(&self) -> Option<&WallError> {
        match self {
            Self::Wall(err) => Some(err),
            Self::Io(_) | Self::Protocol(_) | Self::Closed => None,
        }
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "the wall's socket failed: {err}"),
            Self::Wall(err) => write!(f, "{}", err.message),
            Self::Protocol(why) => write!(f, "the wall said something unexpected: {why}"),
            Self::Closed => write!(f, "the wall closed the connection"),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<io::Error> for ClientError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

/// One connection to the daemon, after hello.
#[derive(Debug)]
pub struct WallClient {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    next_id: u64,
    hello: HelloResult,
    events: VecDeque<Event>,
}

impl WallClient {
    /// Connect to the daemon on `socket` and say hello as `seat`, from
    /// software `client`; a `console` may shut the daemon down.
    ///
    /// # Errors
    ///
    /// Fails if the socket cannot be reached, or the daemon refuses the
    /// hello (a bad seat name, too many connections).
    pub fn connect(
        socket: &Path,
        seat: &str,
        client: &str,
        console: bool,
    ) -> Result<Self, ClientError> {
        Self::hello_as(socket, seat, client, console, false)
    }

    /// Connect as a watcher named `name`: it sees everything, is never
    /// listed or announced, and may change nothing.
    ///
    /// # Errors
    ///
    /// As [`Self::connect`].
    pub fn watch(socket: &Path, name: &str, client: &str) -> Result<Self, ClientError> {
        Self::hello_as(socket, name, client, false, true)
    }

    fn hello_as(
        socket: &Path,
        seat: &str,
        client: &str,
        console: bool,
        watcher: bool,
    ) -> Result<Self, ClientError> {
        let stream = UnixStream::connect(socket)?;
        let writer = stream.try_clone()?;
        let mut wall = Self {
            reader: BufReader::new(stream),
            writer,
            next_id: 1,
            hello: HelloResult {
                daemon: String::new(),
                seat: seat.to_string(),
                console,
                seats: Vec::new(),
                revision: 0,
                watcher,
            },
            events: VecDeque::new(),
        };
        wall.hello = wall.call_as(Request::Hello {
            seat: seat.to_string(),
            client: client.to_string(),
            console,
            watcher,
        })?;
        Ok(wall)
    }

    /// What the daemon said to hello.
    #[must_use]
    pub const fn hello(&self) -> &HelloResult {
        &self.hello
    }

    /// Give up on a read after `timeout` (`None` waits for ever).
    ///
    /// # Errors
    ///
    /// Fails if the socket refuses the setting (a zero duration).
    pub fn set_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.reader.get_ref().set_read_timeout(timeout)
    }

    /// Send `request` and wait for its response's result. Events that
    /// arrive meanwhile (on a subscribed connection) are kept for
    /// [`Self::take_events`].
    ///
    /// # Errors
    ///
    /// [`ClientError::Wall`] for the daemon's error; the others for a
    /// broken or confused connection.
    pub fn call(&mut self, request: Request) -> Result<serde_json::Value, ClientError> {
        let id = self.next_id;
        self.next_id += 1;
        codec::write_line(&mut self.writer, &RequestLine { id, request })?;
        loop {
            match self.read()? {
                ServerLine::Event(event) => self.events.push_back(event),
                ServerLine::Response(response) if response.id == id => {
                    return result(response);
                }
                ServerLine::Response(response) => {
                    return Err(ClientError::Protocol(format!(
                        "a response to request {} while waiting for {id}",
                        response.id
                    )));
                }
            }
        }
    }

    /// Events that arrived while waiting for responses.
    pub fn take_events(&mut self) -> Vec<Event> {
        self.events.drain(..).collect()
    }

    fn call_as<T: DeserializeOwned>(&mut self, request: Request) -> Result<T, ClientError> {
        let value = self.call(request)?;
        serde_json::from_value(value).map_err(|err| ClientError::Protocol(err.to_string()))
    }

    fn read(&mut self) -> Result<ServerLine, ClientError> {
        match codec::read_line_capped(&mut self.reader, MAX_SERVER_LINE)? {
            LineRead::Line(bytes) => {
                let text = String::from_utf8(bytes)
                    .map_err(|_| ClientError::Protocol("a line that is not UTF-8".to_string()))?;
                ServerLine::parse(&text).map_err(|err| ClientError::Protocol(err.to_string()))
            }
            LineRead::End => Err(ClientError::Closed),
            LineRead::TooLong => Err(ClientError::Protocol(
                "a line longer than the protocol allows".to_string(),
            )),
        }
    }

    /// The whole wall.
    ///
    /// # Errors
    ///
    /// As [`Self::call`].
    pub fn look(&mut self) -> Result<Snapshot, ClientError> {
        self.call_as(Request::Look)
    }

    /// Every module kind.
    ///
    /// # Errors
    ///
    /// As [`Self::call`].
    pub fn catalogue(&mut self) -> Result<CatalogueResult, ClientError> {
        self.call_as(Request::Catalogue)
    }

    /// Turn `module`'s `knob` to `value`, gliding over `glide_beats` (the
    /// daemon's default of 2 beats when `None`).
    ///
    /// # Errors
    ///
    /// As [`Self::call`].
    pub fn turn(
        &mut self,
        module: &str,
        knob: &str,
        value: f64,
        glide_beats: Option<f64>,
    ) -> Result<ChangeResult, ClientError> {
        self.call_as(Request::Turn {
            module: module.to_string(),
            knob: knob.to_string(),
            value,
            glide_beats,
        })
    }

    /// Plug output `from` (`lfo1.out`) into input or knob jack `to`
    /// (`vcf1.cutoff`).
    ///
    /// # Errors
    ///
    /// As [`Self::call`].
    pub fn patch(
        &mut self,
        from: &str,
        to: &str,
        amount: Option<f64>,
    ) -> Result<ChangeResult, ClientError> {
        self.call_as(Request::Patch {
            from: from.to_string(),
            to: to.to_string(),
            amount,
        })
    }

    /// Unplug cable number `cable`.
    ///
    /// # Errors
    ///
    /// As [`Self::call`].
    pub fn unpatch_cable(&mut self, cable: u32) -> Result<ChangeResult, ClientError> {
        self.call_as(Request::Unpatch {
            cable: Some(cable),
            to: None,
        })
    }

    /// Unplug whatever is plugged into `to`.
    ///
    /// # Errors
    ///
    /// As [`Self::call`].
    pub fn unpatch_input(&mut self, to: &str) -> Result<ChangeResult, ClientError> {
        self.call_as(Request::Unpatch {
            cable: None,
            to: Some(to.to_string()),
        })
    }

    /// Add a module of `kind`, optionally with a display name. The result's
    /// `module` is its id.
    ///
    /// # Errors
    ///
    /// As [`Self::call`].
    pub fn add(&mut self, kind: &str, name: Option<&str>) -> Result<ChangeResult, ClientError> {
        self.call_as(Request::Add {
            kind: kind.to_string(),
            name: name.map(str::to_string),
            place: None,
        })
    }

    /// Move `module` to `place` on the rack. The result is the rows now.
    ///
    /// # Errors
    ///
    /// As [`Self::call`].
    pub fn arrange(&mut self, module: &str, place: Place) -> Result<ArrangeResult, ClientError> {
        self.call_as(Request::Arrange {
            module: module.to_string(),
            row: place.row,
            before: place.before,
            own: place.own,
        })
    }

    /// Take `module` away, with its cables.
    ///
    /// # Errors
    ///
    /// As [`Self::call`].
    pub fn remove(&mut self, module: &str) -> Result<ChangeResult, ClientError> {
        self.call_as(Request::Remove {
            module: module.to_string(),
        })
    }

    /// Undo change `change`.
    ///
    /// # Errors
    ///
    /// As [`Self::call`].
    pub fn undo(&mut self, change: u64) -> Result<ChangeResult, ClientError> {
        self.call_as(Request::Undo { change })
    }

    /// Up to `limit` changes before `before`, oldest first.
    ///
    /// # Errors
    ///
    /// As [`Self::call`].
    pub fn log(&mut self, before: Option<u64>, limit: Option<u32>) -> Result<LogPage, ClientError> {
        self.call_as(Request::Log { before, limit })
    }

    /// What the wall sounds like.
    ///
    /// # Errors
    ///
    /// As [`Self::call`].
    pub fn listen(&mut self) -> Result<ListenResult, ClientError> {
        self.call_as(Request::Listen)
    }

    /// Set the tempo (asking the desk, when the wall is on it).
    ///
    /// # Errors
    ///
    /// As [`Self::call`].
    pub fn tempo(&mut self, bpm: f64) -> Result<TempoResult, ClientError> {
        self.call_as(Request::Tempo { bpm })
    }

    /// Give `speak` module `module` words to say, in `voice` if given. The
    /// answer comes once the words are rendered and ready to play, which
    /// can take a few seconds.
    ///
    /// # Errors
    ///
    /// As [`Self::call`]: `bad_request` for words, a voice or a module that
    /// cannot speak, `internal` on a machine without `say`.
    pub fn speak(
        &mut self,
        module: &str,
        text: &str,
        voice: Option<&str>,
    ) -> Result<ChangeResult, ClientError> {
        self.call_as(Request::Speak {
            module: module.to_string(),
            text: text.to_string(),
            voice: voice.map(str::to_string),
        })
    }

    /// Make the wall heard (`on`) or silent. Consoles only.
    ///
    /// # Errors
    ///
    /// As [`Self::call`]; `not_allowed` from a seat that is not a console.
    pub fn monitor(&mut self, on: bool) -> Result<MonitorResult, ClientError> {
        self.call_as(Request::Monitor { on })
    }

    /// Start (`on`) or stop recording what the wall plays.
    ///
    /// # Errors
    ///
    /// As [`Self::call`]; `internal` when the file cannot be made,
    /// `not_allowed` from a watcher.
    pub fn record(&mut self, on: bool) -> Result<RecordResult, ClientError> {
        self.call_as(Request::Record { on })
    }

    /// Stop the daemon. Consoles only.
    ///
    /// # Errors
    ///
    /// As [`Self::call`]; `not_allowed` from a seat that is not a console.
    pub fn shutdown(&mut self) -> Result<ShutdownResult, ClientError> {
        self.call_as(Request::Shutdown)
    }

    /// Subscribe this connection to events; they then arrive through
    /// [`Self::take_events`] and [`Self::next_event`].
    ///
    /// # Errors
    ///
    /// As [`Self::call`].
    pub fn subscribe(&mut self) -> Result<SubscribeResult, ClientError> {
        self.call_as(Request::Subscribe)
    }

    /// The next event on a subscribed connection: one already received, or
    /// the next to arrive (waiting up to the read timeout).
    ///
    /// # Errors
    ///
    /// A timeout (see [`ClientError::is_timeout`]), a broken connection,
    /// or a response where an event was expected.
    pub fn next_event(&mut self) -> Result<Event, ClientError> {
        if let Some(event) = self.events.pop_front() {
            return Ok(event);
        }
        match self.read()? {
            ServerLine::Event(event) => Ok(event),
            ServerLine::Response(response) => Err(ClientError::Protocol(format!(
                "a response to request {} with no request waiting",
                response.id
            ))),
        }
    }
}

fn result(response: Response) -> Result<serde_json::Value, ClientError> {
    if response.ok {
        Ok(response.result.unwrap_or(serde_json::Value::Null))
    } else {
        Err(response.error.map_or_else(
            || ClientError::Protocol("a failed response with no error".to_string()),
            ClientError::Wall,
        ))
    }
}

/// A connection that only listens for events.
#[derive(Debug)]
pub struct Subscription {
    client: WallClient,
}

impl Subscription {
    /// Connect, say hello and subscribe. Returns the subscription and the
    /// seats online when it started.
    ///
    /// # Errors
    ///
    /// As [`WallClient::connect`] and [`WallClient::subscribe`].
    pub fn open(
        socket: &Path,
        seat: &str,
        client: &str,
        console: bool,
    ) -> Result<(Self, SubscribeResult), ClientError> {
        let mut client = WallClient::connect(socket, seat, client, console)?;
        let subscribed = client.subscribe()?;
        Ok((Self { client }, subscribed))
    }

    /// Connect as a watcher named `name` and subscribe (see
    /// [`WallClient::watch`]).
    ///
    /// # Errors
    ///
    /// As [`WallClient::watch`] and [`WallClient::subscribe`].
    pub fn watch(
        socket: &Path,
        name: &str,
        client: &str,
    ) -> Result<(Self, SubscribeResult), ClientError> {
        let mut client = WallClient::watch(socket, name, client)?;
        let subscribed = client.subscribe()?;
        Ok((Self { client }, subscribed))
    }

    /// Give up waiting for an event after `timeout` (`None` waits for
    /// ever).
    ///
    /// # Errors
    ///
    /// Fails if the socket refuses the setting (a zero duration).
    pub fn set_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.client.set_timeout(timeout)
    }

    /// The next event, waiting up to the timeout.
    ///
    /// # Errors
    ///
    /// As [`WallClient::next_event`].
    pub fn next_event(&mut self) -> Result<Event, ClientError> {
        self.client.next_event()
    }

    /// A handle that ends the subscription from another thread: a blocked
    /// [`Self::next_event`] then returns [`ClientError::Closed`].
    ///
    /// # Errors
    ///
    /// Fails if the socket cannot be duplicated.
    pub fn closer(&self) -> io::Result<Closer> {
        Ok(Closer {
            stream: self.client.writer.try_clone()?,
        })
    }
}

/// Ends a [`Subscription`] from another thread.
#[derive(Debug)]
pub struct Closer {
    stream: UnixStream,
}

impl Closer {
    /// End the subscription.
    ///
    /// # Errors
    ///
    /// Fails if the socket cannot be shut down (it is already closed).
    pub fn close(&self) -> io::Result<()> {
        self.stream.shutdown(Shutdown::Both)
    }
}
