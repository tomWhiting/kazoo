//! The console's state and what every key does.
//!
//! [`App`] holds what the console knows of the wall (the latest snapshot,
//! the catalogue, the log) and what Tom is doing (the selection, a patch in
//! progress, a prompt). Keys never touch the socket: they queue [`Job`]s
//! for the worker, and the worker's [`Reply`]s come back through
//! [`App::on_reply`]. That keeps drawing smooth whatever the socket does,
//! and lets tests check the exact requests each key produces.
//!
//! Knob turns are gathered before they are sent: holding `=` moves the
//! knob on screen at once, and the wall hears one turn when the key rests
//! (or every [`TURN_LONGEST`] while it is held), so a long sweep is a few
//! changes in the log rather than dozens, and stays inside the wall's
//! flood guard.
//!
//! What each key does is in [`keys`]; what the mouse does, in the rack
//! view, is in [`mouse`]. Every change the console sends counts against
//! its copy of the wall's flood guard ([`flood`]); turns wait for it.

mod flood;
mod keys;
mod mouse;
mod moves;
mod rack;

#[cfg(test)]
pub use mouse::dragged_value;
pub use moves::Shove;
pub use rack::{Drag, PathKey, RackState};
#[cfg(test)]
pub use rack::{EDGE_COLUMNS, EDGE_STEP};

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;

use kazoo_wall::catalogue::Signal;
use kazoo_wall::fingerprints::Shares;
use kazoo_wall::format;
use kazoo_wall::protocol::{
    ArrangeResult, CableRecord, CatalogueResult, ChangeResult, ErrorCode, Event, KindInfo,
    KnobInfo, LogPage, ModuleView, MonitorResult, Place, RecordResult, Request, Snapshot,
    TempoResult,
};

use super::knob::Travel;
use super::link::LinkState;
use super::log::{Log, Tone as LogTone};
use super::worker::{Failure, Job, Reply};

/// The glides `[` and `]` step through, in beats.
pub const GLIDES: [f64; 11] = [0.0, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 48.0, 64.0];

/// The glide a console starts with: the wall's own default, 2 beats.
const DEFAULT_GLIDE: usize = 4;

/// A turn is sent once its key has rested this long...
pub const TURN_SETTLE: Duration = Duration::from_millis(150);

/// ...or has been held this long.
pub const TURN_LONGEST: Duration = Duration::from_millis(400);

/// While a knob is dragged, a turn goes to the wall at most this often
/// (ten a second), and the last value when the mouse lets go.
pub const DRAG_INTERVAL: Duration = Duration::from_millis(100);

/// The glide of a dragged or reset knob, in beats: short, so the knob
/// follows the mouse, but never a jump that clicks.
pub const DRAG_GLIDE: f64 = 0.125;

/// How long a sent turn is shown ahead of the wall catching up.
const AWAIT_LONGEST: Duration = Duration::from_secs(2);

/// How long a result stays in the status line.
pub const STATUS_LIFE: Duration = Duration::from_secs(12);

/// Changes fetched on connecting.
const LOG_PAGE: u32 = 100;

/// Changes fetched when the wall's revision runs ahead of the log.
const CATCH_UP: u32 = 50;

/// One press of the amount keys while patching.
const AMOUNT_STEP: f64 = 0.05;

/// One shifted press of the amount keys while patching.
const AMOUNT_FINE: f64 = 0.01;

/// Longest tempo or value typed at a prompt.
const MAX_ENTRY: usize = 24;

/// Kinds a page key moves the picker by.
const PICKER_PAGE: usize = 10;

/// The picker's groups, in order.
pub use kazoo_wall::catalogue::GROUPS;

/// How the wall is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// Panels of knob rows, with the cable list: every value in words.
    List,
    /// A rack of faceplates with dials, sockets and hanging cables, for the
    /// mouse.
    Rack,
}

/// Where the arrow keys go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// The module panels.
    Wall,
    /// The cable list.
    Cables,
    /// The log.
    Log,
}

/// How the console ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// Tom left; the wall keeps playing.
    Left,
    /// Tom stopped the wall.
    Stopped,
}

/// The status line's mood.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// News.
    Info,
    /// Something worked.
    Done,
    /// Something did not.
    Trouble,
}

/// The latest result, for the status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// The sentence.
    pub text: String,
    /// Its mood.
    pub tone: Tone,
    /// When it was said.
    pub at: Instant,
}

