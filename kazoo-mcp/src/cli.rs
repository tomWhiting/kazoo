//! The command line: how often the seat is told. It says nothing about
//! who the seat is or where the wall is: a seat joins a wall by name with
//! `wall_join` (KZ-N02). `--seat`, `KAZOO_SEAT` and `--socket` still join
//! at start, for one release, and say they are going.

use std::path::PathBuf;
use std::time::Duration;

use kazoo_wall::protocol::valid_name;

use crate::run::Args;

/// The environment variable naming the seat when `--seat` is not given
/// (going: see the module's words).
pub const SEAT_ENV: &str = "KAZOO_SEAT";

/// The notification window when `--notify-every` is not given, in seconds.
pub const DEFAULT_NOTIFY_EVERY: u64 = 20;

/// The longest notification window, in seconds: an hour.
pub const MAX_NOTIFY_EVERY: u64 = 3_600;

/// What `--help` prints.
pub const USAGE: &str = "\
kazoo-mcp: the wall for a Claude seat, an MCP server on stdio

usage: kazoo mcp [--notify-every <secs>]
       kazoo-mcp [--notify-every <secs>]

  --notify-every <secs>  news of other seats' changes comes at most once
                         per this many seconds (default 20, 1 to 3600)
  -h, --help             this text
  --version              the version

The seat starts on no wall and with no name: wall_walls lists the walls
it can reach, and wall_join {as} puts it on one as a name it chooses.

Going, and kept for one release:
  --seat <Name>          join this Mac's wall as this name at start
                         (or set KAZOO_SEAT)
  --socket <path>        the wall's control socket that join uses

It never starts the wall: run `kazoo wall` for that.
";

/// How this run was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invocation {
    /// Serve a seat.
    Serve(Args),
    /// Print the usage.
    Help,
    /// Print the version.
    Version,
}

/// A seat's settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// The name to join this Mac's wall as at start, already checked
    /// (`--seat` or `KAZOO_SEAT`, both going); `None` joins nothing.
    pub seat: Option<String>,
    /// The shortest time between two notifications of other seats' changes.
    pub notify_every: Duration,
    /// This Mac's wall's control socket.
    pub socket: PathBuf,
    /// What is going among the settings given: one sentence each, said on
    /// stderr as the seat starts.
    pub going: Vec<String>,
}

/// Read the command line (`args` without the program name). `env_seat` is
/// `$KAZOO_SEAT`. Defaults are applied when the seat starts.
///
/// # Errors
///
/// A sentence for the person at the terminal: an unknown flag, a flag with
/// no value, a seat name the wall would refuse, or a window out of range.
pub fn parse(
    args: impl IntoIterator<Item = String>,
    env_seat: Option<String>,
) -> Result<Invocation, String> {
    let args: Vec<String> = args.into_iter().collect();
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        return Ok(Invocation::Help);
    }
    let mut seat = None;
    let mut notify_every = None;
    let mut socket = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag.to_string(), Some(value)),
            _ => (arg.clone(), None),
        };
        match flag.as_str() {
            "--help" | "-h" => return Ok(Invocation::Help),
            "--version" | "-V" => return Ok(Invocation::Version),
            "--seat" => seat = Some(value(&flag, inline, &mut args)?),
            "--notify-every" => notify_every = Some(value(&flag, inline, &mut args)?),
            "--socket" => socket = Some(value(&flag, inline, &mut args)?),
            _ => {
                return Err(format!(
                    "unknown argument '{arg}'; run `kazoo-mcp --help` for the flags"
                ));
            }
        }
    }
    let seat = seat.or_else(|| env_seat.filter(|seat| !seat.is_empty()));
    if let Some(seat) = seat.as_deref()
        && !valid_name(seat)
    {
        return Err(format!(
            "'{}' is not a valid seat name: use 1 to 24 of A-Z a-z 0-9 space _ . -",
            seat.escape_default()
        ));
    }
    let notify_every = notify_every
        .map(|text| seconds(&text).map(Duration::from_secs))
        .transpose()?;
    if socket.as_deref().is_some_and(str::is_empty) {
        return Err("--socket needs a path".to_string());
    }
    Ok(Invocation::Serve(Args {
        seat,
        notify_every,
        socket: socket.map(PathBuf::from),
    }))
}

/// Apply defaults and validate settings supplied by another crate.
pub fn resolve(args: Args) -> Result<Options, String> {
    let env_seat = match std::env::var(SEAT_ENV) {
        Ok(seat) => Some(seat).filter(|seat| !seat.is_empty()),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(format!(
                "{SEAT_ENV} is not valid text: use 1 to 24 of A-Z a-z 0-9 space _ . -"
            ));
        }
    };
    resolve_with(args, env_seat.as_deref(), kazoo_wall::paths::socket_path)
}

/// [`resolve`], with `$KAZOO_SEAT` as `env_seat` (only to say it is going).
fn resolve_with(
    args: Args,
    env_seat: Option<&str>,
    default_socket: impl FnOnce() -> PathBuf,
) -> Result<Options, String> {
    if let Some(seat) = args.seat.as_deref()
        && !valid_name(seat)
    {
        return Err(format!(
            "'{}' is not a valid seat name: use 1 to 24 of A-Z a-z 0-9 space _ . -",
            seat.escape_default()
        ));
    }
    let notify_every = args
        .notify_every
        .map_or(Ok(DEFAULT_NOTIFY_EVERY), |window| {
            seconds(&window.as_secs_f64().to_string())
        })?;
    if args
        .socket
        .as_ref()
        .is_some_and(|path| path.as_os_str().is_empty())
    {
        return Err("--socket needs a path".to_owned());
    }
    let mut going = Vec::new();
    if env_seat.is_some() {
        going.push(format!(
            "{SEAT_ENV} is going: unset it, and the seat joins with wall_join"
        ));
    }
    if args.seat.is_some() && args.seat.as_deref() != env_seat {
        going.push("--seat is going: leave it out, and the seat joins with wall_join".to_string());
    }
    if args.socket.is_some() {
        going.push(
            "--socket is going: leave it out, and wall_walls finds this Mac's wall".to_string(),
        );
    }
    Ok(Options {
        seat: args.seat,
        notify_every: Duration::from_secs(notify_every),
        socket: args.socket.unwrap_or_else(default_socket),
        going,
    })
}

