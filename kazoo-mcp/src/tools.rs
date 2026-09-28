//! The MCP surface: the tools a seat plays the wall with, their argument
//! shapes, the words they answer in, and the server's handshake.
//!
//! Each tool is one method below: it builds the wall's request, asks
//! through [`Wall`], and renders the answer (see [`crate::render`]). Adding
//! a tool is adding one method.

use std::future::Future;
use std::sync::Arc;

use kazoo_wall::protocol::{
    CatalogueResult, ChangeResult, ListenResult, LogPage, RecordResult, Request, Snapshot,
    TempoResult,
};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CacheScope, CallToolResult, ContentBlock, Implementation, ListToolsResult,
    PaginatedRequestParams, ProtocolVersion, ResultType, ServerCapabilities, ServerInfo,
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
- wall_record starts or stops recording what the wall plays to a WAV file.

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
}

/// `wall_remove`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveArgs {
    /// The module's id, e.g. `lfo3`. Its cables go with it.
    pub module: String,
}

/// `wall_undo`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UndoArgs {
    /// The change's number, as `wall_log` lists it (`#57`).
    pub change: u64,
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
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TempoArgs {
    /// Beats per minute, 20 to 300. When the wall is following the kazoo-mix
    /// desk, the desk is asked, and it changes the tempo for everyone.
    pub bpm: f64,
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

/// `wall_record`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecordArgs {
    /// true starts recording what the wall plays; false stops the
    /// recording under way, whoever started it.
    pub on: bool,
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

    /// Make a change, note its number for the channel, and say what it did.
    async fn change(&self, request: Request) -> CallToolResult {
        match self.wall.ask::<ChangeResult>(request).await {
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
        description = "The whole wall as a listing: tempo and position, the seats here, the master peaks, what it sounds like, every module with each knob's exact value (a→b while gliding) and range, each module's inputs and outputs, every cable with its number and amount, and whether a recording is under way. Call this before changing anything, and after a channel notification."
    )]
    async fn wall_look(&self, Parameters(_): Parameters<NoArgs>) -> CallToolResult {
        self.read::<Snapshot>(Request::Look, |snapshot| Ok(render::look(snapshot)))
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
        self.change(Request::Turn {
            module: args.module,
            knob: args.knob,
            value: args.value,
            glide_beats: args.glide_beats,
        })
        .await
    }

    #[tool(
        name = "wall_patch",
        description = "Plug a cable from an output (module.port, e.g. lfo1.out) into an input (module.port, e.g. vcf1.in). Every knob is also an input by its name, so lfo1.out into vcf1.cutoff moves the cutoff. amount runs from -1 (inverted) to 1, default 1. An input takes one cable: plugging into a used one replaces it. Outputs can feed any number of inputs. Answers with the cable's number."
    )]
    async fn wall_patch(&self, Parameters(args): Parameters<PatchArgs>) -> CallToolResult {
        self.change(Request::Patch {
            from: args.from,
            to: args.to,
            amount: args.amount,
        })
        .await
    }

    #[tool(
        name = "wall_unpatch",
        description = "Unplug one cable: by its number (cable, as wall_look lists it) or by the input it is plugged into (to, as module.port). Give exactly one of them."
    )]
    async fn wall_unpatch(&self, Parameters(args): Parameters<UnpatchArgs>) -> CallToolResult {
        match (args.cable, args.to) {
            (Some(cable), None) => {
                self.change(Request::Unpatch {
                    cable: Some(cable),
                    to: None,
                })
                .await
            }
            (None, Some(to)) => {
                self.change(Request::Unpatch {
                    cable: None,
                    to: Some(to),
                })
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
        self.change(Request::Add {
            kind: args.kind,
            name: args.name,
            place: None,
        })
        .await
    }

    #[tool(
        name = "wall_remove",
        description = "Take a module away by its id. Its cables go with it; the change log keeps everything, so wall_undo can bring it back."
    )]
    async fn wall_remove(&self, Parameters(args): Parameters<RemoveArgs>) -> CallToolResult {
        self.change(Request::Remove {
            module: args.module,
        })
        .await
    }

    #[tool(
        name = "wall_undo",
        description = "Undo a change by its number (from wall_log or a tool's answer), whoever made it: the inverse is applied as a new change. Refused when the change no longer applies."
    )]
    async fn wall_undo(&self, Parameters(args): Parameters<UndoArgs>) -> CallToolResult {
        self.change(Request::Undo {
            change: args.change,
        })
        .await
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
        name = "wall_speak",
        description = "Give a speak module words to say, rendered with macOS `say` (this can take a few seconds; the answer comes when they are ready). The module plays them on its gate input (once, looped or held, by its mode knob) out of its out jack; patch a clock or a gate into it, and its out into an out module or a vocoder's modulator. The words stay private: other seats hear only that the module was given words."
    )]
    async fn wall_speak(&self, Parameters(args): Parameters<SpeakArgs>) -> CallToolResult {
        self.change(Request::Speak {
            module: args.module,
            text: args.text,
            voice: args.voice,
        })
        .await
    }

    #[tool(
        name = "wall_record",
        description = "Start (on: true) or stop (on: false) recording what the wall plays: a 32-bit float stereo WAV at the wall's rate, in Tom's ~/Music/kazoo-wall, named for the time it starts. It records the wall even while its sound is silenced. Answers with the file's path, and on a stop how long it is. Starting while a recording is under way, or stopping when none is, changes nothing and says so. wall_look shows a recording under way."
    )]
    async fn wall_record(&self, Parameters(args): Parameters<RecordArgs>) -> CallToolResult {
        match self
            .wall
            .ask::<RecordResult>(Request::Record { on: args.on })
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
        name = "wall_tempo",
        description = "Set the tempo in BPM (20 to 300). On its own clock the wall changes at once; while it follows the kazoo-mix desk, the desk is asked and changes the tempo for everyone."
    )]
    async fn wall_tempo(&self, Parameters(args): Parameters<TempoArgs>) -> CallToolResult {
        match self
            .wall
            .ask::<TempoResult>(Request::Tempo { bpm: args.bpm })
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

    fn get_info(&self) -> ServerInfo {
        let mut capabilities = ServerCapabilities::builder().enable_tools().build();
        capabilities.experimental = Some(std::collections::BTreeMap::from_iter([(
            "claude/channel".to_owned(),
            serde_json::Map::new(),
        )]));
        ServerInfo::new(capabilities)
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
mod tests {
    use super::*;

    /// Every tool, in the router's order.
    const TOOLS: [&str; 13] = [
        "wall_add",
        "wall_catalogue",
        "wall_listen",
        "wall_log",
        "wall_look",
        "wall_patch",
        "wall_record",
        "wall_remove",
        "wall_speak",
        "wall_tempo",
        "wall_turn",
        "wall_undo",
        "wall_unpatch",
    ];

    #[test]
    fn the_router_holds_every_tool_with_a_description_and_schema() {
        let tools = Seat::tool_router().list_all();
        let mut names = tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>();
        names.sort_unstable();
        assert_eq!(names, TOOLS);
        for tool in &tools {
            let about = tool.description.as_deref().unwrap_or_default();
            assert!(about.len() > 60, "{} is barely described", tool.name);
            assert_eq!(
                tool.input_schema.get("type").and_then(|kind| kind.as_str()),
                Some("object"),
                "{}",
                tool.name
            );
        }
        let turn = tools
            .iter()
            .find(|tool| tool.name == "wall_turn")
            .map(|tool| serde_json::Value::Object(tool.input_schema.as_ref().clone()))
            .unwrap_or_default();
        let required = turn["required"].as_array().cloned().unwrap_or_default();
        assert_eq!(required, ["module", "knob", "value"]);
        let glide = turn["properties"]["glide_beats"]["description"]
            .as_str()
            .unwrap_or_default();
        assert!(glide.contains("beats"), "{glide}");
    }

    #[test]
    fn arguments_are_strict() {
        assert!(serde_json::from_str::<TurnArgs>(r#"{"module":"vcf1","knob":"cutoff"}"#).is_err());
        assert!(
            serde_json::from_str::<TurnArgs>(
                r#"{"module":"vcf1","knob":"cutoff","value":800,"glide":4}"#
            )
            .is_err(),
            "an unknown field is refused, not ignored"
        );
        let unpatch: UnpatchArgs = serde_json::from_str(r#"{"to":"vcf1.cutoff"}"#).unwrap();
        assert_eq!(unpatch.to.as_deref(), Some("vcf1.cutoff"));
        assert_eq!(unpatch.cable, None);
        assert!(serde_json::from_str::<NoArgs>("{}").is_ok());
        assert!(
            serde_json::from_str::<RecordArgs>("{}").is_err(),
            "on is required"
        );
        let record: RecordArgs = serde_json::from_str(r#"{"on":true}"#).unwrap();
        assert!(record.on);
    }
}