/// A step in patching a cable.
#[derive(Debug, Clone, PartialEq)]
pub enum Patching {
    /// Choosing the output the cable comes from.
    From {
        /// The module whose outputs are offered.
        module: String,
        /// The output, by index.
        port: usize,
    },
    /// Choosing the input (or knob jack) it goes to.
    To {
        /// The output chosen, e.g. `lfo1.out`.
        from: String,
        /// The module whose jacks are offered.
        module: String,
        /// The jack, by index: inputs first, then knobs.
        jack: usize,
    },
    /// Setting the cable's amount.
    Amount {
        /// The output, e.g. `lfo1.out`.
        from: String,
        /// The input, e.g. `vcf1.cutoff`.
        to: String,
        /// The attenuverter, -1 to 1.
        amount: f64,
        /// The cable already in that input, which this one replaces.
        replaces: Option<u32>,
    },
}

/// What the keys are doing.
#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    /// Moving, turning and the single-key actions.
    Normal,
    /// The key list, scrolled this many lines.
    Help {
        /// Lines scrolled.
        scroll: usize,
    },
    /// Picking a kind to add (the index counts the kinds that match, in
    /// picker order).
    Add {
        /// The kind under the cursor.
        index: usize,
        /// What has been typed to find a kind.
        filter: String,
        /// Where on the rack it goes; `None` where the wall puts it.
        place: Option<Place>,
    },
    /// Naming a module before adding it.
    Name {
        /// The kind.
        kind: String,
        /// The name so far.
        text: String,
        /// Where on the rack it goes.
        place: Option<Place>,
    },
    /// Asking before a module goes.
    Remove {
        /// The module.
        module: String,
        /// Its cables, which go with it.
        cables: usize,
    },
    /// Patching a cable.
    Patch(Patching),
    /// Typing a tempo.
    Tempo {
        /// The digits so far.
        text: String,
    },
    /// Typing a knob's value.
    Value {
        /// The module.
        module: String,
        /// The knob.
        knob: String,
        /// The text so far.
        text: String,
    },
    /// Asking before the wall stops.
    Stop,
    /// Typing a module's id, name or kind to jump to it.
    Jump {
        /// The text so far.
        text: String,
    },
}

/// A jack on the wall, for drawing the patch cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JackRef {
    /// The module.
    pub module: String,
    /// The jack's name.
    pub name: String,
    /// An output (rather than an input or knob jack).
    pub output: bool,
}

/// A turn gathered from held keys, not yet sent.
#[derive(Debug, Clone, PartialEq)]
struct PendingTurn {
    module: String,
    knob: String,
    value: f64,
    first: Instant,
    last: Instant,
    kind: TurnKind,
    /// The glide it goes with; `None` for the console's chosen glide.
    glide: Option<f64>,
}

/// What a waiting turn is waiting for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TurnKind {
    /// A turn key to rest (or to have been held long enough).
    Keys,
    /// The drag interval, while the mouse holds the knob.
    Drag,
    /// Only the flood guard: a typed value, a reset, a released drag.
    Now,
}

/// A turn sent, shown until the wall's snapshot catches up.
#[derive(Debug, Clone, PartialEq)]
struct SentTurn {
    module: String,
    knob: String,
    value: f64,
    at: Instant,
}

/// Everything the console knows and is doing.
#[derive(Debug)]
pub struct App {
    seat: String,
    socket: String,
    link: LinkState,
    feed: LinkState,
    snapshot: Option<Snapshot>,
    catalogue: Vec<KindInfo>,
    log: Log,
    focus: Focus,
    mode: Mode,
    selected: Option<String>,
    selected_hint: usize,
    knob: usize,
    cable_cursor: usize,
    log_cursor: usize,
    glide: usize,
    pending: Vec<PendingTurn>,
    sent: Vec<SentTurn>,
    select_after_add: Option<String>,
    log_inflight: bool,
    caught_up_to: u64,
    launching: bool,
    status: Option<Status>,
    jobs: Vec<Job>,
    exit: Option<Exit>,
    view: View,
    log_hidden: bool,
    flood: flood::Flood,
    drag_sends: Vec<(String, String, Instant)>,
    /// The rack view's layout, pan and drag, as last drawn.
    pub rack: RackState,
    /// Panels across, as last drawn: up and down move by this many.
    pub columns: usize,
    /// Lines the wall is scrolled, as last drawn.
    pub scroll: u32,
}

impl App {
    /// A console for `seat`, talking to the daemon on `socket` (shown while
    /// it is not answering).
    #[must_use]
    pub fn new(seat: &str, socket: String) -> Self {
        Self {
            seat: seat.to_string(),
            socket,
            link: LinkState::Connecting,
            feed: LinkState::Connecting,
            snapshot: None,
            catalogue: Vec::new(),
            log: Log::default(),
            focus: Focus::Wall,
            mode: Mode::Normal,
            selected: None,
            selected_hint: 0,
            knob: 0,
            cable_cursor: 0,
            log_cursor: 0,
            glide: DEFAULT_GLIDE,
            pending: Vec::new(),
            sent: Vec::new(),
            select_after_add: None,
            log_inflight: false,
            caught_up_to: 0,
            launching: false,
            status: None,
            jobs: Vec::new(),
            exit: None,
            view: View::List,
            log_hidden: false,
            flood: flood::Flood::new(),
            drag_sends: Vec::new(),
            rack: RackState::default(),
            columns: 1,
            scroll: 0,
        }
    }

