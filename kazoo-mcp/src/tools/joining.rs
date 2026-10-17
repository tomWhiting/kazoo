//! `wall_walls`, `wall_join` and `wall_leave`: which walls a seat can
//! reach, and going on and off them under a name of its own (KZ-N02).

use std::fmt::Write as _;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use super::{Seat, bad, ok};
use crate::walls::{Asked, Gone, Joining, Left};

/// `wall_walls`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WallsArgs {}

/// `wall_join`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JoinArgs {
    /// The wall to join, by its name from `wall_walls`; needed only when it
    /// lists more than one.
    #[serde(default)]
    pub wall: Option<String>,
    /// The name to be on the wall as, chosen now: 1 to 24 of A-Z a-z 0-9
    /// space _ . -. The others see it beside every change made.
    #[serde(rename = "as")]
    pub name: String,
}

/// `wall_leave`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LeaveArgs {
    /// The wall to leave, by its name; needed only when this seat is on
    /// more than one.
    #[serde(default)]
    pub wall: Option<String>,
}

#[tool_router(router = joining_router, vis = "pub(super)")]
impl Seat {
    #[tool(
        name = "wall_walls",
        description = "The walls this seat can reach: each one's name (what wall_join and every tool's wall take), where it is, whether it is playing, who is on it, and the name this seat is on it as, once joined. Looking joins nothing and is never announced."
    )]
    async fn wall_walls(&self, Parameters(_): Parameters<WallsArgs>) -> CallToolResult {
        let mut text = String::new();
        for found in self.walls.found().await {
            let state = match &found.playing {
                Ok(()) if found.seats.is_empty() => "playing, nobody on it".to_string(),
                Ok(()) => format!("playing, with {}", found.seats.join(", ")),
                Err(why) => why.clone(),
            };
            let joined = found
                .joined_as
                .as_ref()
                .map_or_else(String::new, |seat| format!("; you are on it as {seat}"));
            if writeln!(text, "{} ({}): {state}{joined}", found.name, found.place).is_err() {
                return bad("the list of walls could not be written");
            }
        }
        ok(text)
    }

    #[tool(
        name = "wall_join",
        description = "Go on a wall as a name of your choosing (as: 1 to 24 of A-Z a-z 0-9 space _ . -, not one someone on it has, nor desk or kazoo-wall), so the tools play it and its news comes to you. wall is its name from wall_walls, needed only when there is more than one. Answers who else is on it. A wall that is not playing yet can be joined: it is reached as it starts. Joining a wall this seat is on already is refused; wall_leave first to come back as another name."
    )]
    async fn wall_join(&self, Parameters(args): Parameters<JoinArgs>) -> CallToolResult {
        match self
            .walls
            .join(args.wall.as_deref(), &args.name, Asked::ByTool)
            .await
        {
            Ok(Joining::Playing {
                wall,
                seats,
                revision,
            }) => {
                let others = seats
                    .iter()
                    .filter(|seat| **seat != args.name)
                    .map(String::as_str)
                    .collect::<Vec<_>>();
                let with = if others.is_empty() {
                    "nobody else on it".to_string()
                } else {
                    format!("with {}", others.join(", "))
                };
                ok(format!(
                    "On {wall} as {}, {with}, at change {revision}. Call wall_look to see it.",
                    args.name
                ))
            }
            Ok(Joining::Waiting { wall, socket }) => ok(format!(
                "On {wall} as {}, but it is not playing yet (at {}): this seat is on it as it starts.",
                args.name,
                socket.display()
            )),
            Err(why) => bad(why),
        }
    }

    #[tool(
        name = "wall_leave",
        description = "Go off a wall: its news stops and the others see this seat leave. wall is needed only when this seat is on more than one."
    )]
    async fn wall_leave(&self, Parameters(args): Parameters<LeaveArgs>) -> CallToolResult {
        match self.walls.leave(args.wall.as_deref()).await {
            Ok(Left { wall, seat, gone }) => ok(match gone {
                Gone::Yes => format!("Off {wall}, where this seat was {seat}."),
                Gone::StillListed => format!(
                    "Off {wall}, where this seat was {seat}; the wall still lists {seat}, so another connection is on it as that name."
                ),
                Gone::Unseen(why) => format!(
                    "Off {wall}, where this seat was {seat}; whether the wall has let {seat} go could not be seen ({why})."
                ),
            }),
            Err(why) => bad(why),
        }
    }
}
