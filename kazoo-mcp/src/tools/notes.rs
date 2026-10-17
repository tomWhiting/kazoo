use super::Seat;
use crate::render;
use kazoo_wall::protocol::{NotesResult, Request};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotesArgs {
    /// The change number whose description and attachments to read.
    pub seq: u64,
}

#[tool_router(router = notes_router, vis = "pub(super)")]
impl Seat {
    #[tool(
        name = "wall_notes",
        description = "Read a batch's description and attached texts, such as its proof or score, by change number."
    )]
    async fn wall_notes(&self, Parameters(args): Parameters<NotesArgs>) -> CallToolResult {
        self.read::<NotesResult>(Request::Notes { seq: args.seq }, |notes| {
            Ok(render::notes(notes))
        })
        .await
    }

    pub(crate) fn all_tools() -> ToolRouter<Self> {
        Self::tool_router() + Self::notes_router()
    }
}