    // -----------------------------------------------------------------------
    // What the view reads
    // -----------------------------------------------------------------------

    /// The daemon's socket, as shown.
    #[must_use]
    pub fn socket(&self) -> &str {
        &self.socket
    }

    /// The request link.
    #[must_use]
    pub const fn link(&self) -> &LinkState {
        &self.link
    }

    /// The event feed's link.
    #[must_use]
    pub const fn feed(&self) -> &LinkState {
        &self.feed
    }

    /// The wall, as last seen.
    #[must_use]
    pub const fn snapshot(&self) -> Option<&Snapshot> {
        self.snapshot.as_ref()
    }

    /// The log.
    #[must_use]
    pub const fn log(&self) -> &Log {
        &self.log
    }

    /// Where the arrow keys go.
    #[must_use]
    pub const fn focus(&self) -> Focus {
        self.focus
    }

    /// What the keys are doing.
    #[must_use]
    pub const fn mode(&self) -> &Mode {
        &self.mode
    }

    /// The selected module's index in the snapshot.
    #[must_use]
    pub fn selected_index(&self) -> Option<usize> {
        let id = self.selected.as_deref()?;
        self.modules().iter().position(|module| module.id == id)
    }

    /// The selected knob's index in the selected module.
    #[must_use]
    pub const fn knob_index(&self) -> usize {
        self.knob
    }

    /// The cable list's cursor.
    #[must_use]
    pub const fn cable_cursor(&self) -> usize {
        self.cable_cursor
    }

    /// The log's cursor.
    #[must_use]
    pub const fn log_cursor(&self) -> usize {
        self.log_cursor
    }

    /// How the wall is shown.
    #[must_use]
    pub const fn view(&self) -> View {
        self.view
    }

    /// Whether the log is on screen: always in the list view, and in the
    /// rack view unless it has been put away.
    #[must_use]
    pub const fn log_shown(&self) -> bool {
        !(self.log_hidden && matches!(self.view, View::Rack))
    }

    /// The module a jump to `text` goes to: one whose id is `text`, else
    /// whose name is, else of that kind, else whose id, name or kind
    /// starts with it, else whose id or name holds it (ignoring case). Of
    /// several equally good, the first after the selection, so jumping
    /// again goes on to the next.
    #[must_use]
    pub fn jump_target(&self, text: &str) -> Option<usize> {
        let wanted = text.trim().to_lowercase();
        if wanted.is_empty() {
            return None;
        }
        let modules = self.modules();
        let tiers: [&dyn Fn(&ModuleView) -> bool; 5] = [
            &|module| module.id.to_lowercase() == wanted,
            &|module| {
                module
                    .name
                    .as_ref()
                    .is_some_and(|name| name.to_lowercase() == wanted)
            },
            &|module| module.kind.to_lowercase() == wanted,
            &|module| {
                module.id.to_lowercase().starts_with(&wanted)
                    || module.kind.to_lowercase().starts_with(&wanted)
                    || module
                        .name
                        .as_ref()
                        .is_some_and(|name| name.to_lowercase().starts_with(&wanted))
            },
            &|module| {
                module.id.to_lowercase().contains(&wanted)
                    || module
                        .name
                        .as_ref()
                        .is_some_and(|name| name.to_lowercase().contains(&wanted))
            },
        ];
        let after = self.selected_index().map_or(0, |index| index + 1);
        tiers.iter().find_map(|tier| {
            (0..modules.len())
                .map(|offset| (after + offset) % modules.len())
                .find(|&index| tier(&modules[index]))
        })
    }

    /// The glide turns take, in beats.
    #[must_use]
    pub const fn glide_beats(&self) -> f64 {
        GLIDES[self.glide]
    }

    /// Whether a start of the daemon is under way.
    #[must_use]
    pub const fn launching(&self) -> bool {
        self.launching
    }

    /// The status line, while it is fresh.
    #[must_use]
    pub fn status(&self, now: Instant) -> Option<&Status> {
        self.status
            .as_ref()
            .filter(|status| now.saturating_duration_since(status.at) < STATUS_LIFE)
    }

    /// How the console ended, once it has.
    #[must_use]
    pub const fn exit(&self) -> Option<Exit> {
        self.exit
    }

