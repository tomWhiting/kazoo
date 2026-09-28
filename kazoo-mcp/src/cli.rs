//! The command line: who this seat is, how often it is told, and where the
//! wall's socket is.

use std::path::PathBuf;
use std::time::Duration;

use kazoo_wall::protocol::valid_name;

/// The environment variable naming the seat when `--seat` is not given.
pub const SEAT_ENV: &str = "KAZOO_SEAT";

/// The notification window when `--notify-every` is not given, in seconds.
pub const DEFAULT_NOTIFY_EVERY: u64 = 20;

/// The longest notification window, in seconds: an hour.
pub const MAX_NOTIFY_EVERY: u64 = 3_600;

/// What `--help` prints.
pub const USAGE: &str = "\
kazoo-mcp: the wall for a Claude seat (an MCP server on stdio)

usage: kazoo-mcp --seat <Name> [--notify-every <secs>] [--socket <path>]

  --seat <Name>          who this seat is on the wall: 1 to 24 of
                         A-Z a-z 0-9 space _ . - (or set KAZOO_SEAT)
  --notify-every <secs>  news of other seats' changes comes at most once
                         per this many seconds (default 20, 1 to 3600)
  --socket <path>        the wall's control socket (default: kazoo-wall.sock
                         in kazoo's runtime directory)
  --help                 this text
  --version              the version

It never starts the wall: run `kazoo-wall` for that.";

/// How this run was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invocation {
    /// Serve a seat.
    Serve(Options),
    /// Print the usage.
    Help,
    /// Print the version.
    Version,
}

/// A seat's settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// The seat's name, already checked.
    pub seat: String,
    /// The shortest time between two notifications of other seats' changes.
    pub notify_every: Duration,
    /// The wall's control socket.
    pub socket: PathBuf,
}

/// Read the command line (`args` without the program name). `env_seat` is
/// `$KAZOO_SEAT`, and `default_socket` the socket used when none is named.
///
/// # Errors
///
/// A sentence for the person at the terminal: an unknown flag, a flag with
/// no value, no seat, a seat name the wall would refuse, or a window out of
/// range.
pub fn parse(
    args: impl IntoIterator<Item = String>,
    env_seat: Option<String>,
    default_socket: impl FnOnce() -> PathBuf,
) -> Result<Invocation, String> {
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
    let seat = seat
        .or_else(|| env_seat.filter(|seat| !seat.is_empty()))
        .ok_or_else(|| {
            format!(
                "kazoo-mcp needs to know which seat it is: run it as `kazoo-mcp --seat <Name>` \
                 or set {SEAT_ENV}"
            )
        })?;
    if !valid_name(&seat) {
        return Err(format!(
            "'{}' is not a valid seat name: use 1 to 24 of A-Z a-z 0-9 space _ . -",
            seat.escape_default()
        ));
    }
    let notify_every = notify_every.map_or(Ok(DEFAULT_NOTIFY_EVERY), |text| seconds(&text))?;
    if socket.as_deref().is_some_and(str::is_empty) {
        return Err("--socket needs a path".to_string());
    }
    Ok(Invocation::Serve(Options {
        seat,
        notify_every: Duration::from_secs(notify_every),
        socket: socket.map_or_else(default_socket, PathBuf::from),
    }))
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
            || PathBuf::from("/run/kazoo-wall.sock"),
        )
    }

    fn serve(args: &[&str], env: Option<&str>) -> Options {
        match run(args, env) {
            Ok(Invocation::Serve(options)) => options,
            other => panic!("not a seat: {other:?}"),
        }
    }

    #[test]
    fn a_seat_comes_from_the_flag_or_the_environment() {
        let options = serve(&["--seat", "Waffles"], None);
        assert_eq!(options.seat, "Waffles");
        assert_eq!(options.notify_every, Duration::from_secs(20));
        assert_eq!(options.socket, PathBuf::from("/run/kazoo-wall.sock"));
        assert_eq!(serve(&[], Some("Vesper")).seat, "Vesper");
        assert_eq!(serve(&["--seat=Tom"], Some("Vesper")).seat, "Tom");
    }

    #[test]
    fn no_seat_says_how_to_set_one() {
        for env in [None, Some("")] {
            let err = run(&[], env).unwrap_err();
            assert!(err.contains("--seat <Name>"), "{err}");
            assert!(err.contains(SEAT_ENV), "{err}");
        }
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
            assert!(
                err.contains("valid seat name") || err.contains("needs to know"),
                "{err}"
            );
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
