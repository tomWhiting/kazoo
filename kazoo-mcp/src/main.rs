//! kazoo-mcp: the wall for a Claude seat.
//!
//! A stdio MCP server, one per Claude session. Its tools play the shared
//! modular synth that `kazoo-wall` keeps running (see
//! `design/wall/DESIGN.md`), and it tells the session when other seats
//! change the wall with `notifications/claude/channel`.
//!
//! - [`cli`]: the command line.
//! - [`link`]: the line to the wall's daemon.
//! - [`tools`]: the MCP tools and handshake.
//! - [`render`]: the wall's answers as text.
//! - [`channel`]: the notifications.
//!
//! Stdout is the MCP transport: everything this process says for people
//! goes to stderr.

mod channel;
mod cli;
mod link;
mod render;
mod tools;

use std::process::ExitCode;
use std::sync::Arc;

use rmcp::ServiceExt as _;
use tokio::sync::{mpsc, watch};

use crate::cli::{Invocation, Options};
use crate::link::Wall;
use crate::tools::Seat;

fn main() -> ExitCode {
    let env_seat = match std::env::var(cli::SEAT_ENV) {
        Ok(seat) => Some(seat),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            eprintln!(
                "kazoo-mcp: {} is not valid text: use 1 to 24 of A-Z a-z 0-9 space _ . -",
                cli::SEAT_ENV
            );
            return ExitCode::from(2);
        }
    };
    let invocation = cli::parse(
        std::env::args().skip(1),
        env_seat,
        kazoo_wall::paths::socket_path,
    );
    let options = match invocation {
        Ok(Invocation::Serve(options)) => options,
        Ok(Invocation::Help) => {
            println!("{}", cli::USAGE);
            return ExitCode::SUCCESS;
        }
        Ok(Invocation::Version) => {
            println!("{}", link::CLIENT);
            return ExitCode::SUCCESS;
        }
        Err(why) => {
            eprintln!("kazoo-mcp: {why}");
            return ExitCode::from(2);
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("kazoo-mcp: the async runtime could not start: {err}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(serve(options)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("kazoo-mcp: {why}");
            ExitCode::FAILURE
        }
    }
}

/// Serve the seat on stdio until the session ends.
async fn serve(options: Options) -> Result<(), String> {
    let Options {
        seat,
        notify_every,
        socket,
    } = options;
    eprintln!(
        "kazoo-mcp: seat {seat}, wall at {}, news at most every {} seconds",
        socket.display(),
        notify_every.as_secs()
    );
    let (feed, heard) = mpsc::unbounded_channel();
    let (peer, session) = watch::channel(None);
    let follower = tokio::spawn(link::follow(socket.clone(), seat.clone(), feed.clone()));
    let notifier = tokio::spawn(channel::notify(seat.clone(), notify_every, heard, session));
    let server = Seat::new(Arc::new(Wall::new(socket, seat)), feed, peer);
    let outcome = match server
        .serve((tokio::io::stdin(), tokio::io::stdout()))
        .await
    {
        Ok(running) => running
            .waiting()
            .await
            .map(|_| ())
            .map_err(|err| format!("the MCP session ended badly: {err}")),
        Err(err) => Err(format!("the MCP session would not start on stdio: {err}")),
    };
    follower.abort();
    notifier.abort();
    outcome
}