    /// The jobs keys and replies have queued, for the worker.
    pub fn take_jobs(&mut self) -> Vec<Job> {
        let jobs = std::mem::take(&mut self.jobs);
        // Turns took their place in the flood guard when they were let go;
        // every other change counts as it leaves.
        let now = Instant::now();
        for job in &jobs {
            if let Job::Call(request) = job {
                // Moves, like turns, took their place when they were let go.
                let held = matches!(request, Request::Turn { .. } | Request::Arrange { .. });
                if request.is_change() && !held {
                    self.flood.spend(now);
                }
                // Anything else done since a move means z undoes that, not
                // the move.
                if request.is_change() && !matches!(request, Request::Arrange { .. }) {
                    self.rack.last_move = None;
                }
            }
        }
        jobs
    }

    /// Lay the rack view out for a view `view` (width, height), made again
    /// only when the wall's shape, the density or the view has changed;
    /// false when there is nothing to lay out.
    pub fn lay_out_rack(&mut self, view: (u16, u16)) -> bool {
        match self.snapshot.as_ref() {
            Some(snapshot) if !snapshot.modules.is_empty() => {
                self.rack
                    .lay_out(&snapshot.modules, snapshot.rack.as_deref(), view);
                true
            }
            _ => {
                self.rack.clear();
                false
            }
        }
    }

    fn modules(&self) -> &[ModuleView] {
        self.snapshot
            .as_ref()
            .map_or(&[][..], |snapshot| snapshot.modules.as_slice())
    }

    fn module(&self, id: &str) -> Option<&ModuleView> {
        self.modules().iter().find(|module| module.id == id)
    }

    /// The catalogue's entry for `kind`.
    #[must_use]
    pub fn kind_info(&self, kind: &str) -> Option<&KindInfo> {
        self.catalogue.iter().find(|info| info.kind == kind)
    }

    /// The catalogue's entry for `kind`'s knob `knob`.
    #[must_use]
    pub fn knob_info(&self, kind: &str, knob: &str) -> Option<&KnobInfo> {
        self.kind_info(kind)?
            .knobs
            .iter()
            .find(|info| info.name == knob)
    }

    /// What jack `name` of `module` carries: knob jacks are CV; ports say
    /// in the catalogue (CV when the catalogue has not said).
    #[must_use]
    pub fn jack_signal(&self, module: &ModuleView, name: &str, output: bool) -> Signal {
        let Some(info) = self.kind_info(&module.kind) else {
            return Signal::Cv;
        };
        let ports = if output { &info.outputs } else { &info.inputs };
        ports
            .iter()
            .find(|port| port.name == name)
            .map_or(Signal::Cv, |port| port.signal)
    }

