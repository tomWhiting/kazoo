//! The control protocol: newline-delimited JSON over the daemon's Unix
//! socket.
//!
//! Every client sends [`Request::Hello`] first. Each request line carries a
//! client-chosen `id` and gets exactly one [`Response`] with the same id.
//! After [`Request::Subscribe`], [`Event`]s are pushed on that connection
//! too; a line with an `"event"` key is an event, any other line is a
//! response (see [`ServerLine::parse`]).
//!
//! Request lines are at most [`MAX_LINE`] bytes; a longer one closes the
//! connection with a `bad_request` error. The daemon's lines are at most
//! [`MAX_SERVER_LINE`]. [`codec`] reads and writes lines with that
//! cap, and [`client`] is a blocking client for consoles and tests.
//!
//! Every name that reaches another seat (seat names, module display names)
//! passes [`valid_name`]; change summaries are built only from sanitised
//! ids, names, numbers and units.

pub mod client;
pub mod codec;

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::catalogue::{Curve, JackLaw, Signal};
use crate::fingerprints::{Fingerprints, Shares};

/// Longest request line a client may send, in bytes (newline excluded). A
/// longer one closes the connection with a `bad_request` error.
pub const MAX_LINE: usize = 64 * 1024;

/// Longest line the daemon sends. Requests are capped at [`MAX_LINE`]; the
/// daemon's own answers can be longer (a full wall's snapshot), and this
/// bound keeps clients safe from a runaway one.
pub const MAX_SERVER_LINE: usize = 4 * 1024 * 1024;

/// The daemon's socket file name, in kazoo's runtime directory.
pub const SOCKET_NAME: &str = "kazoo-wall.sock";

/// Longest seat or module display name.
pub const MAX_NAME: usize = 24;

/// The daemon's name and version, as it reports itself in `hello`.
pub const DAEMON: &str = concat!("kazoo-wall ", env!("CARGO_PKG_VERSION"));

/// Whether `name` is a valid seat or display name: 1 to 24 characters from
/// `A-Z a-z 0-9 space _ . -`.
#[must_use]
pub fn valid_name(name: &str) -> bool {
    (1..=MAX_NAME).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b' ' | b'_' | b'.' | b'-'))
}

/// A knob value or cable amount (held as `f32`) as the number people
/// meant: the shortest decimal that reads back as the same `f32`, so 0.35
/// travels as `0.35` rather than `0.3499999940395355`.
#[must_use]
pub fn widen(value: f32) -> f64 {
    if !value.is_finite() {
        return f64::from(value);
    }
    // The shortest round-trip form of a finite f32 always parses as f64.
    value
        .to_string()
        .parse()
        .unwrap_or_else(|_| f64::from(value))
}

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

/// One request line: an id and the request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestLine {
    /// Chosen by the client; echoed in the response.
    pub id: u64,
    /// What is asked.
    #[serde(flatten)]
    pub request: Request,
}

