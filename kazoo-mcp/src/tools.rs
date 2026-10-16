//! The MCP surface: the tools a seat plays the wall with, their argument
//! shapes, the words they answer in, and the server's handshake.
//!
//! Each tool is one method below: it builds the wall's request, asks
//! through [`Wall`], and renders the answer (see [`crate::render`]). Adding
//! a tool is adding one method.

mod batch;
use batch::BatchArgs;

use std::future::Future;
use std::sync::Arc;

use kazoo_wall::protocol::{
    CatalogueResult, ChangeResult, ListenResult, LoadResult, LogPage, LoopOp, MarkResult,
    ProbeResult, RecordResult, Request, SaveResult, SavesResult, Snapshot, TempoResult, TreeResult,
};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CacheScope, CallToolResult, ContentBlock, Implementation, ListToolsResult,
    PaginatedRequestParams, ProtocolVersion, ResultType, ServerCapabilities, ServerConfig,
};
use rmcp::service::{MaybeSendFuture, NotificationContext, RequestContext};
use rmcp::{ErrorData, Peer, RoleServer, ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tokio::sync::{mpsc, watch};

use crate::channel::Feed;
use crate::link::{Answer, Wall};
use crate::render;

/// The name this server gives in its handshake.
pub const SERVER_NAME: &str = "kazoo-mcp";

/// What the session is told about this server.
pub const INSTRUCTIONS: &str = "\
The wall is a shared modular synth that plays forever. Tom and every Claude seat can reach up \
at any time and turn a knob, move a cable, add a module or take one away; nothing is automated \
and nothing is decided in advance about how it should sound. Every change is logged under the \
seat that made it and can be undone.

How to play:
- wall_look first: every module with its knobs, every cable, the tempo, who is here and what \
it sounds like. wall_catalogue for what each kind of module does.
- Ports are named module.port: outputs like lfo1.out, inputs like vcf1.in. Every knob is also \
a jack, so a cable can go into vcf1.cutoff to move that knob.
- wall_turn glides a knob to a value over glide_beats (default 2 beats, 0 to 64): weather, \
not a crash. Values are held to each knob's range, and the dangerous ones are capped.
- A cable's amount runs from -1 to 1 (an attenuverter; 1 by default). Patching into an input \
that is already used replaces the cable there.
- Pitch is 1.0 per octave with 0 = C4; gates are high above 0.5.
- wall_listen says what the wall sounds like now, in numbers and words.
- To measure a point in the patch, wall_add a scope (inputs a and b) or a readout (one input), \
patch the signal into it (it passes it on, so it can sit inline), and wall_probe reads it: \
frequency, levels, DC, a gate's rate and duty, a pitch CV as a note, and the waveform in words.
- wall_record starts or stops recording what the wall plays to a WAV file.
- wall_save keeps the wall as it is under a name; wall_saves lists the saves; wall_load puts \
one back up while the wall plays (the wall as it was is saved first, and wall_undo of the load \
brings it back). Only Tom's console replaces or deletes a save. Once a wall is loaded, every \
module and cable is new: changes meant for the wall last looked at are refused (wall_replaced) \
until wall_look is called again.
- The wall's past is a tree (git): every change is a moment, and nothing in the past ever changes. \
wall_tree shows the branches, marks and recent moments; wall_mark names the moment the wall is at; \
wall_go puts the wall at any moment: on a branch the next change carries it on; off any line it grows a new branch (a bud) from there.

When other seats change the wall, this server sends a channel notification (source \
kazoo-wall). Those are hints about the past: call wall_look before acting on them.";

/// A tool that takes nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoArgs {}

/// `wall_catalogue`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CatalogueArgs {
    /// Only this kind (e.g. `vcf`) or family (e.g. `drive`). Leave it out
    /// for every kind.
    #[serde(default)]
    pub kind: Option<String>,
}

/// `wall_turn`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TurnArgs {
    /// The module's id, as `wall_look` lists it, e.g. `vcf1`.
    pub module: String,
    /// The knob's name, e.g. `cutoff`.
    pub knob: String,
    /// Where to turn it, in the knob's own unit (Hz, seconds, semitones...;
    /// stepped knobs take the position number). Held to the knob's range.
    pub value: f64,
    /// How long the knob takes to get there, in beats: 0 jumps, 64 is the
    /// longest. Default 2.
    #[serde(default)]
    pub glide_beats: Option<f64>,
    /// Why, in a line (1 to 200 characters): kept with the change in the
    /// log, told to the other seats and shown on its moment in the tree.
    #[serde(default)]
    pub comment: Option<String>,
}