    /// The cables, by number.
    #[must_use]
    pub fn cables(&self) -> Vec<&CableRecord> {
        let mut cables: Vec<&CableRecord> = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.cables.iter().collect())
            .unwrap_or_default();
        cables.sort_by_key(|cable| cable.id);
        cables
    }

    /// Where `module`'s `knob` is heading as far as the console knows: a
    /// turn gathered or sent ahead of the wall showing it, if any.
    #[must_use]
    pub fn ahead(&self, module: &str, knob: &str) -> Option<f64> {
        self.pending
            .iter()
            .find(|turn| turn.module == module && turn.knob == knob)
            .map(|turn| turn.value)
            .or_else(|| {
                self.sent
                    .iter()
                    .rev()
                    .find(|turn| turn.module == module && turn.knob == knob)
                    .map(|turn| turn.value)
            })
    }

    /// The patch cursor's jack and the cable's chosen output, while
    /// patching.
    #[must_use]
    pub fn patch_marks(&self) -> (Option<JackRef>, Option<JackRef>) {
        let Mode::Patch(step) = &self.mode else {
            return (None, None);
        };
        match step {
            Patching::From { module, port } => {
                let hot = self.module(module).and_then(|view| {
                    view.outputs.get(*port).map(|name| JackRef {
                        module: module.clone(),
                        name: name.clone(),
                        output: true,
                    })
                });
                (hot, None)
            }
            Patching::To { from, module, jack } => {
                let hot = self.module(module).and_then(|view| {
                    jack_names(view).get(*jack).map(|name| JackRef {
                        module: module.clone(),
                        name: (*name).to_string(),
                        output: false,
                    })
                });
                (hot, split_jack(from, true))
            }
            Patching::Amount { from, to, .. } => (split_jack(to, false), split_jack(from, true)),
        }
    }

    /// The kinds in picker order, each with its group's index in
    /// [`GROUPS`].
    #[cfg(test)]
    #[must_use]
    pub fn picker(&self) -> Vec<(usize, &KindInfo)> {
        self.picker_for("")
    }

    /// The kinds whose name or description holds `filter` (ignoring case),
    /// in picker order, each with its group's index in [`GROUPS`].
    #[must_use]
    pub fn picker_for(&self, filter: &str) -> Vec<(usize, &KindInfo)> {
        let wanted = filter.trim().to_lowercase();
        let mut kinds: Vec<(usize, &KindInfo)> = self
            .catalogue
            .iter()
            .filter(|info| {
                wanted.is_empty()
                    || info.kind.to_lowercase().contains(&wanted)
                    || info.about.to_lowercase().contains(&wanted)
            })
            .map(|info| (info.group(), info))
            .collect();
        kinds.sort_by_key(|(group, _)| *group);
        kinds
    }

    // -----------------------------------------------------------------------
    // Replies
    // -----------------------------------------------------------------------

    /// Take in something the connections reported.
    pub fn on_reply(&mut self, reply: Reply, now: Instant) {
        match reply {
            Reply::Link(state) => self.on_link(state, now),
            Reply::Feed(state) => self.on_feed(state),
            Reply::Snapshot(snapshot) => self.on_snapshot(*snapshot, now),
            Reply::Answer { request, outcome } => match outcome {
                Ok(value) => self.on_result(&request, value, now),
                Err(failure) => self.on_failure(&request, &failure, now),
            },
            Reply::Launched(result) => {
                self.launching = false;
                match result {
                    Ok(()) => {
                        self.say(Tone::Done, "the wall is playing".to_string(), now);
                        self.note("started the wall".to_string(), LogTone::Note);
                    }
                    Err(message) => self.say(Tone::Trouble, message, now),
                }
            }
            Reply::Event(event) => self.on_event(event, now),
        }
    }

    fn on_link(&mut self, state: LinkState, now: Instant) {
        match &state {
            LinkState::Connected { daemon } if !self.link.is_up() => {
                self.note(
                    format!("connected to {daemon} as {}", self.seat),
                    LogTone::Note,
                );
                self.say(
                    Tone::Done,
                    format!("connected to {daemon} as {}", self.seat),
                    now,
                );
                self.jobs.push(Job::Call(Request::Catalogue));
                self.jobs.push(Job::Call(Request::Log {
                    before: None,
                    limit: Some(LOG_PAGE),
                }));
                self.log_inflight = true;
            }
            LinkState::Down { reason, .. } if self.link.is_up() => {
                self.note(
                    format!("lost the wall ({reason}); reconnecting"),
                    LogTone::Note,
                );
                self.say(
                    Tone::Trouble,
                    format!("lost the wall ({reason}); reconnecting"),
                    now,
                );
                self.log_inflight = false;
            }
            LinkState::Connecting | LinkState::Connected { .. } | LinkState::Down { .. } => {}
        }
        self.link = state;
    }

    fn on_feed(&mut self, state: LinkState) {
        if let LinkState::Down { reason, .. } = &state {
            if self.feed.is_up() && self.link.is_up() {
                self.note(
                    format!("the live feed dropped ({reason}); reconnecting"),
                    LogTone::Note,
                );
            }
        }
        self.feed = state;
    }

    fn on_event(&mut self, event: Event, now: Instant) {
        match event {
            Event::Change { change } => {
                let mine = change.seat == self.seat;
                self.log.add_change(&change, mine);
            }
            Event::Seat { seat, joined, .. } => {
                let text = if joined {
                    format!("{seat} joined the wall")
                } else {
                    format!("{seat} left the wall")
                };
                self.note(text, LogTone::Seat);
            }
            Event::Fault { summary, .. } => {
                self.say(Tone::Trouble, summary.clone(), now);
                self.note(summary, LogTone::Fault);
            }
            // From a newer wall: nothing this build can show.
            Event::Unknown => {}
            Event::Fingerprints {
                modules, cables, ..
            } => {
                // The next look brings them too; this shows them at once.
                if let Some(snapshot) = self.snapshot.as_mut() {
                    merge_shares(&mut snapshot.fingerprints.modules, modules);
                    merge_shares(&mut snapshot.fingerprints.cables, cables);
                }
            }
            Event::Rack { rows } => {
                // The next look brings them too; this shows a move at once.
                if let Some(snapshot) = self.snapshot.as_mut() {
                    snapshot.rack = Some(rows);
                }
                self.rows_seen();
            }
        }
    }

    fn on_snapshot(&mut self, snapshot: Snapshot, now: Instant) {
        if let Some(id) = self.select_after_add.take() {
            if snapshot.modules.iter().any(|module| module.id == id) {
                self.selected = Some(id);
                self.knob = 0;
                self.focus = Focus::Wall;
                // The rack view brings the new faceplate into sight.
                self.rack.follow = true;
            } else {
                self.select_after_add = Some(id);
            }
        }
        let found = self
            .selected
            .as_deref()
            .and_then(|id| snapshot.modules.iter().position(|module| module.id == id));
        let index = match found {
            Some(index) => Some(index),
            None if snapshot.modules.is_empty() => None,
            None => Some(self.selected_hint.min(snapshot.modules.len() - 1)),
        };
        if found.is_none() {
            self.knob = 0;
        }
        self.selected = index.map(|index| snapshot.modules[index].id.clone());
        if let Some(index) = index {
            self.selected_hint = index;
            self.knob = self
                .knob
                .min(snapshot.modules[index].knobs.len().saturating_sub(1));
        }
        self.cable_cursor = self
            .cable_cursor
            .min(snapshot.cables.len().saturating_sub(1));
        self.sent.retain(|turn| {
            now.saturating_duration_since(turn.at) < AWAIT_LONGEST
                && !snapshot
                    .modules
                    .iter()
                    .find(|module| module.id == turn.module)
                    .and_then(|module| module.knobs.iter().find(|knob| knob.name == turn.knob))
                    .is_some_and(|knob| Travel::of(knob, None).same(knob.target, turn.value))
        });
        if !self.log_inflight
            && snapshot.revision > self.log.latest_seq().unwrap_or(0)
            && snapshot.revision > self.caught_up_to
        {
            self.caught_up_to = snapshot.revision;
            self.log_inflight = true;
            self.jobs.push(Job::Call(Request::Log {
                before: None,
                limit: Some(CATCH_UP),
            }));
        }
        self.snapshot = Some(snapshot);
        self.rows_seen();
        self.check_patch(now);
    }

    /// End a patch whose modules have gone from the wall.
    fn check_patch(&mut self, now: Instant) {
        let Mode::Patch(step) = &self.mode else {
            return;
        };
        let gone = match step {
            Patching::From { module, .. } => self.module(module).is_none().then(|| module.clone()),
            Patching::To { from, module, .. } => [jack_module(from), module.as_str()]
                .into_iter()
                .find(|id| self.module(id).is_none())
                .map(str::to_string),
            Patching::Amount { from, to, .. } => [jack_module(from), jack_module(to)]
                .into_iter()
                .find(|id| self.module(id).is_none())
                .map(str::to_string),
        };
        if let Some(module) = gone {
            self.mode = Mode::Normal;
            self.say(
                Tone::Trouble,
                format!("{module} left the wall; patching stopped"),
                now,
            );
        }
    }

    fn on_result(&mut self, request: &Request, value: serde_json::Value, now: Instant) {
        match request {
            Request::Catalogue => {
                if let Some(catalogue) = self.parse::<CatalogueResult>(request, value, now) {
                    self.catalogue = catalogue.kinds;
                }
            }
            Request::Log { .. } => {
                self.log_inflight = false;
                if let Some(page) = self.parse::<LogPage>(request, value, now) {
                    for change in &page.changes {
                        let mine = change.seat == self.seat;
                        self.log.add_change(change, mine);
                    }
                }
            }
            Request::Turn { .. }
            | Request::Patch { .. }
            | Request::Unpatch { .. }
            | Request::Add { .. }
            | Request::Remove { .. }
            | Request::Undo { .. } => {
                if let Some(result) = self.parse::<ChangeResult>(request, value, now) {
                    self.log.add_change(&result.change, true);
                    self.say(Tone::Done, result.change.summary.clone(), now);
                    if matches!(request, Request::Add { .. }) {
                        self.select_after_add = result.module;
                    }
                }
            }
            Request::Arrange { module, .. } => {
                if let Some(result) = self.parse::<ArrangeResult>(request, value, now) {
                    let row = result
                        .rows
                        .iter()
                        .position(|ids| ids.contains(module))
                        .map_or(0, |row| row + 1);
                    if let Some(snapshot) = self.snapshot.as_mut() {
                        snapshot.rack = Some(result.rows);
                    }
                    self.rows_seen();
                    self.say(Tone::Done, format!("{module} hangs in row {row}"), now);
                }
            }
            Request::Tempo { .. } => {
                if let Some(result) = self.parse::<TempoResult>(request, value, now) {
                    self.log.add_change(&result.change, true);
                    self.say(Tone::Done, result.change.summary.clone(), now);
                }
            }
            Request::Monitor { .. } => {
                if let Some(result) = self.parse::<MonitorResult>(request, value, now) {
                    if let Some(snapshot) = self.snapshot.as_mut() {
                        snapshot.heard = result.on;
                    }
                    let words = if result.on {
                        "you can hear the wall (m to silence it)"
                    } else {
                        "the wall is silent and plays on (m to hear it)"
                    };
                    self.say(Tone::Done, words.to_string(), now);
                }
            }
            Request::Record { .. } => {
                if let Some(result) = self.parse::<RecordResult>(request, value, now) {
                    self.recorded(&result, now);
                }
            }
            Request::Shutdown => self.exit = Some(Exit::Stopped),
            // The console asks nothing else; an answer to anything else
            // has nothing to show.
            _ => {}
        }
    }

    /// Show what a `record` answer says, and keep the header in step until
    /// the next look.
    fn recorded(&mut self, result: &RecordResult, now: Instant) {
        if let Some(change) = &result.change {
            self.log.add_change(change, true);
        }
        let file = result
            .path
            .as_deref()
            .map_or("", |path| path.rsplit('/').next().unwrap_or(path));
        let words = match (result.on, result.change.is_some()) {
            (true, true) => format!("recording {file} (r to stop)"),
            (true, false) => format!(
                "already recording {file}, {} in (r to stop)",
                format::clock(result.seconds)
            ),
            (false, true) if result.dropped > 0 => format!(
                "recorded {file}: {}, {} samples lost",
                format::clock(result.seconds),
                result.dropped
            ),
            (false, true) => format!("recorded {file}: {}", format::clock(result.seconds)),
            (false, false) => "nothing was recording".to_string(),
        };
        if let Some(snapshot) = self.snapshot.as_mut() {
            snapshot.recording = match (result.on, &result.path) {
                (true, Some(path)) => Some(kazoo_wall::protocol::Recording {
                    path: path.clone(),
                    seat: self.seat.clone(),
                    seconds: result.seconds,
                    dropped: result.dropped,
                    sample_rate: snapshot.timing.sample_rate,
                }),
                _ => None,
            };
        }
        self.say(Tone::Done, words, now);
    }

    fn parse<T: DeserializeOwned>(
        &mut self,
        request: &Request,
        value: serde_json::Value,
        now: Instant,
    ) -> Option<T> {
        match serde_json::from_value(value) {
            Ok(result) => Some(result),
            Err(err) => {
                self.say(
                    Tone::Trouble,
                    format!(
                        "the wall answered '{}' with something this console cannot read: {err}",
                        describe(request)
                    ),
                    now,
                );
                None
            }
        }
    }

    fn on_failure(&mut self, request: &Request, failure: &Failure, now: Instant) {
        match request {
            Request::Log { .. } => self.log_inflight = false,
            Request::Turn { module, knob, .. } => self
                .sent
                .retain(|turn| !(turn.module == *module && turn.knob == *knob)),
            // The moves shown ahead of the wall go back to where the wall
            // has them.
            Request::Arrange { .. } => {
                self.rack.sent = None;
                self.rack.last_move = None;
            }
            // Nothing else is held while waiting for an answer.
            _ => {}
        }
        let code = failure
            .code
            .map_or_else(String::new, |code| format!(" [{}]", code_name(code)));
        self.say(
            Tone::Trouble,
            format!("{} refused{code}: {}", describe(request), failure.message),
            now,
        );
    }

    fn say(&mut self, tone: Tone, text: String, now: Instant) {
        self.status = Some(Status {
            text,
            tone,
            at: now,
        });
    }

    fn note(&mut self, text: String, tone: LogTone) {
        self.log.add_note(text, tone, unix_now());
    }

    // -----------------------------------------------------------------------
    // Time
    // -----------------------------------------------------------------------

    /// Send the turns that are due: keys that have rested (or been held
    /// long enough), drags whose interval has passed, and anything to go at
    /// once; each only when the flood guard would take it. In the rack
    /// view, a cable carried to the edge pans the rack.
    pub fn tick(&mut self, now: Instant) {
        if self.view == View::Rack {
            self.rack.edge_pan(now);
            self.carry_plate(false);
        }
        self.tick_moves(now);
        let chosen = self.glide_beats();
        let mut index = 0;
        while index < self.pending.len() {
            let turn = &self.pending[index];
            let due = match turn.kind {
                TurnKind::Keys => {
                    now.saturating_duration_since(turn.last) >= TURN_SETTLE
                        || now.saturating_duration_since(turn.first) >= TURN_LONGEST
                }
                TurnKind::Drag => self
                    .drag_sends
                    .iter()
                    .find(|(module, knob, _)| *module == turn.module && *knob == turn.knob)
                    .is_none_or(|(_, _, at)| now.saturating_duration_since(*at) >= DRAG_INTERVAL),
                TurnKind::Now => true,
            };
            if due && self.flood.take(now) {
                let turn = self.pending.remove(index);
                if turn.kind == TurnKind::Drag {
                    self.drag_sends.retain(|(module, knob, _)| {
                        !(*module == turn.module && *knob == turn.knob)
                    });
                    self.drag_sends
                        .push((turn.module.clone(), turn.knob.clone(), now));
                }
                let glide = turn.glide.unwrap_or(chosen);
                self.send_turn(turn.module, turn.knob, turn.value, glide, now);
            } else {
                index += 1;
            }
        }
    }

    /// Hold `value` for `module`'s `knob` until it is due (replacing any
    /// value already held for it).
    fn hold_turn(
        &mut self,
        module: &str,
        knob: &str,
        value: f64,
        kind: TurnKind,
        glide: Option<f64>,
        now: Instant,
    ) {
        if let Some(turn) = self
            .pending
            .iter_mut()
            .find(|turn| turn.module == module && turn.knob == knob)
        {
            turn.value = value;
            turn.last = now;
            turn.kind = kind;
            turn.glide = glide;
        } else {
            self.pending.push(PendingTurn {
                module: module.to_string(),
                knob: knob.to_string(),
                value,
                first: now,
                last: now,
                kind,
                glide,
            });
        }
    }

    fn send_turn(&mut self, module: String, knob: String, value: f64, glide: f64, now: Instant) {
        self.jobs.push(Job::Call(Request::Turn {
            module: module.clone(),
            knob: knob.clone(),
            value,
            glide_beats: Some(glide),
        }));
        self.sent
            .retain(|turn| !(turn.module == module && turn.knob == knob));
        self.sent.push(SentTurn {
            module,
            knob,
            value,
            at: now,
        });
    }
}