/// What a client asks of the wall.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Say who is here. Must come first.
    Hello {
        /// The seat's name (see [`valid_name`]).
        seat: String,
        /// The client software, for the daemon's log.
        client: String,
        /// A console (Tom's TUI) may shut the daemon down.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        console: bool,
        /// A watcher (a visualiser) sees everything but is not listed among
        /// the seats, is never announced, and may change nothing.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        watcher: bool,
    },
    /// The whole wall: a [`Snapshot`].
    Look,
    /// Every module kind: a [`CatalogueResult`].
    Catalogue,
    /// Turn a knob, gliding over `glide_beats` (default 2, 0 to 64).
    Turn {
        /// Module id, e.g. `vcf1`.
        module: String,
        /// Knob name, e.g. `cutoff`.
        knob: String,
        /// New value; held to the knob's range.
        value: f64,
        /// Glide length in beats.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        glide_beats: Option<f64>,
    },
    /// Plug an output into an input, replacing any cable already in it.
    Patch {
        /// Output, e.g. `lfo1.out`.
        from: String,
        /// Input, e.g. `vcf1.cutoff`.
        to: String,
        /// Attenuverter, -1 to 1 (default 1).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        amount: Option<f64>,
    },
    /// Unplug a cable, by its number or by the input it is plugged into.
    Unpatch {
        /// Cable number.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cable: Option<u32>,
        /// Input, e.g. `vcf1.cutoff`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<String>,
    },
    /// Add a module.
    Add {
        /// Kind, e.g. `lfo`.
        kind: String,
        /// Display name (see [`valid_name`]).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// Where on the rack it goes. Left out, it goes at the end of the
        /// row holding the newest module of its sort, or on a new bottom
        /// row. (A wall older than the rack's rows ignores it.)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        place: Option<Place>,
    },
    /// Take a module away, with its cables.
    Remove {
        /// Module id.
        module: String,
    },
    /// Move a module to another place on the rack: an [`ArrangeResult`].
    /// The rack's rows are shared by every console, like a real case, but
    /// a move is not a change to the sound: it is not logged, cannot be
    /// undone from the log, and is told to subscribers as [`Event::Rack`].
    Arrange {
        /// Module id.
        module: String,
        /// The row it goes to (see [`Place`]).
        row: usize,
        /// The module it goes in front of, in that row.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<String>,
        /// A new row of its own, at `row`.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        own: bool,
    },
    /// Apply the inverse of a change, as a new change.
    Undo {
        /// The change's sequence number.
        change: u64,
    },
    /// Give a `speak` module words to say, rendered off the audio thread
    /// (macOS `say`). Answered once the words are ready to play (the
    /// module plays them on its gate), which can take a few seconds. The
    /// words themselves stay private: other seats hear that the module
    /// was given words, never what they are.
    Speak {
        /// A `speak` module's id.
        module: String,
        /// The words: at most 500 characters.
        text: String,
        /// A voice `say` knows, e.g. `Samantha`; the system's own when
        /// left out.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        voice: Option<String>,
    },
    /// Recent changes: a [`LogPage`].
    Log {
        /// Only changes before this sequence number (default: the latest).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<u64>,
        /// At most this many (default 50, at most 500).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
    },
    /// What the wall sounds like: a [`ListenResult`].
    Listen,
    /// Set the tempo: asks the desk when the wall is on it.
    Tempo {
        /// Beats per minute, 20 to 300.
        bpm: f64,
    },
    /// Push [`Event`]s on this connection from now on.
    Subscribe,
    /// Make the wall heard or silent: a [`MonitorResult`]. Silent, it goes
    /// on playing, listening and changing; nothing leaves it. Console only.
    Monitor {
        /// Heard when true.
        on: bool,
    },
    /// Start or stop recording what the wall plays to a WAV file: a
    /// [`RecordResult`]. The recording hears the wall even while it is
    /// silent. Any seat but a watcher.
    Record {
        /// Start when true, stop when false.
        on: bool,
    },
    /// Stop the daemon. Console only.
    Shutdown,
}

impl Request {
    /// The request's `op` name.
    #[must_use]
    pub const fn op(&self) -> &'static str {
        match self {
            Self::Hello { .. } => "hello",
            Self::Look => "look",
            Self::Catalogue => "catalogue",
            Self::Turn { .. } => "turn",
            Self::Patch { .. } => "patch",
            Self::Unpatch { .. } => "unpatch",
            Self::Add { .. } => "add",
            Self::Remove { .. } => "remove",
            Self::Arrange { .. } => "arrange",
            Self::Undo { .. } => "undo",
            Self::Speak { .. } => "speak",
            Self::Log { .. } => "log",
            Self::Listen => "listen",
            Self::Tempo { .. } => "tempo",
            Self::Subscribe => "subscribe",
            Self::Monitor { .. } => "monitor",
            Self::Record { .. } => "record",
            Self::Shutdown => "shutdown",
        }
    }

    /// Whether the request changes the wall (and so counts against the
    /// seat's flood guard).
    #[must_use]
    pub const fn is_change(&self) -> bool {
        matches!(
            self,
            Self::Turn { .. }
                | Self::Patch { .. }
                | Self::Unpatch { .. }
                | Self::Add { .. }
                | Self::Remove { .. }
                | Self::Arrange { .. }
                | Self::Undo { .. }
                | Self::Tempo { .. }
                | Self::Speak { .. }
                | Self::Record { .. }
        )
    }
}