/// `wall_patch`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PatchArgs {
    /// The output to plug from, as `module.port`, e.g. `lfo1.out`.
    pub from: String,
    /// The input to plug into, as `module.port`, e.g. `vcf1.in`; every
    /// knob is also an input by its name, e.g. `vcf1.cutoff`. A cable
    /// already in that input is replaced.
    pub to: String,
    /// How much of the signal goes through, from -1 (inverted) to 1.
    /// Default 1.
    #[serde(default)]
    pub amount: Option<f64>,
    /// Why, in a line (1 to 200 characters): kept with the change in the
    /// log, told to the other seats and shown on its moment in the tree.
    #[serde(default)]
    pub comment: Option<String>,
}

/// `wall_unpatch`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UnpatchArgs {
    /// The cable's number, as `wall_look` lists it (`#12`). Give this or
    /// `to`.
    #[serde(default)]
    pub cable: Option<u32>,
    /// The input the cable is plugged into, as `module.port`, e.g.
    /// `vcf1.cutoff`. Give this or `cable`.
    #[serde(default)]
    pub to: Option<String>,
    /// Why, in a line (1 to 200 characters): kept with the change in the
    /// log, told to the other seats and shown on its moment in the tree.
    #[serde(default)]
    pub comment: Option<String>,
}

/// `wall_add`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddArgs {
    /// The kind of module, e.g. `lfo`; `wall_catalogue` lists them.
    pub kind: String,
    /// A display name, 1 to 24 of A-Z a-z 0-9 space _ . -, e.g. `slow
    /// wobble`.
    #[serde(default)]
    pub name: Option<String>,
    /// Why, in a line (1 to 200 characters): kept with the change in the
    /// log, told to the other seats and shown on its moment in the tree.
    #[serde(default)]
    pub comment: Option<String>,
}

/// `wall_remove`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveArgs {
    /// The module's id, e.g. `lfo3`. Its cables go with it.
    pub module: String,
    /// Why, in a line (1 to 200 characters): kept with the change in the
    /// log, told to the other seats and shown on its moment in the tree.
    #[serde(default)]
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChainAction {
    Save,
    Add,
    List,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChainArgs {
    pub action: ChainAction,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub modules: Vec<String>,
    #[serde(default)]
    pub group: Option<String>,
    /// Why, in a line (1 to 200 characters): kept with the change in the
    /// log, told to the other seats and shown on its moment in the tree.
    #[serde(default)]
    pub comment: Option<String>,
}

/// `wall_group`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GroupArgs {
    /// The group's name: 1 to 24 of A-Z a-z 0-9 _ . - (no spaces), e.g. `kick`.
    pub group: String,
    /// Module ids or names, e.g. `["osc1", "klon2"]`.
    pub modules: Vec<String>,
    /// True (the default) puts them in the group; false takes them out.
    #[serde(default)]
    pub leave: bool,
}

/// `wall_undo`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UndoArgs {
    /// The change's number, as `wall_log` lists it (`#57`).
    pub change: u64,
    /// Why, in a line (1 to 200 characters): kept with the change in the
    /// log, told to the other seats and shown on its moment in the tree.
    #[serde(default)]
    pub comment: Option<String>,
}

/// `wall_log`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LogArgs {
    /// Only changes before this number, for older pages. Default: up to the
    /// latest.
    #[serde(default)]
    pub before: Option<u64>,
    /// At most this many changes: default 50, at most 500.
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `wall_tempo`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TempoArgs {
    /// Beats per minute, 20 to 300. When the wall is following the kazoo-mix
    /// desk, the desk is asked, and it changes the tempo for everyone.
    pub bpm: f64,
    /// Why, in a line (1 to 200 characters): kept with the change in the
    /// log, told to the other seats and shown on its moment in the tree.
    #[serde(default)]
    pub comment: Option<String>,
}

/// `wall_speak`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpeakArgs {
    /// A `speak` module's id, e.g. `speak1` (add one with `wall_add`).
    pub module: String,
    /// The words to say: at most 500 characters. They stay private: the
    /// other seats hear only that the module was given words.
    pub text: String,
    /// A macOS voice, e.g. `Samantha`; the system's own when left out.
    #[serde(default)]
    pub voice: Option<String>,
}

/// `wall_loop` preserves an explicit null source as patched inputs.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LoopArgs {
    /// The loop module's id, as listed by `wall_look`.
    pub module: String,
    /// True erases the tape; give this or `from`.
    #[serde(default)]
    pub erase: Option<bool>,
    /// A desk strip label, or null to record the patched inputs.
    #[serde(default, deserialize_with = "loop_from")]
    #[schemars(with = "Option<String>")]
    pub from: Option<LoopSource>,
    /// Why, in a line (1 to 200 characters): kept with the change in the
    /// log, told to the other seats and shown on its moment in the tree.
    #[serde(default)]
    pub comment: Option<String>,
}

/// A supplied source distinguishes patched inputs from a named desk strip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopSource {
    Patched,
    Strip(String),
}

fn loop_from<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<LoopSource>, D::Error> {
    Option::<String>::deserialize(deserializer)
        .map(|source| Some(source.map_or(LoopSource::Patched, LoopSource::Strip)))
}