/// Fold changed fingerprint shares into `held`: empty shares mean the
/// entry is gone.
fn merge_shares(held: &mut BTreeMap<String, Shares>, changed: BTreeMap<String, Shares>) {
    for (key, shares) in changed {
        if shares.is_empty() {
            held.remove(&key);
        } else {
            held.insert(key, shares);
        }
    }
}

/// Every jack of `module` a cable can plug into: its inputs, then its
/// knobs.
#[must_use]
pub fn jack_names(module: &ModuleView) -> Vec<&str> {
    module
        .inputs
        .iter()
        .map(String::as_str)
        .chain(module.knobs.iter().map(|knob| knob.name.as_str()))
        .collect()
}

fn has_outputs(module: &ModuleView) -> bool {
    !module.outputs.is_empty()
}

fn has_jacks(module: &ModuleView) -> bool {
    !module.inputs.is_empty() || !module.knobs.is_empty()
}

/// The module of a jack such as `lfo1.out`.
fn jack_module(jack: &str) -> &str {
    jack.split_once('.').map_or(jack, |(module, _)| module)
}

/// A jack such as `lfo1.out` as a [`JackRef`].
fn split_jack(jack: &str, output: bool) -> Option<JackRef> {
    let (module, name) = jack.split_once('.')?;
    Some(JackRef {
        module: module.to_string(),
        name: name.to_string(),
        output,
    })
}