/// A place on the rack, as the rows stand when it is asked for.
///
/// It is in row `row` (counting from 0 at the top), before module
/// `before`, or at the row's end. A row past the last is a new bottom row;
/// `own` makes a new row of its own at `row`, pushing that row and those
/// below it down. When `before` has gone from the wall (another seat moved
/// or removed it), the place is the end of row `row`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    /// The row.
    pub row: usize,
    /// The module it goes in front of, in that row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    /// A new row of its own, at `row`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub own: bool,
}

impl Place {
    /// The end of row `row`.
    #[must_use]
    pub const fn end_of(row: usize) -> Self {
        Self {
            row,
            before: None,
            own: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Responses and errors
// ---------------------------------------------------------------------------

/// The answer to one request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    /// The request's id (0 when the request's id could not be read).
    pub id: u64,
    /// Whether it worked.
    pub ok: bool,
    /// The result, when it worked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// Why not, when it did not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<WallError>,
}

impl Response {
    /// A successful response carrying `result`.
    ///
    /// # Errors
    ///
    /// Fails only if `result` cannot be represented as JSON, which the
    /// protocol's own types never do.
    pub fn success(id: u64, result: &impl Serialize) -> Result<Self, WallError> {
        let value = serde_json::to_value(result).map_err(|err| {
            WallError::new(
                ErrorCode::Internal,
                format!("the result could not be encoded: {err}"),
            )
        })?;
        Ok(Self {
            id,
            ok: true,
            result: Some(value),
            error: None,
        })
    }

    /// A failed response.
    #[must_use]
    pub const fn failure(id: u64, error: WallError) -> Self {
        Self {
            id,
            ok: false,
            result: None,
            error: Some(error),
        }
    }
}

/// Why a request failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Not valid JSON, not a known op, a missing field, a line too long.
    BadRequest,
    /// The connection has not said hello yet.
    NotHello,
    /// A seat or display name outside `[A-Za-z0-9 _.-]{1,24}`.
    BadName,
    /// No module with that id.
    UnknownModule,
    /// The module has no knob with that name.
    UnknownKnob,
    /// The module has no port with that name, or the port is malformed.
    UnknownPort,
    /// No module kind with that name.
    UnknownKind,
    /// No cable with that number, or nothing plugged into that input.
    UnknownCable,
    /// No change with that number in the log.
    UnknownChange,
    /// A cap was reached: modules, cables or connections.
    Full,
    /// The seat has made too many changes too quickly.
    SlowDown,
    /// Not allowed: a seat asking for shutdown, a watcher asking for a
    /// change, a second hello, an undo that no longer applies.
    NotAllowed,
    /// Something went wrong inside the daemon.
    Internal,
}

impl ErrorCode {
    /// Every code.
    pub const ALL: [Self; 13] = [
        Self::BadRequest,
        Self::NotHello,
        Self::BadName,
        Self::UnknownModule,
        Self::UnknownKnob,
        Self::UnknownPort,
        Self::UnknownKind,
        Self::UnknownCable,
        Self::UnknownChange,
        Self::Full,
        Self::SlowDown,
        Self::NotAllowed,
        Self::Internal,
    ];
}

/// A failed request's code and explanation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WallError {
    /// What kind of failure.
    pub code: ErrorCode,
    /// A sentence for people.
    pub message: String,
}

impl WallError {
    /// An error with `code` and `message`.
    #[must_use]
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for WallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for WallError {}

// ---------------------------------------------------------------------------
// Changes
// ---------------------------------------------------------------------------

/// One change to the wall, as logged and pushed to other seats.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Change {
    /// Sequence number: every change gets the next one, forever.
    pub seq: u64,
    /// When, in UTC (`2026-09-26T12:00:01Z`).
    pub at: String,
    /// Who.
    pub seat: String,
    /// What exactly, with enough detail to undo it.
    pub what: What,
    /// One plain sentence.
    pub summary: String,
    /// The change this one undid, if it was an undo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undoes: Option<u64>,
}

