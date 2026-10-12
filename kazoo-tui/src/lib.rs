//! Kazoo TUI: the mouth synth, tracking views, and mixing desk.

mod app;
mod input;
mod state;
mod status;
#[cfg(test)]
mod test_support;
mod theme;
mod ui;

mod run;

pub use run::{Args, USAGE, cli, parse_args, run};
