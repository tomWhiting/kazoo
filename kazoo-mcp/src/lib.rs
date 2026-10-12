//! The wall for a Claude seat: a stdio MCP server and wall notifications.
//!
//! Stdout is the MCP transport. Messages for people go to stderr.

mod channel;
mod cli;
mod link;
mod render;
mod run;
mod tools;

pub use run::{Args, USAGE, cli, parse_args, run};
