use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rmcp::ServiceExt as _;
use tokio::sync::watch;

use crate::cli::{self, Invocation, Options};
use crate::link;
use crate::tools::Seat;
use crate::walls::{HERE, Walls};

/// A seat's settings, with defaults applied when it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    /// The name to join this Mac's wall as at start (`--seat`, going);
    /// `None` starts on no wall, for `wall_join`.
    pub seat: Option<String>,
    /// The notification window in whole seconds, 1 to 3600; `None` uses 20.
    pub notify_every: Option<Duration>,
    /// This Mac's wall's control socket (`--socket`, going); `None` uses
    /// its runtime directory.
    pub socket: Option<PathBuf>,
}

/// Help text for both ways to start the seat.
pub const USAGE: &str = cli::USAGE;

/// Read arguments after the program name, using `KAZOO_SEAT` when needed.
/// `Ok(None)` requests help or version output. Help wins wherever it stands.
///
/// # Errors
///
/// Returns the existing command-line error words for invalid settings.
pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<Args>, String> {
    parse_invocation(args).map(|invocation| match invocation {
        Invocation::Serve(args) => Some(args),
        Invocation::Help | Invocation::Version => None,
    })
}

fn parse_invocation(args: impl IntoIterator<Item = String>) -> Result<Invocation, String> {
    let args: Vec<String> = args.into_iter().collect();
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        return Ok(Invocation::Help);
    }
    let env_seat = match std::env::var(cli::SEAT_ENV) {
        Ok(seat) => Some(seat),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(format!(
                "{} is not valid text: use 1 to 24 of A-Z a-z 0-9 space _ . -",
                cli::SEAT_ENV
            ));
        }
    };
    cli::parse(args, env_seat)
}

/// Print help or version output, or serve a seat from command-line arguments.
///
/// The caller installs error reports once with `kazoo_host::reports::install`.
///
/// # Errors
///
/// Returns argument, output, runtime, or MCP session errors. Argument errors
/// carry `std::io::ErrorKind::InvalidInput` for the standalone exit status.
pub fn cli(args: impl IntoIterator<Item = String>) -> color_eyre::Result<()> {
    match parse_invocation(args) {
        Ok(Invocation::Help) => {
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(USAGE.as_bytes())?;
            stdout.flush()?;
            Ok(())
        }
        Ok(Invocation::Version) => {
            let mut stdout = std::io::stdout().lock();
            writeln!(stdout, "{}", link::CLIENT)?;
            stdout.flush()?;
            Ok(())
        }
        Ok(Invocation::Serve(args)) => run(args),
        Err(why) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{why}\n\n{USAGE}"),
        )
        .into()),
    }
}

/// Serve stdio until EOF with a runtime owned and dropped by this call.
///
/// The caller installs error reports once with `kazoo_host::reports::install`.
/// This never installs them, allowing several instruments in one process.
///
/// # Errors
///
/// Returns invalid settings, runtime construction, or MCP session errors.
pub fn run(args: Args) -> color_eyre::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(main_loop(args))
}

async fn main_loop(args: Args) -> color_eyre::Result<()> {
    #[cfg(test)]
    if tests::STAND_IN.get() {
        return tests::stand_in(args).await;
    }

    let Options {
        seat,
        notify_every,
        socket,
    } = cli::resolve(args).map_err(color_eyre::eyre::Error::msg)?;
    eprintln!(
        "kazoo-mcp: this Mac's wall at {}, news at most every {} seconds",
        socket.display(),
        notify_every.as_secs()
    );
    let (peer, session) = watch::channel(None);
    let walls = Arc::new(Walls::new(socket, notify_every, session));
    if let Some(seat) = seat {
        eprintln!(
            "kazoo-mcp: --seat and {} are going: leave them out, and the seat joins with wall_join",
            cli::SEAT_ENV
        );
        walls
            .join(None, &seat)
            .await
            .map_err(color_eyre::eyre::Error::msg)?;
        eprintln!("kazoo-mcp: on {HERE} as {seat}");
    }
    let server = Seat::new(Arc::clone(&walls), peer);
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
    // Every joined wall's event line and notifier stop with the seat.
    drop(walls);
    outcome.map_err(color_eyre::eyre::Error::msg)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::path::PathBuf;
    use std::time::Duration;

    use super::{Args, USAGE, parse_args, run};

    thread_local! {
        pub(super) static STAND_IN: Cell<bool> = const { Cell::new(false) };
    }

    pub(super) async fn stand_in(args: Args) -> color_eyre::Result<()> {
        assert_eq!(args.seat.as_deref(), Some("Cassio"));
        tokio::spawn(async { tokio::runtime::Handle::current() }).await?;
        Ok(())
    }

    #[test]
    fn help_asks_for_the_usage() {
        for args in [
            vec!["-h"],
            vec!["--help"],
            vec!["--bogus", "--help"],
            vec!["--seat", "-h"],
            vec!["--notify-every", "-h"],
            vec!["--socket", "-h"],
        ] {
            assert_eq!(parse_args(args.into_iter().map(String::from)), Ok(None));
        }
    }

    #[test]
    fn an_unknown_argument_is_refused_by_name() {
        assert!(parse_args(["--bogus"].map(String::from)).is_err_and(
            |why| why == "unknown argument '--bogus'; run `kazoo-mcp --help` for the flags"
        ));
    }

    #[test]
    fn usage_names_both_ways_to_start_it() {
        assert!(USAGE.contains("kazoo mcp"));
        assert!(USAGE.contains("kazoo-mcp"));
        assert!(USAGE.starts_with("kazoo-mcp: "));
    }

    #[test]
    fn seat_and_socket_are_parsed_as_today() {
        assert_eq!(
            parse_args(
                [
                    "--seat",
                    "Cassio",
                    "--notify-every",
                    "3",
                    "--socket",
                    "/tmp/w.sock"
                ]
                .map(String::from)
            ),
            Ok(Some(Args {
                seat: Some("Cassio".to_owned()),
                notify_every: Some(Duration::from_secs(3)),
                socket: Some(PathBuf::from("/tmp/w.sock"))
            }))
        );
        assert_eq!(
            parse_args(["--seat=Cassio"].map(String::from)),
            Ok(Some(Args {
                seat: Some("Cassio".to_owned()),
                notify_every: None,
                socket: None
            }))
        );
    }

    #[test]
    fn usage_opens_with_the_name_and_description() {
        assert!(
            USAGE.starts_with("kazoo-mcp: the wall for a Claude seat, an MCP server on stdio\n")
        );
    }

    #[test]
    fn run_builds_its_own_runtime() -> color_eyre::Result<()> {
        STAND_IN.set(true);
        let args = Args {
            seat: Some("Cassio".to_owned()),
            notify_every: None,
            socket: None,
        };
        run(args.clone())?;
        run(args)?;
        STAND_IN.set(false);
        Ok(())
    }
}