/// The next focus, round the three panes.
const fn next_focus(focus: Focus, forward: bool) -> Focus {
    match (focus, forward) {
        (Focus::Wall, true) | (Focus::Log, false) => Focus::Cables,
        (Focus::Cables, true) | (Focus::Wall, false) => Focus::Log,
        (Focus::Log, true) | (Focus::Cables, false) => Focus::Wall,
    }
}

/// A cursor moved one step within `count` items.
const fn step_cursor(cursor: usize, count: usize, down: bool) -> usize {
    if count == 0 {
        0
    } else if down {
        if cursor + 1 < count {
            cursor + 1
        } else {
            count - 1
        }
    } else {
        cursor.saturating_sub(1)
    }
}

/// A glide in words: `at once`, `1 beat`, `4 beats`.
#[must_use]
pub fn glide_words(beats: f64) -> String {
    if beats <= 0.0 {
        "at once".to_string()
    } else {
        format::beats(beats)
    }
}

/// An error code as the wall names it.
fn code_name(code: ErrorCode) -> String {
    match serde_json::to_value(code) {
        Ok(serde_json::Value::String(name)) => name,
        Ok(other) => other.to_string(),
        Err(err) => format!("{code:?} ({err})"),
    }
}

/// A request in a few words, for the status line.
#[must_use]
pub fn describe(request: &Request) -> String {
    match request {
        Request::Turn { module, knob, .. } => format!("turn {module} {knob}"),
        Request::Patch { from, to, .. } => format!("patch {from} → {to}"),
        Request::Unpatch {
            cable: Some(cable), ..
        } => format!("unplug cable {cable}"),
        Request::Unpatch { to: Some(to), .. } => format!("unplug {to}"),
        Request::Unpatch { .. } => "unplug".to_string(),
        Request::Add { kind, .. } => format!("add {kind}"),
        Request::Remove { module } => format!("remove {module}"),
        Request::Arrange { module, .. } => format!("move {module}"),
        Request::Undo { change } => format!("undo change {change}"),
        Request::Tempo { bpm } => format!("tempo {}", format::bpm(*bpm)),
        Request::Monitor { on: true } => "hear the wall".to_string(),
        Request::Monitor { on: false } => "silence the wall".to_string(),
        Request::Record { on: true } => "record the wall".to_string(),
        Request::Record { on: false } => "stop recording".to_string(),
        Request::Shutdown => "stop the wall".to_string(),
        other => other.op().to_string(),
    }
}

/// Now, in Unix seconds, if the clock can say.
#[must_use]
pub fn unix_now() -> Option<i64> {
    // A clock set before 1970 leaves the times unknown.
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(None, |since| {
            Some(i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
        })
}