/// What a change did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum What {
    /// A knob turned from one target to another.
    Turn {
        /// Module id.
        module: String,
        /// Knob name.
        knob: String,
        /// The target before.
        from: f64,
        /// The target now.
        to: f64,
        /// The glide, in beats.
        glide_beats: f64,
    },
    /// A cable plugged in.
    Patch {
        /// The new cable.
        cable: CableRecord,
        /// The cable it replaced in the same input.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        replaced: Option<CableRecord>,
    },
    /// A cable unplugged.
    Unpatch {
        /// The cable.
        cable: CableRecord,
        /// A cable plugged back into the same input (when undoing a patch
        /// that replaced it).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        replugged: Option<CableRecord>,
    },
    /// A module added.
    Add {
        /// The module as added.
        module: ModuleRecord,
    },
    /// A module taken away, with its cables.
    Remove {
        /// The module as it was.
        module: ModuleRecord,
        /// Its cables, as they were.
        cables: Vec<CableRecord>,
        /// Cables plugged back where its cables had replaced them (when
        /// undoing a restore).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        replugged: Vec<CableRecord>,
        /// Where it was on the rack, so undoing brings it back there.
        /// (Absent from changes made before the rack had rows.)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        place: Option<Place>,
    },
    /// A removed module brought back (undoing a removal).
    Restore {
        /// The module.
        module: ModuleRecord,
        /// Its cables that came back.
        cables: Vec<CableRecord>,
        /// Cables its cables replaced.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        replaced: Vec<CableRecord>,
        /// Its cables that could not come back: the other end is gone, or
        /// the wall is full.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        skipped: Vec<CableRecord>,
        /// Where it went on the rack.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        place: Option<Place>,
    },
    /// A `speak` module was given words (never shown: only how many).
    Speak {
        /// The module.
        module: String,
        /// How many words.
        words: u32,
        /// How long they take to say, in seconds.
        seconds: f64,
    },
    /// The daemon brought the saved patch up to date as it loaded it, or
    /// left out parts that could not load. Made by the seat `kazoo-wall`;
    /// it cannot be undone.
    Migrate {
        /// The patch file's version before.
        from_version: u32,
        /// The version after.
        to_version: u32,
        /// What changed, one sentence each.
        notes: Vec<String>,
    },
    /// The tempo set, or asked of the desk.
    Tempo {
        /// Beats per minute before.
        from: f64,
        /// Beats per minute asked for.
        to: f64,
        /// Asked of the desk rather than set on the wall's own clock.
        desk: bool,
    },
    /// A recording started or stopped. Started by a seat, or by the seat
    /// `kazoo-wall` when it carries a recording on into a new file (the
    /// audio restarted, or the file was full); stopped by a seat, or by
    /// `kazoo-wall` when it had to. It cannot be undone.
    Record {
        /// Started (true) or stopped (false).
        on: bool,
        /// The file.
        path: String,
        /// On a stop: how long the file is, in seconds.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seconds: Option<f64>,
        /// On a stop: samples lost because the writer fell behind.
        #[serde(default)]
        dropped: u64,
        /// On a start that carries a recording on: the file before.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        continues: Option<String>,
        /// Why the wall itself started or stopped it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// A change this build does not know, made by a newer wall: it reads,
    /// with its summary, but cannot be undone here.
    #[serde(other)]
    Unknown,
}

/// A cable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CableRecord {
    /// Its number: never reused on this wall.
    pub id: u32,
    /// Output, e.g. `lfo1.out`.
    pub from: String,
    /// Input, e.g. `vcf1.cutoff`.
    pub to: String,
    /// Attenuverter, -1 to 1.
    pub amount: f64,
}