/// The value of `flag`: after its `=`, or the next argument.
fn value(
    flag: &str,
    inline: Option<&str>,
    rest: &mut impl Iterator<Item = String>,
) -> Result<String, String> {
    inline
        .map(str::to_string)
        .or_else(|| rest.next())
        .ok_or_else(|| format!("{flag} needs a value"))
}

/// A notification window in whole seconds, 1 to [`MAX_NOTIFY_EVERY`].
fn seconds(text: &str) -> Result<u64, String> {
    let why = || {
        format!(
            "--notify-every takes whole seconds from 1 to {MAX_NOTIFY_EVERY}, not '{}'",
            text.escape_default()
        )
    };
    let secs: u64 = text.trim().parse().map_err(|_| why())?;
    if (1..=MAX_NOTIFY_EVERY).contains(&secs) {
        Ok(secs)
    } else {
        Err(why())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str], env: Option<&str>) -> Result<Invocation, String> {
        parse(
            args.iter().map(|arg| (*arg).to_string()),
            env.map(str::to_string),
        )
    }

    fn serve(args: &[&str], env: Option<&str>) -> Options {
        match run(args, env) {
            Ok(Invocation::Serve(options)) => {
                resolve_with(options, env.filter(|seat| !seat.is_empty()), || {
                    PathBuf::from("/run/kazoo-wall.sock")
                })
                .unwrap()
            }
            other => panic!("not a seat: {other:?}"),
        }
    }

    #[test]
    fn a_seat_comes_from_the_flag_or_the_environment() {
        let options = serve(&["--seat", "Waffles"], None);
        assert_eq!(options.seat.as_deref(), Some("Waffles"));
        assert_eq!(options.notify_every, Duration::from_secs(20));
        assert_eq!(options.socket, PathBuf::from("/run/kazoo-wall.sock"));
        assert_eq!(serve(&[], Some("Vesper")).seat.as_deref(), Some("Vesper"));
        assert_eq!(
            serve(&["--seat=Tom"], Some("Vesper")).seat.as_deref(),
            Some("Tom")
        );
    }

    #[test]
    fn the_bare_command_names_nobody() {
        for env in [None, Some("")] {
            let options = serve(&[], env);
            assert_eq!(options.seat, None);
            assert_eq!(options.notify_every, Duration::from_secs(20));
        }
    }

    #[test]
    fn each_going_setting_is_said_on_its_own() {
        assert!(serve(&[], None).going.is_empty());
        assert_eq!(
            serve(&["--seat", "Tom"], None).going,
            ["--seat is going: leave it out, and the seat joins with wall_join"]
        );
        assert_eq!(
            serve(&[], Some("Vesper")).going,
            ["KAZOO_SEAT is going: unset it, and the seat joins with wall_join"]
        );
        assert_eq!(serve(&["--seat", "Tom"], Some("Vesper")).going.len(), 2);
        assert_eq!(
            serve(&["--socket", "/tmp/w.sock"], None).going,
            ["--socket is going: leave it out, and wall_walls finds this Mac's wall"]
        );
    }

    #[test]
    fn the_usage_leads_with_the_bare_command_and_names_the_going_flags() {
        assert!(USAGE.contains("usage: kazoo mcp [--notify-every <secs>]\n"));
        assert!(USAGE.contains("wall_join"));
        let going = USAGE.split_once("Going").expect("the going flags").1;
        assert!(going.contains("--seat") && going.contains("--socket"));
    }

    #[test]
    fn a_seat_the_wall_would_refuse_is_refused_here() {
        for bad in [
            "bad/name",
            "",
            "a name that is far too long for it",
            "tab\there",
        ] {
            let err = run(&["--seat", bad], None).unwrap_err();
            assert!(err.contains("valid seat name"), "{err}");
        }
    }

    #[test]
    fn the_window_and_the_socket_can_be_set() {
        let options = serve(
            &[
                "--seat",
                "Tom",
                "--notify-every",
                "3",
                "--socket",
                "/tmp/w.sock",
            ],
            None,
        );
        assert_eq!(options.notify_every, Duration::from_secs(3));
        assert_eq!(options.socket, PathBuf::from("/tmp/w.sock"));
        for bad in ["0", "-1", "1.5", "3601", "soon"] {
            let err = run(&["--seat", "Tom", "--notify-every", bad], None).unwrap_err();
            assert!(err.contains("whole seconds"), "{err}");
        }
        assert!(run(&["--seat", "Tom", "--socket", ""], None).is_err());
    }

    #[test]
    fn flags_need_values_and_unknown_ones_are_refused() {
        assert_eq!(run(&["--seat"], None).unwrap_err(), "--seat needs a value");
        assert!(
            run(&["--seat", "Tom", "--loud"], None)
                .unwrap_err()
                .contains("unknown argument")
        );
        assert_eq!(run(&["--help"], None), Ok(Invocation::Help));
        assert_eq!(run(&["--version"], None), Ok(Invocation::Version));
    }
}