impl LoopArgs {
    fn request(self) -> Result<Request, &'static str> {
        let op = match (self.erase == Some(true), self.from) {
            (true, None) => LoopOp::Erase,
            (false, Some(LoopSource::Patched)) => LoopOp::From { from: None },
            (false, Some(LoopSource::Strip(from))) => LoopOp::From { from: Some(from) },
            (true, Some(_)) | (false, None) => {
                return Err(
                    "Give exactly one of erase: true or from (a desk strip label, or null for patched inputs).",
                );
            }
        };
        Ok(Request::Loop {
            module: self.module,
            op,
        })
    }
}

/// `wall_save`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveArgs {
    /// The save's name: 1 to 64 of A-Z a-z 0-9 space _ . -, starting with
    /// a letter or digit, e.g. `waffles dub 2026-10-09`. Not one starting
    /// with `before-` (the wall's own) nor one already saved (any case).
    pub name: String,
}

/// `wall_load`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LoadArgs {
    /// The save's name, as `wall_saves` lists it (any case).
    pub name: String,
    /// Why, in a line (1 to 200 characters): kept with the change in the
    /// log, told to the other seats and shown on its moment in the tree.
    #[serde(default)]
    pub comment: Option<String>,
}

/// `wall_mark`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MarkArgs {
    /// The mark's name: 1 to 64 of A-Z a-z 0-9 space _ . -, starting with a
    /// letter or digit, e.g. `dawn` or `waffles low end 2026-10-13`. Not one
    /// already marked (any case): marks never move.
    pub name: String,
}

/// `wall_go`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GoArgs {
    /// The moment: a mark's name, a branch (`trunk`, or a `bud/...` name as
    /// `wall_tree` lists it), a change as `#<number>` (the wall just after that
    /// change, from `wall_log`), or a commit id (7 to 40 hex digits).
    pub moment: String,
    /// Why, in a line (1 to 200 characters): kept with the change in the
    /// log, told to the other seats and shown on its moment in the tree.
    #[serde(default)]
    pub comment: Option<String>,
}

/// `wall_tree`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TreeArgs {
    /// How many of the latest moments on the wall's line to list: default
    /// 20, at most 200.
    #[serde(default)]
    pub recent: Option<u32>,
}

/// `wall_probe`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProbeArgs {
    /// A scope's or readout's id, as `wall_look` lists it, e.g. `scope1`.
    /// Leave it out for every one on the wall (the first 16 in the wall's
    /// order).
    #[serde(default)]
    pub module: Option<String>,
}

/// `wall_record`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecordArgs {
    /// true starts recording what the wall plays; false stops the
    /// recording under way, whoever started it.
    pub on: bool,
    /// Why, in a line (1 to 200 characters): kept with the change in the
    /// log, told to the other seats and shown on its moment in the tree.
    #[serde(default)]
    pub comment: Option<String>,
}

/// One text block, as a successful tool result.
fn ok(text: impl Into<String>) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(text)])
}

/// One text block, as a failed tool result.
fn bad(text: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(text)])
}

/// A seat's MCP server.
#[derive(Clone)]
pub struct Seat {
    wall: Arc<Wall>,
    feed: mpsc::UnboundedSender<Feed>,
    peer: Arc<watch::Sender<Option<Peer<RoleServer>>>>,
    tool_router: ToolRouter<Self>,
}

impl std::fmt::Debug for Seat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Seat")
            .field("wall", &self.wall)
            .finish_non_exhaustive()
    }
}

impl Seat {
    /// A seat asking `wall`, telling the notifier of its own changes on
    /// `feed`, and handing it the session on `peer` once initialised.
    #[must_use]
    pub fn new(
        wall: Arc<Wall>,
        feed: mpsc::UnboundedSender<Feed>,
        peer: watch::Sender<Option<Peer<RoleServer>>>,
    ) -> Self {
        Self {
            wall,
            feed,
            peer: Arc::new(peer),
            tool_router: Self::tool_router(),
        }
    }

    /// Ask the wall something that changes nothing, and render the answer.
    async fn read<T: DeserializeOwned>(
        &self,
        request: Request,
        render: impl FnOnce(&T) -> Result<String, String>,
    ) -> CallToolResult {
        match self.wall.ask::<T>(request).await {
            Ok(Answer::Read(result)) => match render(&result) {
                Ok(text) => ok(text),
                Err(why) => bad(why),
            },
            Ok(Answer::Unread(why, value)) => ok(render::unread(&why, &value)),
            Err(err) => bad(err.to_string()),
        }
    }

    /// Make a change, with the sender's `comment` on it, note its number
    /// for the channel, and say what it did.
    async fn change(&self, request: Request, comment: Option<&str>) -> CallToolResult {
        match self
            .wall
            .ask_commented::<ChangeResult>(request, comment)
            .await
        {
            Ok(Answer::Read(result)) => {
                self.own(result.change.seq);
                ok(render::changed(&result))
            }
            Ok(Answer::Unread(why, value)) => ok(render::unread(&why, &value)),
            Err(err) => bad(err.to_string()),
        }
    }