/// A module: its identity and knob targets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModuleRecord {
    /// Id: kind and number, e.g. `vco2`; never reused on this wall.
    pub id: String,
    /// Kind, e.g. `vco` or `plate`.
    pub kind: String,
    /// Display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Knob targets by name.
    pub knobs: BTreeMap<String, f64>,
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// Something pushed to subscribed connections.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// Another seat changed the wall. A seat never hears its own changes
    /// (it has them in its responses).
    Change {
        /// The change.
        change: Box<Change>,
    },
    /// A seat came or went. Watchers never are.
    Seat {
        /// Its name.
        seat: String,
        /// Came (true) or went (false).
        joined: bool,
        /// The latest change's sequence number when it happened, placing it
        /// in the change stream.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seq: Option<u64>,
    },
    /// A module produced a non-finite sample and was reset.
    Fault {
        /// One plain sentence.
        summary: String,
        /// The latest change's sequence number when it happened.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seq: Option<u64>,
    },
    /// Fingerprints moved: sent to every subscriber, the actor included,
    /// right after the change that moved them. Only what changed is here;
    /// an entry with empty shares is gone (removed, or no dye reaches it).
    Fingerprints {
        /// The change that moved them.
        seq: u64,
        /// Changed shares by module id.
        #[serde(default)]
        modules: BTreeMap<String, Shares>,
        /// Changed shares by cable number.
        #[serde(default)]
        cables: BTreeMap<String, Shares>,
    },
    /// The rack's rows were rearranged (a module moved): sent to every
    /// subscriber, the mover included. Moves are not changes: they are not
    /// logged and do not move the revision.
    Rack {
        /// The rows, top to bottom, each its module ids left to right.
        rows: Vec<Vec<String>>,
    },
    /// An event this build does not know, from a newer wall: nothing to do.
    #[serde(other)]
    Unknown,
}

/// A line from the daemon: a response or an event.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerLine {
    /// The answer to a request.
    Response(Response),
    /// A pushed event.
    Event(Event),
}

impl ServerLine {
    /// Read a line from the daemon.
    ///
    /// # Errors
    ///
    /// Fails if the line is not a response or event.
    pub fn parse(line: &str) -> Result<Self, serde_json::Error> {
        let value: serde_json::Value = serde_json::from_str(line)?;
        if value.get("event").is_some() {
            serde_json::from_value(value).map(Self::Event)
        } else {
            serde_json::from_value(value).map(Self::Response)
        }
    }
}

// ---------------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------------

/// The result of `hello`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloResult {
    /// The daemon and its version.
    pub daemon: String,
    /// The seat, as accepted.
    pub seat: String,
    /// Whether this connection is a console.
    pub console: bool,
    /// Seats online now, this one included (unless it is a watcher).
    pub seats: Vec<String>,
    /// The latest change's sequence number.
    pub revision: u64,
    /// Whether this connection is a watcher.
    #[serde(default)]
    pub watcher: bool,
}

/// Where the wall's beat comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockSource {
    /// Following the kazoo-mix desk while it plays.
    Desk,
    /// The wall's own clock.
    Own,
}

/// A snapshot from a wall that predates the monitor was always heard.
const fn heard_by_default() -> bool {
    true
}

/// The result of `look`: the whole wall.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// The latest change's sequence number.
    pub revision: u64,
    /// Beats per minute.
    pub tempo: f64,
    /// Song position, in beats.
    pub beat: f64,
    /// Where the beat comes from.
    pub clock: ClockSource,
    /// Whether the wall's sound is going to the desk (rather than the
    /// audio device).
    pub on_desk: bool,
    /// Whether anything leaves the wall: when false it plays on, silent,
    /// until a console turns the monitor on.
    #[serde(default = "heard_by_default")]
    pub heard: bool,
    /// Seats online.
    pub seats: Vec<String>,
    /// Every module, in the order they were added.
    pub modules: Vec<ModuleView>,
    /// Every cable.
    pub cables: Vec<CableRecord>,
    /// Master meters.
    pub levels: Levels,
    /// The latest listening, if any yet.
    pub listen: Option<Listen>,
    /// Modules that produced non-finite samples.
    pub faults: Faults,
    /// Who touched what, and where the signal carried it: each module's and
    /// cable's share per seat (see [`crate::fingerprints`]).
    #[serde(default)]
    pub fingerprints: Fingerprints,
    /// Latency and the delays that keep every path in step.
    #[serde(default)]
    pub timing: Timings,
    /// The recording under way, if there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording: Option<Recording>,
    /// The rack's rows, top to bottom, each its module ids left to right:
    /// every module is in exactly one. `None` from a wall older than the
    /// rack's rows (a console then lays the rack out itself).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rack: Option<Vec<Vec<String>>>,
}

