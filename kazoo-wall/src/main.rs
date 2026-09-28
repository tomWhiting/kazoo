//! `kazoo-wall`: the shared modular synth that never stops.
//!
//! - `kazoo-wall serve [--no-audio] [--no-desk]` runs the daemon until
//!   SIGINT, SIGTERM or a console's `shutdown`.
//! - `kazoo-wall stop` asks the running daemon to stop.
//! - `kazoo-wall` with no command opens the console (the TUI), starting a
//!   detached daemon first if none is playing.
//!
//! The socket and state directories follow `KAZOO_WALL_RUNTIME_DIR` and
//! `KAZOO_WALL_STATE_DIR` when set.

use std::process::ExitCode;
use std::time::Duration;

use kazoo_wall::daemon::{self, AudioMode, DaemonConfig, DeskMode};
use kazoo_wall::paths;

mod tui;

const USAGE: &str = "\
usage: kazoo-wall
       kazoo-wall serve [--no-audio] [--no-desk] [--rate <hz>]
       kazoo-wall stop

  (none)      open the console, starting the wall if it is not playing
  serve       run the wall until stopped (Ctrl-C, SIGTERM, or a console)
    --no-audio  render on a timer with no audio device (the desk still
                hears the wall when it is plugged in)
    --no-desk   never join the kazoo-mix desk
    --rate      sample rate: with a device, switch it to this rate (by
                default the wall plays at the rate the device is set to);
                with --no-audio, the rate to render at (default 48000)
  stop        ask the running wall to stop

environment:
  KAZOO_WALL_RUNTIME_DIR  directory for the control socket
  KAZOO_WALL_STATE_DIR    directory for the patch and the change log
";

/// How long `stop` waits for the daemon to go.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => tui::run(),
        Some("serve") => serve(&args[1..]),
        Some("stop") if args.len() == 1 => stop(),
        Some("help" | "--help" | "-h") => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => {
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn serve(args: &[String]) -> ExitCode {
    let mut config = match DaemonConfig::standard() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("kazoo-wall: {err}");
            return ExitCode::FAILURE;
        }
    };
    let mut headless = false;
    let mut rate = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--no-audio" => headless = true,
            "--no-desk" => config.desk = DeskMode::Off,
            "--rate" => match rest.next().map(|value| value.parse::<u32>()) {
                Some(Ok(value)) if (8_000..=384_000).contains(&value) => rate = Some(value),
                _ => {
                    eprintln!("kazoo-wall: --rate takes a sample rate from 8000 to 384000");
                    return ExitCode::from(2);
                }
            },
            other => {
                eprintln!("kazoo-wall: unknown option '{other}'\n");
                eprint!("{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    config.audio = if headless {
        AudioMode::Headless {
            sample_rate: rate.unwrap_or(48_000),
        }
    } else {
        AudioMode::Device { sample_rate: rate }
    };
    match daemon::serve(config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("kazoo-wall: {err}");
            ExitCode::FAILURE
        }
    }
}

fn stop() -> ExitCode {
    let socket = paths::socket_path();
    match daemon::stop_daemon(&socket, STOP_TIMEOUT) {
        Ok(()) => {
            eprintln!("kazoo-wall: stopped");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("kazoo-wall: {err}");
            ExitCode::FAILURE
        }
    }
}