    /// Tell the notifier this seat made change `seq`, so the channel's
    /// `seq` is never behind this seat's own work.
    fn own(&self, seq: u64) {
        if self.feed.send(Feed::Own(seq)).is_err() {
            eprintln!("kazoo-mcp: the notifier has stopped; change #{seq} was not noted");
        }
    }
}

#[tool_router(vis = "pub(crate)")]
impl Seat {
    #[tool(
        name = "wall_look",
        description = "The whole wall as a listing: tempo and position, the seats here, the master peaks, what it sounds like, every module with each knob's exact value (a→b while gliding) and range, each module's inputs and outputs, every cable with its number and amount, and whether a recording is under way. Call this before changing anything, and after a channel notification: once another wall has been loaded, changes meant for the wall last looked at are refused (wall_replaced) until this seat looks again."
    )]
    async fn wall_look(&self, Parameters(_): Parameters<NoArgs>) -> CallToolResult {
        self.read::<Snapshot>(Request::Look, |snapshot| {
            // Changes from now on are for the wall this seat has seen.
            self.wall.saw(snapshot.wall);
            Ok(render::look(snapshot))
        })
        .await
    }

    #[tool(
        name = "wall_catalogue",
        description = "What each kind of module does: its knobs with ranges, units, defaults, travel and jack law, the named positions of stepped knobs, and its inputs and outputs with what they carry (audio, gate or cv). Every knob is also a jack by its name. Give kind to narrow it to one kind (e.g. vcf) or one family (e.g. drive); the whole catalogue is long."
    )]
    async fn wall_catalogue(&self, Parameters(args): Parameters<CatalogueArgs>) -> CallToolResult {
        let only = args.kind.as_deref().map(str::trim);
        self.read::<CatalogueResult>(Request::Catalogue, |catalogue| {
            render::catalogue(catalogue, only)
        })
        .await
    }

    #[tool(
        name = "wall_turn",
        description = "Turn one knob: module id, knob name and the value in the knob's own unit (Hz, seconds, semitones; stepped knobs take a position number). It glides there over glide_beats (default 2 beats, 0 jumps, at most 64). The value is held to the knob's range. Answers with the change's number and what happened."
    )]
    async fn wall_turn(&self, Parameters(args): Parameters<TurnArgs>) -> CallToolResult {
        self.change(
            Request::Turn {
                module: args.module,
                knob: args.knob,
                value: args.value,
                glide_beats: args.glide_beats,
            },
            args.comment.as_deref(),
        )
        .await
    }

    #[tool(
        name = "wall_patch",
        description = "Plug a cable from an output (module.port, e.g. lfo1.out) into an input (module.port, e.g. vcf1.in). Every knob is also an input by its name, so lfo1.out into vcf1.cutoff moves the cutoff. amount runs from -1 (inverted) to 1, default 1. An input takes one cable: plugging into a used one replaces it. Outputs can feed any number of inputs. Answers with the cable's number."
    )]
    async fn wall_patch(&self, Parameters(args): Parameters<PatchArgs>) -> CallToolResult {
        self.change(
            Request::Patch {
                from: args.from,
                to: args.to,
                amount: args.amount,
            },
            args.comment.as_deref(),
        )
        .await
    }

    #[tool(
        name = "wall_unpatch",
        description = "Unplug one cable: by its number (cable, as wall_look lists it) or by the input it is plugged into (to, as module.port). Give exactly one of them."
    )]
    async fn wall_unpatch(&self, Parameters(args): Parameters<UnpatchArgs>) -> CallToolResult {
        match (args.cable, args.to.clone()) {
            (Some(cable), None) => {
                self.change(
                    Request::Unpatch {
                        cable: Some(cable),
                        to: None,
                    },
                    args.comment.as_deref(),
                )
                .await
            }
            (None, Some(to)) => {
                self.change(
                    Request::Unpatch {
                        cable: None,
                        to: Some(to),
                    },
                    args.comment.as_deref(),
                )
                .await
            }
            (Some(_), Some(_)) | (None, None) => bad(
                "wall_unpatch takes exactly one of cable (its number) or to (the input it is \
                 plugged into, as module.port)",
            ),
        }
    }

    #[tool(
        name = "wall_add",
        description = "Add a module of a kind wall_catalogue lists, with an optional display name (1 to 24 of A-Z a-z 0-9 space _ . -). Answers with its id (kind and number, e.g. lfo3), which is how every other tool names it. A new module starts at its defaults with nothing plugged in."
    )]
    async fn wall_add(&self, Parameters(args): Parameters<AddArgs>) -> CallToolResult {
        self.change(
            Request::Add {
                alias: None,
                kind: args.kind,
                name: args.name,
                place: None,
            },
            args.comment.as_deref(),
        )
        .await
    }

    #[tool(
        name = "wall_remove",
        description = "Take a module away by its id. Its cables go with it; the change log keeps everything, so wall_undo can bring it back."
    )]
    async fn wall_remove(&self, Parameters(args): Parameters<RemoveArgs>) -> CallToolResult {
        self.change(
            Request::Remove {
                module: args.module,
            },
            args.comment.as_deref(),
        )
        .await
    }

    #[tool(
        name = "wall_batch",
        description = "Make several changes toward one idea in one call, so the tree shows that idea with its note. note is 1 to 200 characters on one line. changes is an ordered array of add, remove, turn, patch, unpatch or tempo objects with the same fields as their single tools. An add may have as: a unique label without spaces or dots; later modules and ports can use @label or @label.out. Any step may carry a comment (1 to 200 characters, one line) saying why, shown on its own moment in the tree. Every step succeeds or the entire batch is refused, naming the refused step by its index from 0. Answers with every step, numbered from 0, and each add's id beside its label."
    )]
    async fn wall_batch(&self, Parameters(args): Parameters<BatchArgs>) -> CallToolResult {
        let request = match args.request() {
            Ok(request) => request,
            Err(err) => return bad(format!("invalid batch: {err}")),
        };
        match self
            .wall
            .ask::<kazoo_wall::protocol::BatchResult>(request)
            .await
        {
            Ok(Answer::Read(result)) => {
                self.own(result.change.seq);
                let mut lines = vec![format!(
                    "Change #{}: {}",
                    result.change.seq, result.change.summary
                )];
                if let kazoo_wall::protocol::What::Batch {
                    changes, comments, ..
                } = &result.change.what
                {
                    for (index, (change, step)) in changes.iter().zip(&result.steps).enumerate() {
                        // Numbered from 0, as a refusal names its step.
                        let mut line =
                            format!("Step {index}: {}", kazoo_wall::change::describe(change));
                        if let (Some(label), Some(id)) =
                            (step["as"].as_str(), step["module"].as_str())
                        {
                            line = format!("{line} (@{label} = {id})");
                        }
                        if let Some(comment) = comments.get(index).and_then(Option::as_deref) {
                            line = format!("{line} \u{2014} \"{comment}\"");
                        }
                        lines.push(line);
                    }
                }
                ok(lines.join("\n"))
            }
            Ok(Answer::Unread(why, value)) => ok(render::unread(&why, &value)),
            Err(err) => bad(err.to_string()),
        }
    }

    #[tool(
        name = "wall_chain",
        description = "Save selected modules and their internal cables as a reusable named chain, add a saved copy on a fresh rack row as one undo step, or list saved chains. action is save, add or list. save takes name and either modules or group; add takes name; list takes no other arguments. Cables to outside modules are left off and counted."
    )]
    pub(crate) async fn wall_chain(
        &self,
        Parameters(args): Parameters<ChainArgs>,
    ) -> CallToolResult {
        // Only an add changes the wall; a comment on a save or a list
        // would go nowhere, so it is refused rather than dropped.
        if args.comment.is_some() && args.action != ChainAction::Add {
            return bad(
                "a comment goes on chain add, which changes the wall; a chain save or list changes nothing",
            );
        }
        match args.action {
            ChainAction::List
                if args.name.is_none() && args.modules.is_empty() && args.group.is_none() =>
            {
                self.read::<kazoo_wall::protocol::ChainsResult>(Request::Chains, |result| {
                    Ok(if result.names.is_empty() {
                        "No saved chains.".to_string()
                    } else {
                        format!("Chains: {}.", result.names.join(", "))
                    })
                })
                .await
            }
            ChainAction::Add if args.modules.is_empty() && args.group.is_none() => {
                match args.name {
                    Some(name) => {
                        self.change(Request::ChainAdd { name }, args.comment.as_deref())
                            .await
                    }
                    None => bad("chain add requires a name"),
                }
            }
            ChainAction::Save => {
                let Some(name) = args.name else {
                    return bad("chain save requires a name");
                };
                let modules = match (args.group, args.modules.is_empty()) {
                    (Some(group), true) => match self.wall.ask::<Snapshot>(Request::Look).await {
                        Ok(Answer::Read(snapshot)) => match snapshot
                            .groups
                            .into_iter()
                            .find(|(name, _)| name.eq_ignore_ascii_case(&group))
                        {
                            Some((_, modules)) => modules,
                            None => return bad(format!("no group called '{group}'")),
                        },
                        Ok(Answer::Unread(why, value)) => return bad(render::unread(&why, &value)),
                        Err(err) => return bad(err.to_string()),
                    },
                    (None, false) => args.modules,
                    _ => return bad("chain save requires either modules or a group"),
                };
                self.read::<kazoo_wall::protocol::ChainSaveResult>(
                    Request::ChainSave {
                        name,
                        modules,
                        overwrite: false,
                    },
                    |result| {
                        Ok(format!(
                            "Saved {} modules, {} cables, {} {} to the outside left off.",
                            result.modules,
                            result.cables,
                            result.outside,
                            if result.outside == 1 {
                                "cable"
                            } else {
                                "cables"
                            }
                        ))
                    },
                )
                .await
            }
            ChainAction::List | ChainAction::Add => {
                bad("chain list takes no other arguments; chain add takes only a name")
            }
        }
    }

    #[tool(
        name = "wall_group",
        description = "Put modules in a named group (a tag, e.g. a kick chain), or take them out with leave: true. A group is made by its first module and goes with its last; a removed module leaves its groups. Grouping is not logged and is not undone with wall_undo. wall_look lists the groups; consoles can show one group alone."
    )]
    async fn wall_group(&self, Parameters(args): Parameters<GroupArgs>) -> CallToolResult {
        self.read(
            Request::Group {
                group: args.group,
                modules: args.modules,
                join: !args.leave,
            },
            |result| Ok(render::groups(result)),
        )
        .await
    }

    #[tool(
        name = "wall_undo",
        description = "Undo a change by its number (from wall_log or a tool's answer), whoever made it: the inverse is applied as a new change. Refused when the change no longer applies."
    )]
    async fn wall_undo(&self, Parameters(args): Parameters<UndoArgs>) -> CallToolResult {
        match self
            .wall
            .ask_commented::<serde_json::Value>(
                Request::Undo {
                    change: args.change,
                },
                args.comment.as_deref(),
            )
            .await
        {
            Ok(Answer::Read(value)) => {
                if value.get("backup").is_some() {
                    match serde_json::from_value::<LoadResult>(value.clone()) {
                        Ok(result) => {
                            self.own(result.change.seq);
                            ok(render::went(&result))
                        }
                        Err(err) => ok(render::unread(&err.to_string(), &value)),
                    }
                } else {
                    match serde_json::from_value::<ChangeResult>(value.clone()) {
                        Ok(result) => {
                            self.own(result.change.seq);
                            ok(render::changed(&result))
                        }
                        Err(err) => ok(render::unread(&err.to_string(), &value)),
                    }
                }
            }
            Ok(Answer::Unread(why, value)) => ok(render::unread(&why, &value)),
            Err(err) => bad(err.to_string()),
        }
    }

    #[tool(
        name = "wall_log",
        description = "The recent changes, oldest first: number, time (UTC), who, and what. limit is at most 500 (default 50); before pages back to older changes."
    )]
    async fn wall_log(&self, Parameters(args): Parameters<LogArgs>) -> CallToolResult {
        self.read::<LogPage>(
            Request::Log {
                before: args.before,
                limit: args.limit,
            },
            |page| Ok(render::log(page)),
        )
        .await
    }

    #[tool(
        name = "wall_listen",
        description = "What the wall sounds like right now, heard four times a second: a line of plain words (e.g. dark, sparse, slow pulse around A2, quiet) and the numbers behind it: RMS and peak in dBFS, spectral centroid, the low/mid/high energy balance, onsets per second, and the dominant pitch if there is one."
    )]
    async fn wall_listen(&self, Parameters(_): Parameters<NoArgs>) -> CallToolResult {
        self.read::<ListenResult>(Request::Listen, |heard| Ok(render::listen(heard)))
            .await
    }

    #[tool(
        name = "wall_probe",
        description = "What a scope or readout module is measuring, as numbers: for each input its latest value, low and high, mean (DC offset), RMS and peak in dBFS, frequency (and whether it is steady, wavering or paused) and shape (sine, triangle, saw, square, pulse), its rate and duty as a gate (and whether it has paused), and its lowest and highest (and any non-numbers counted) since its reset gate; a pitch CV as a note with cents and Hz; non-numbers arriving (a fault upstream) are counted, never measured. A scope also says how its screen is set (span, zoom, trigger, trigger/free/xy) and draws each trace as a sparkline, with the picture in words. Give module (e.g. scope1) for one, or leave it out for every one on the wall. Add one with wall_add (scope or readout) and patch the signal into it: it passes the signal on, so it can sit inline."
    )]
    async fn wall_probe(&self, Parameters(args): Parameters<ProbeArgs>) -> CallToolResult {
        self.read::<ProbeResult>(
            Request::Probe {
                module: args.module,
                modules: Vec::new(),
            },
            |result| Ok(render::probe(result)),
        )
        .await
    }

    #[tool(
        name = "wall_speak",
        description = "Give a speak module words to say, rendered with macOS `say` (this can take a few seconds; the answer comes when they are ready). The module plays them on its gate input (once, looped or held, by its mode knob) out of its out jack; patch a clock or a gate into it, and its out into an out module or a vocoder's modulator. The words stay private: other seats hear only that the module was given words."
    )]
    async fn wall_speak(&self, Parameters(args): Parameters<SpeakArgs>) -> CallToolResult {
        self.change(
            Request::Speak {
                module: args.module,
                text: args.text,
                voice: args.voice,
            },
            None,
        )
        .await
    }

    #[tool(
        name = "wall_loop",
        description = "Erase a loop module's tape (undoable with wall_undo), or set which desk strip it records from (e.g. `mouth/dry` for Tom's voice; `null` to record what is patched into its left and right). To record, turn its `record` knob to 1 with wall_turn: it starts at the loop's next bar line and stops at the bar line after you turn it back to 0. Its `bars` and `beats` knobs set the length of a blank loop; two loops of unequal lengths (17 and 23 bars) drift against each other and meet again rarely."
    )]
    async fn wall_loop(&self, Parameters(args): Parameters<LoopArgs>) -> CallToolResult {
        let comment = args.comment.clone();
        match args.request() {
            Ok(request) => self.change(request, comment.as_deref()).await,
            Err(why) => bad(why),
        }
    }

    #[tool(
        name = "wall_record",
        description = "Start (on: true) or stop (on: false) recording what the wall plays: a 32-bit float stereo WAV at the wall's rate, in Tom's ~/Music/kazoo-wall, named for the time it starts. It records the wall even while its sound is silenced. Answers with the file's path, and on a stop how long it is. Starting while a recording is under way, or stopping when none is, changes nothing and says so. wall_look shows a recording under way."
    )]
    async fn wall_record(&self, Parameters(args): Parameters<RecordArgs>) -> CallToolResult {
        match self
            .wall
            .ask_commented::<RecordResult>(Request::Record { on: args.on }, args.comment.as_deref())
            .await
        {
            Ok(Answer::Read(result)) => {
                if let Some(change) = &result.change {
                    self.own(change.seq);
                }
                ok(render::recorded(&result))
            }
            Ok(Answer::Unread(why, value)) => ok(render::unread(&why, &value)),
            Err(err) => bad(err.to_string()),
        }
    }

    #[tool(
        name = "wall_save",
        description = "Save the wall as it is now under a name (1 to 64 of A-Z a-z 0-9 space _ . -): every module with its knobs where they are going, every cable, the tempo and the rack's rows. The sound does not change and nothing is logged; the other seats are told. A name already saved is refused (only Tom's console can replace or delete a save), as are names starting with before-, which are the wall's own. A seat may save three times a minute (slow_down beyond), and not once there are 200 saves (full)."
    )]
    async fn wall_save(&self, Parameters(args): Parameters<SaveArgs>) -> CallToolResult {
        self.read::<SaveResult>(
            Request::Save {
                name: args.name,
                overwrite: false,
            },
            |result| Ok(render::saved(result)),
        )
        .await
    }

    #[tool(
        name = "wall_saves",
        description = "Every save, newest first: its name, when it was saved (UTC), its tempo, and how many modules and cables it holds; which are the wall's own (before-load-..., the wall as it was before it was replaced); and any file among them that cannot be loaded, with why."
    )]
    async fn wall_saves(&self, Parameters(_): Parameters<NoArgs>) -> CallToolResult {
        self.read::<SavesResult>(Request::Saves, |result| Ok(render::saves(result)))
            .await
    }

    #[tool(
        name = "wall_load",
        description = "Replace the whole wall with a save (by its name from wall_saves) while it plays on: the master fades out and back in over a few hundredths of a second. The wall as it is is kept first (as before-load-<time>, or in the save that already holds it unchanged), and the load is one change: wall_undo with its number brings the wall back exactly. Every seat is told. Call wall_look afterwards: every module and cable is new, and changes are refused (wall_replaced) until this seat has looked. A seat may replace the wall (a load, or undoing one) three times a minute (slow_down beyond)."
    )]
    async fn wall_load(&self, Parameters(args): Parameters<LoadArgs>) -> CallToolResult {
        match self
            .wall
            .ask_commented::<LoadResult>(Request::Load { name: args.name }, args.comment.as_deref())
            .await
        {
            Ok(Answer::Read(result)) => {
                self.own(result.change.seq);
                ok(render::loaded(&result))
            }
            Ok(Answer::Unread(why, value)) => ok(render::unread(&why, &value)),
            Err(err) => bad(err.to_string()),
        }
    }

    #[tool(
        name = "wall_mark",
        description = "Name the moment the wall is at (just after its latest change) so anyone can come back to it with wall_go: 1 to 64 of A-Z a-z 0-9 space _ . -, starting with a letter or digit, e.g. dawn. A mark is a name in the wall's tree (a git tag, mark/<name>); it never moves and is never deleted, and the sound does not change. A name already marked (any case) is refused (mark_exists). Every seat is told. A seat may mark three times a minute (slow_down beyond), and not once the tree holds 1,000 marks (full)."
    )]
    async fn wall_mark(&self, Parameters(args): Parameters<MarkArgs>) -> CallToolResult {
        self.read::<MarkResult>(Request::Mark { name: args.name }, |result| {
            Ok(render::marked(result))
        })
        .await
    }

    #[tool(
        name = "wall_go",
        description = "Put the whole wall at a moment from its past while it plays on: a mark's name or a branch from wall_tree, a change as #<number> from wall_log (the wall just after it), or a commit id. It is a cut: the master fades out and back in over a few hundredths of a second. Nothing in the past changes: the wall as it is is kept first (as before-go-<time>, or in the save that already holds it unchanged), the go is one change, and wall_undo with its number brings the wall back exactly to where it was. On a branch, the next change carries it on. Off any line, the first change anyone makes grows a bud, a new branch named for the moment and the time; with no change, nothing grows. Every seat is told. Call wall_look afterwards: every module and cable is new, and changes are refused (wall_replaced) until this seat has looked. A seat may replace the wall (a load, a go, or undoing one) three times a minute (slow_down beyond)."
    )]
    async fn wall_go(&self, Parameters(args): Parameters<GoArgs>) -> CallToolResult {
        match self
            .wall
            .ask_commented::<LoadResult>(
                Request::Go {
                    moment: args.moment,
                },
                args.comment.as_deref(),
            )
            .await
        {
            Ok(Answer::Read(result)) => {
                self.own(result.change.seq);
                ok(render::went(&result))
            }
            Ok(Answer::Unread(why, value)) => ok(render::unread(&why, &value)),
            Err(err) => bad(err.to_string()),
        }
    }

    #[tool(
        name = "wall_tree",
        description = "The wall's tree, where every change is a moment: where the wall is (its line and the moment), every branch (trunk and the buds, newest first, each with the moment it grew from), every mark and every save's mark, and the latest moments on the wall's line (recent: default 20, at most 200), each with its change number, who, when (UTC) and what. Anything listed is a moment wall_go takes. Anyone may ask, at most ten times in a few seconds."
    )]
    async fn wall_tree(&self, Parameters(args): Parameters<TreeArgs>) -> CallToolResult {
        self.read::<TreeResult>(
            Request::Tree {
                recent: args.recent,
            },
            |result| Ok(render::tree(result)),
        )
        .await
    }

    #[tool(
        name = "wall_tempo",
        description = "Set the tempo in BPM (20 to 300). On its own clock the wall changes at once; while it follows the kazoo-mix desk, the desk is asked and changes the tempo for everyone."
    )]
    async fn wall_tempo(&self, Parameters(args): Parameters<TempoArgs>) -> CallToolResult {
        match self
            .wall
            .ask_commented::<TempoResult>(Request::Tempo { bpm: args.bpm }, args.comment.as_deref())
            .await
        {
            Ok(Answer::Read(result)) => {
                self.own(result.change.seq);
                ok(render::tempo(&result))
            }
            Ok(Answer::Unread(why, value)) => ok(render::unread(&why, &value)),
            Err(err) => bad(err.to_string()),
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Seat {
    fn supported_protocol_versions(&self) -> std::borrow::Cow<'static, [ProtocolVersion]> {
        // Claude channel notifications cannot travel over the 2026-07-28
        // revision, and they are how a seat hears the others.
        std::borrow::Cow::Borrowed(&[
            ProtocolVersion::V_2024_11_05,
            ProtocolVersion::V_2025_03_26,
            ProtocolVersion::V_2025_06_18,
            ProtocolVersion::V_2025_11_25,
        ])
    }

    fn get_info(&self) -> ServerConfig {
        let mut capabilities = ServerCapabilities::builder().enable_tools().build();
        capabilities.experimental = Some(std::collections::BTreeMap::from_iter([(
            "claude/channel".to_owned(),
            serde_json::Map::new(),
        )]));
        ServerConfig::new(capabilities)
            .with_server_info(Implementation::new(SERVER_NAME, env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }

    // The tool list is written out here rather than left to the
    // tool_handler macro, as dot-seat does: the macro's list_tools is an
    // `async fn` that never awaits, which clippy names as unused_async. The
    // fields are the macro's, field for field.
    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, ErrorData>> + MaybeSendFuture + '_ {
        let supports_cache_hints = context
            .protocol_version()
            .is_some_and(|version| version >= ProtocolVersion::V_2026_07_28);
        std::future::ready(Ok(ListToolsResult {
            result_type: Some(ResultType::COMPLETE),
            tools: self.tool_router.list_all(),
            meta: None,
            next_cursor: None,
            ttl_ms: supports_cache_hints.then_some(0),
            cache_scope: supports_cache_hints.then_some(CacheScope::Public),
        }))
    }

    fn on_initialized(
        &self,
        context: NotificationContext<RoleServer>,
    ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
        self.peer.send_replace(Some(context.peer));
        std::future::ready(())
    }
}

#[cfg(test)]
mod tests;