/// A recording under way.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recording {
    /// The file: a 32-bit float stereo WAV.
    pub path: String,
    /// Who started it (`kazoo-wall` when it carries one on).
    pub seat: String,
    /// How much is written so far, in seconds.
    pub seconds: f64,
    /// Samples lost so far because the writer fell behind.
    pub dropped: u64,
    /// The file's frames per second: the engine's.
    pub sample_rate: u32,
}

/// How late each module's signal is, and the delays that keep every path
/// in step (see [`crate::daemon::timing`]). Only what is not zero is
/// listed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Timings {
    /// The engine's frames per second, which the frame counts are in.
    #[serde(default)]
    pub sample_rate: u32,
    /// By module id.
    #[serde(default)]
    pub modules: BTreeMap<String, ModuleTiming>,
    /// Each delayed cable's delay in frames, by cable number (as a
    /// string, as JSON keys are).
    #[serde(default)]
    pub cables: BTreeMap<String, u32>,
    /// Cables that needed more delay than a cable holds (about 21 ms at
    /// 48 kHz): their paths are not fully in step.
    #[serde(default)]
    pub uncompensated: Vec<u32>,
    /// Modules whose latency follows a knob with a cable in it: the cable
    /// moves the latency as it plays, so their paths are kept in step only
    /// for the knob as set.
    #[serde(default)]
    pub unsteady: Vec<String>,
}

/// One module's timing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ModuleTiming {
    /// Frames the module holds its sound back (an effect's oversampling).
    pub latency_frames: u32,
    /// The same in milliseconds.
    pub latency_ms: f64,
    /// Frames its inputs arrive after the wall's sources.
    pub arrival_frames: u32,
}

/// One module in a snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModuleView {
    /// Id.
    pub id: String,
    /// Kind, e.g. `vco` or `plate`.
    pub kind: String,
    /// Display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Knobs, in catalogue order. Every knob is also a jack a cable can
    /// plug into, by the knob's name.
    pub knobs: Vec<KnobView>,
    /// Input names (other than the knob jacks), in catalogue order.
    pub inputs: Vec<String>,
    /// Output names, in catalogue order.
    pub outputs: Vec<String>,
}

/// One knob in a snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnobView {
    /// Name.
    pub name: String,
    /// Where it is now (mid-glide, this is on the way).
    pub value: f64,
    /// Where it is going.
    pub target: f64,
    /// Lowest value.
    pub min: f64,
    /// Highest value.
    pub max: f64,
    /// Unit name (`Hz`, `s`, `st`, ...; empty for plain numbers).
    pub unit: String,
    /// Whole numbers only.
    pub stepped: bool,
    /// The value as people read it, e.g. `420 Hz`.
    pub display: String,
    /// The target as people read it.
    pub target_display: String,
}

/// Master meters.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Levels {
    /// Left peak, in dBFS (-120 is silence).
    pub peak_l: f64,
    /// Right peak, in dBFS.
    pub peak_r: f64,
}

/// Modules that produced non-finite samples.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Faults {
    /// Every one since the daemon started.
    pub count: u64,
    /// The latest few, newest last.
    pub recent: Vec<String>,
}

/// What the wall sounds like.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Listen {
    /// When it was heard, in UTC.
    pub at: String,
    /// RMS level, in dBFS.
    pub rms_db: f64,
    /// Peak level, in dBFS.
    pub peak_db: f64,
    /// Spectral centroid, in Hz.
    pub centroid_hz: f64,
    /// Share of the energy below 250 Hz, 0 to 1.
    pub low: f64,
    /// Share from 250 Hz to 4 kHz.
    pub mid: f64,
    /// Share above 4 kHz.
    pub high: f64,
    /// Onsets per second.
    pub onsets_per_second: f64,
    /// The dominant pitch, in Hz, if there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pitch_hz: Option<f64>,
    /// The dominant pitch as a note, e.g. `A2`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pitch: Option<String>,
    /// All of it in plain words, e.g. `dark, sparse, slow pulse around A2,
    /// quiet`.
    pub words: String,
}

