use std::ffi::OsString;
use std::io::{self, Write};
use std::time::Duration;

use crate::daemon::{self, AudioMode, DaemonConfig, DeskMode, ServeCommand};
use crate::{paths, tui};

/// The operation requested on the command line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Args {
    /// Open the console, starting the daemon if it is not playing.
    #[default]
    Console,
    /// Run the daemon until it is stopped.
    Serve(ServeArgs),
    /// Ask the running daemon to stop and save its patch.
    Stop,
}

/// How the daemon renders and joins the desk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServeArgs {
    /// Play through the audio device; false renders on a timer.
    pub audio: bool,
    /// Join the desk whenever one is running.
    pub desk: bool,
    /// Requested sample rate; none uses the device's native rate, or 48000 Hz headless.
    pub rate: Option<u32>,
}

impl Default for ServeArgs {
    fn default() -> Self {
        Self {
            audio: true,
            desk: true,
            rate: None,
        }
    }
}

/// Help text for the standalone binary and the shared kazoo command.
pub const USAGE: &str = "kazoo-wall: the shared modular synth that never stops

usage: kazoo wall
       kazoo wall serve [--no-audio] [--no-desk] [--rate <hz>]
       kazoo wall stop
       kazoo-wall
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

const STOP_TIMEOUT: Duration = Duration::from_secs(10);
const RATE_ERROR: &str = "--rate takes a sample rate from 8000 to 384000";

/// Read arguments after the program name. `Ok(None)` requests help,
/// wherever `-h` or `--help` stands. Errors name the refused argument.
pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<Args>, String> {
    let args: Vec<String> = args.into_iter().collect();
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        return Ok(None);
    }
    let mut args = args.into_iter();
    let Some(command) = args.next() else {
        return Ok(Some(Args::Console));
    };
    match command.as_str() {
        "help" => Ok(None),
        "stop" => args.next().map_or(Ok(Some(Args::Stop)), |other| {
            Err(format!("unknown argument '{other}'"))
        }),
        "serve" => {
            let mut serve = ServeArgs::default();
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--no-audio" => serve.audio = false,
                    "--no-desk" => serve.desk = false,
                    "--rate" => match args.next().map(|value| value.parse::<u32>()) {
                        Some(Ok(rate)) if valid_rate(rate) => serve.rate = Some(rate),
                        _ => return Err(RATE_ERROR.to_owned()),
                    },
                    other => return Err(format!("unknown option '{other}'")),
                }
            }
            Ok(Some(Args::Serve(serve)))
        }
        other => Err(format!("unknown argument '{other}'")),
    }
}

const fn valid_rate(rate: u32) -> bool {
    rate >= 8_000 && rate <= 384_000
}

/// Print help or run the wall from command-line arguments. Help is flushed
/// before returning, and write failures are returned to the caller.
pub fn cli(args: impl IntoIterator<Item = String>) -> color_eyre::Result<()> {
    match parse_args(args) {
        Ok(None) => {
            write_usage(&mut io::stdout().lock())?;
            Ok(())
        }
        Ok(Some(args)) => run(args),
        Err(why) => Err(color_eyre::eyre::eyre!("{why}\n\n{USAGE}")),
    }
}

fn write_usage(output: &mut impl Write) -> io::Result<()> {
    output.write_all(USAGE.as_bytes())?;
    output.flush()
}

/// Run the console, serve the wall, or stop it. The caller installs error
/// reports (`color_eyre::install`) before calling this.
pub fn run(args: Args) -> color_eyre::Result<()> {
    match args {
        Args::Console => {
            let serve = ServeCommand {
                program: std::env::current_exe().map_err(|err| {
                    color_eyre::eyre::eyre!(
                        "cannot find this program to start the wall with: {err}"
                    )
                })?,
                args: vec![OsString::from("serve")],
            };
            tui::run(serve)
        }
        Args::Serve(args) => serve(args),
        Args::Stop => stop(),
    }
}

fn serve(args: ServeArgs) -> color_eyre::Result<()> {
    if args.rate.is_some_and(|rate| !valid_rate(rate)) {
        return Err(color_eyre::eyre::eyre!(RATE_ERROR));
    }
    let mut config = DaemonConfig::standard()?;
    config.desk = if args.desk {
        DeskMode::Discover
    } else {
        DeskMode::Off
    };
    config.audio = if args.audio {
        AudioMode::Device {
            sample_rate: args.rate,
        }
    } else {
        AudioMode::Headless {
            sample_rate: args.rate.unwrap_or(48_000),
        }
    };
    daemon::serve(config)?;
    Ok(())
}

fn stop() -> color_eyre::Result<()> {
    daemon::stop_daemon(&paths::socket_path(), STOP_TIMEOUT)?;
    eprintln!("kazoo-wall: stopped");
    Ok(())
}

#[cfg(test)]
mod tests;