/// The result of `listen`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListenResult {
    /// The latest listening; none until the first quarter second.
    pub listen: Option<Listen>,
}

/// The result of `catalogue`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogueResult {
    /// Every kind.
    pub kinds: Vec<KindInfo>,
}

/// One module kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KindInfo {
    /// Kind: the name module ids start with, e.g. `vco` or `plate`.
    pub kind: String,
    /// `synth` for the wall's own modules, `fx` for effects.
    pub family: String,
    /// What it is.
    pub about: String,
    /// Knobs, in order. Every knob is also a jack by its name.
    pub knobs: Vec<KnobInfo>,
    /// Inputs other than the knob jacks, in order.
    pub inputs: Vec<PortInfo>,
    /// Outputs, in order.
    pub outputs: Vec<PortInfo>,
}

impl KindInfo {
    /// Its group, an index into [`crate::catalogue::GROUPS`] (see
    /// [`crate::catalogue::group`]).
    #[must_use]
    pub fn group(&self) -> usize {
        let audio = |ports: &[PortInfo]| ports.iter().any(|port| port.signal == Signal::Audio);
        crate::catalogue::group(
            &self.family,
            &self.kind,
            audio(&self.inputs),
            audio(&self.outputs),
            !self.outputs.is_empty(),
        )
    }
}

/// One knob of a kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnobInfo {
    /// Name.
    pub name: String,
    /// Lowest value.
    pub min: f64,
    /// Highest value.
    pub max: f64,
    /// Value on a new module.
    pub default: f64,
    /// Unit name.
    pub unit: String,
    /// Linear or log travel.
    pub curve: Curve,
    /// Whole numbers only.
    pub stepped: bool,
    /// For stepped knobs with named positions (divisions, scales, notes),
    /// the name of each position from `min` up.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// How a cable into the knob's jack moves it: `range` adds
    /// `cv × (max − min) / 2`; `{"octaves": n}` multiplies by `2^(cv × n)`.
    pub jack: JackLaw,
}

/// One port of a kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortInfo {
    /// Name.
    pub name: String,
    /// What it carries: `audio`, `gate` or `cv`.
    pub signal: Signal,
    /// What it carries.
    pub about: String,
}

/// The result of `turn`, `patch`, `unpatch`, `add`, `remove` and `undo`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangeResult {
    /// The change, as logged.
    pub change: Change,
    /// The module added or restored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    /// The cable plugged in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cable: Option<u32>,
}

/// The result of `arrange`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArrangeResult {
    /// The rack's rows now.
    pub rows: Vec<Vec<String>>,
}

/// The result of `tempo`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TempoResult {
    /// The tempo asked for, held to 20-300 BPM.
    pub bpm: f64,
    /// Whether it was asked of the desk (which answers by changing tempo
    /// for everyone) rather than set on the wall's own clock.
    pub desk: bool,
    /// The change, as logged.
    pub change: Change,
}

/// The result of `log`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogPage {
    /// Changes, oldest first.
    pub changes: Vec<Change>,
    /// Whether there are older changes than these.
    pub more: bool,
}

/// The result of `subscribe`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscribeResult {
    /// Seats online now.
    pub seats: Vec<String>,
}

/// The result of `monitor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorResult {
    /// Whether the wall is heard now.
    pub on: bool,
}

/// The result of `record`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordResult {
    /// Whether the wall is recording now.
    pub on: bool,
    /// The file being recorded, or the one just finished; none when there
    /// was no recording to stop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// How long that file is so far, or in all, in seconds.
    pub seconds: f64,
    /// Samples it lost because the writer fell behind.
    pub dropped: u64,
    /// The change, as logged; none when nothing changed (asked to start
    /// while recording, or to stop while not).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<Change>,
}

/// The result of `shutdown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShutdownResult {
    /// Always true: the daemon saves the patch and exits after answering.
    pub stopping: bool,
}

#[cfg(test)]
mod tests;
