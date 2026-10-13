//! Named studio sessions start the desk before their instruments, each in a tmux pane.
//!
//! Write a session in `~/.kazoo/studio/demo.toml`, then run `kazoo studio up demo`:
//!
//! ```toml
//! [desk]
//! rate = 48000
//!
//! [[instrument]]
//! name = "808"
//!
//! [[instrument]]
//! name = "mini"
//!
//! [[instrument]]
//! name = "dx"
//! args = ["--phrase", "c4/8 e4/8 g4/8"]
//! ```
//!
//! `desk` and `args` are optional. The desk opens at its native device rate;
//! a supplied rate is verified during startup, and a mismatch aborts the session.
//! Detaching leaves the session running. `kazoo studio down demo` sends SIGTERM
//! to the desk before killing the instrument panes. Graceful link teardown
//! requires the desk to handle SIGTERM, which its current entry point does not.
//! Without tmux on PATH, `up` prints exact child commands and returns exit code 2.

use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use color_eyre::Result;
use color_eyre::eyre::{WrapErr, eyre};
use kazoo_mix::shared::DESK_CHANNELS;
use serde::Deserialize;

use crate::cli::{Cli, Command, StudioCommand};

mod tmux;

/// The desk and instruments named in a studio session file.
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StudioFile {
    /// Optional desk settings.
    #[serde(default)]
    pub desk: Desk,
    /// Instruments, in their pane creation order.
    #[serde(default)]
    pub instrument: Vec<Instrument>,
}

/// Settings required of the desk when it starts.
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Desk {
    /// Required sample rate; omitted accepts the desk's native device rate.
    pub rate: Option<u32>,
}

/// One instrument and its command-line arguments.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Instrument {
    /// The instrument's subcommand in the unified executable.
    pub name: String,
    /// Arguments passed verbatim to that instrument.
    #[serde(default)]
    pub args: Vec<String>,
}

impl StudioFile {
    /// Parse a session, rejecting unknown keys, invalid rates and invalid instrument commands.
    pub fn parse(contents: &str) -> Result<Self> {
        let session: Self = toml::from_str(contents)?;
        if session.instrument.len() > DESK_CHANNELS {
            return Err(eyre!(
                "{} instruments are listed but the desk has {DESK_CHANNELS} strips",
                session.instrument.len()
            ));
        }
        if let Some(rate) = session.desk.rate {
            if !(8_000..=384_000).contains(&rate) {
                return Err(eyre!("desk.rate must be between 8000 and 384000 Hz"));
            }
        }
        for instrument in &session.instrument {
            let words = std::iter::once("kazoo")
                .chain(std::iter::once(instrument.name.as_str()))
                .chain(instrument.args.iter().map(String::as_str));
            let parsed = Cli::try_parse_from(words)
                .wrap_err_with(|| format!("invalid instrument {:?}", instrument.name))?;
            if !matches!(
                parsed.command,
                Command::EightOhEight
                    | Command::ThreeOhThree
                    | Command::Cs80
                    | Command::Mini
                    | Command::Juno
                    | Command::Prophet
                    | Command::Dx(_)
                    | Command::String(_)
                    | Command::Glass(_)
                    | Command::Arp(_)
                    | Command::Play(_)
                    | Command::Mouth
            ) {
                return Err(eyre!("{:?} is not an instrument", instrument.name));
            }
            if instrument.args.iter().any(|arg| arg.contains('\0')) {
                return Err(eyre!("instrument arguments cannot contain NUL"));
            }
        }
        Ok(session)
    }
}

/// The session option holding the desk's PID. `up` sets it as it creates
/// the session; `down` stops only a session that carries it.
pub(crate) const DESK_OPTION: &str = "@kazoo_studio_desk";

/// A tmux invocation, kept as data so launch order is testable without running tmux.
#[derive(Debug, PartialEq, Eq)]
pub struct TmuxCommand {
    /// Arguments to the tmux executable, without shell interpretation.
    pub args: Vec<String>,
}

/// The children and ordered tmux operations for a session.
#[derive(Debug, PartialEq, Eq)]
pub struct LaunchPlan {
    /// The exact tmux session name.
    pub session: String,
    /// Shell-quoted commands for the desk followed by the instruments.
    pub children: Vec<String>,
    /// Desk creation, instrument splits and layouts, followed by attachment.
    pub commands: Vec<TmuxCommand>,
}

/// Build all launch commands using the same executable for every child.
///
/// Every pane runs with `XDG_RUNTIME_DIR` set to `runtime`, the directory
/// this process resolves the desk socket under. A pane otherwise inherits
/// the tmux server's environment, which can name a different directory and
/// so a different socket than the one `up` waits on.
pub fn commands(
    name: &str,
    file: &StudioFile,
    executable: &Path,
    runtime: &Path,
) -> Result<LaunchPlan> {
    validate_name(name)?;
    let executable = executable
        .to_str()
        .ok_or_else(|| eyre!("kazoo executable path is not valid UTF-8"))?;
    if !runtime.is_absolute() {
        return Err(eyre!(
            "XDG_RUNTIME_DIR {} is not an absolute path",
            runtime.display()
        ));
    }
    let environment = shell_command([format!(
        "XDG_RUNTIME_DIR={}",
        runtime
            .to_str()
            .ok_or_else(|| eyre!("XDG_RUNTIME_DIR is not valid UTF-8"))?
    )
    .as_str()])?;
    let pane = |child: &str| format!("exec /usr/bin/env {environment} {child}");
    let session = format!("kazoo-{name}");
    // `=` asks tmux for an exact session name, never a prefix. In a pane or
    // window target it needs the trailing colon: tmux 3.5a answers
    // split-window -t =kazoo-demo with "can't find pane: =kazoo-demo".
    let window = format!("={session}:");
    let target = format!("={session}");
    let mut children = vec![shell_command([executable, "mix"])?];
    for instrument in &file.instrument {
        children.push(shell_command(
            [executable, instrument.name.as_str()]
                .into_iter()
                .chain(instrument.args.iter().map(String::as_str)),
        )?);
    }
    let mut commands = vec![TmuxCommand {
        args: vec![
            "new-session".into(),
            "-d".into(),
            "-s".into(),
            session.clone(),
            "-P".into(),
            "-F".into(),
            "#{pane_pid}".into(),
            pane(&children[0]),
            // Same command list, so the session is never without its mark.
            ";".into(),
            "set-option".into(),
            "-F".into(),
            "-t".into(),
            window.clone(),
            DESK_OPTION.into(),
            "#{pane_pid}".into(),
            // A pane whose program fails stays open with what it printed,
            // so a desk or instrument that cannot start says why.
            ";".into(),
            "set-option".into(),
            "-w".into(),
            "-t".into(),
            window.clone(),
            "remain-on-exit".into(),
            "failed".into(),
        ],
    }];
    for child in children.iter().skip(1) {
        commands.push(TmuxCommand {
            args: vec![
                "split-window".into(),
                "-t".into(),
                window.clone(),
                pane(child),
            ],
        });
        commands.push(TmuxCommand {
            args: vec![
                "select-layout".into(),
                "-t".into(),
                window.clone(),
                "tiled".into(),
            ],
        });
    }
    commands.push(TmuxCommand {
        args: vec!["attach".into(), "-t".into(), target],
    });
    Ok(LaunchPlan {
        session,
        children,
        commands,
    })
}

fn shell_command<'a>(words: impl IntoIterator<Item = &'a str>) -> Result<String> {
    words
        .into_iter()
        .map(|word| {
            if word.contains('\0') {
                Err(eyre!("command words cannot contain NUL"))
            } else {
                Ok(format!("'{}'", word.replace('\'', "'\"'\"'")))
            }
        })
        .collect::<Result<Vec<_>>>()
        .map(|words| words.join(" "))
}

pub(crate) fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(eyre!(
            "studio name must be 1 to 64 ASCII letters, digits, '-' or '_'"
        ));
    }
    Ok(())
}

fn session_path(name: &str) -> Result<PathBuf> {
    validate_name(name)?;
    let home = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .ok_or_else(|| eyre!("HOME is missing; cannot find the studio session"))?;
    Ok(PathBuf::from(home)
        .join(".kazoo/studio")
        .join(format!("{name}.toml")))
}

/// The `XDG_RUNTIME_DIR` value under which this process finds the desk
/// socket: the variable itself, or `/tmp` when it is unset.
fn runtime_parent() -> Result<PathBuf> {
    let runtime = kazoo_core::ipc::discovery::runtime_dir();
    runtime
        .parent()
        .filter(|parent| parent.is_absolute())
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            eyre!(
                "the kazoo runtime directory {} is not under an absolute XDG_RUNTIME_DIR",
                runtime.display()
            )
        })
}

fn find_tmux(path: Option<&OsStr>) -> Result<Option<PathBuf>> {
    if let Some(path) = path {
        for directory in std::env::split_paths(path) {
            let candidate = directory.join("tmux");
            let metadata = match fs::metadata(&candidate) {
                Ok(metadata) => Some(metadata),
                // A PATH entry that is missing, is a file or cannot be
                // searched holds no tmux; execvp skips it the same way.
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound
                            | std::io::ErrorKind::NotADirectory
                            | std::io::ErrorKind::PermissionDenied
                    ) =>
                {
                    None
                }
                Err(error) => {
                    return Err(error)
                        .wrap_err_with(|| format!("checking {}", candidate.display()));
                }
            };
            if metadata.is_some_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            }) {
                return Ok(Some(std::path::absolute(candidate)?));
            }
        }
    }
    Ok(None)
}

/// Start or stop a named studio session. Missing tmux returns exit code two.
pub fn run(command: &StudioCommand) -> Result<ExitCode> {
    let path = std::env::var_os("PATH");
    let tmux = find_tmux(path.as_deref())?;
    match command {
        StudioCommand::Up { name } => {
            let session_path = session_path(name)?;
            let file = StudioFile::parse(
                &fs::read_to_string(&session_path)
                    .wrap_err_with(|| format!("reading {}", session_path.display()))?,
            )?;
            let plan = commands(name, &file, &std::env::current_exe()?, &runtime_parent()?)?;
            if let Some(tmux) = tmux {
                tmux::up(&tmux, &plan, file.desk.rate)?;
                Ok(ExitCode::SUCCESS)
            } else {
                let mut stdout = std::io::stdout().lock();
                for child in plan.children {
                    writeln!(stdout, "{child}")?;
                }
                stdout.flush()?;
                eprintln!(
                    "kazoo: tmux is required for studio up; run these commands in separate terminals"
                );
                Ok(ExitCode::from(2))
            }
        }
        StudioCommand::Down { name } => {
            validate_name(name)?;
            if let Some(tmux) = tmux {
                tmux::down(&tmux, &format!("kazoo-{name}"))?;
                Ok(ExitCode::SUCCESS)
            } else {
                eprintln!("kazoo: tmux is required for studio down");
                Ok(ExitCode::from(2))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[test]
    fn file_with_two_instruments_parses() {
        let file = StudioFile::parse(
            "[desk]\nrate = 48000\n[[instrument]]\nname = '808'\n[[instrument]]\nname = 'dx'\nargs = ['--phrase', 'c4/8']\n",
        ).unwrap();
        assert_eq!(file.desk.rate, Some(48_000));
        assert_eq!(file.instrument.len(), 2);
        assert_eq!(file.instrument[0].name, "808");
        assert!(file.instrument[0].args.is_empty());
        assert_eq!(file.instrument[1].args, ["--phrase", "c4/8"]);
        assert_eq!(StudioFile::parse("").unwrap(), StudioFile::default());
    }

    #[test]
    fn unknown_key_is_an_error_naming_it() {
        for (contents, key, line) in [
            ("typo = 1\n", "typo", 1),
            ("[desk]\nmisspelled_rate = 48000\n", "misspelled_rate", 2),
            ("[[instrument]]\nname = 'mini'\nargz = []\n", "argz", 3),
        ] {
            let error = StudioFile::parse(contents).unwrap_err().to_string();
            assert!(error.contains(key), "{error}");
            assert!(error.contains(&format!("line {line}")), "{error}");
        }
    }

    #[test]
    fn commands_are_built_in_order_desk_first() {
        let file =
            StudioFile::parse("[[instrument]]\nname = '808'\n[[instrument]]\nname = 'mini'\n")
                .unwrap();
        let plan = commands(
            "demo",
            &file,
            Path::new("/path with spaces/kazoo"),
            Path::new("/run/user/501"),
        )
        .unwrap();
        assert_eq!(
            plan.children,
            [
                "'/path with spaces/kazoo' 'mix'",
                "'/path with spaces/kazoo' '808'",
                "'/path with spaces/kazoo' 'mini'",
            ]
        );
        let expected: Vec<Vec<&str>> = vec![
            vec![
                "new-session",
                "-d",
                "-s",
                "kazoo-demo",
                "-P",
                "-F",
                "#{pane_pid}",
                "exec /usr/bin/env 'XDG_RUNTIME_DIR=/run/user/501' '/path with spaces/kazoo' 'mix'",
                ";",
                "set-option",
                "-F",
                "-t",
                "=kazoo-demo:",
                "@kazoo_studio_desk",
                "#{pane_pid}",
                ";",
                "set-option",
                "-w",
                "-t",
                "=kazoo-demo:",
                "remain-on-exit",
                "failed",
            ],
            vec![
                "split-window",
                "-t",
                "=kazoo-demo:",
                "exec /usr/bin/env 'XDG_RUNTIME_DIR=/run/user/501' '/path with spaces/kazoo' '808'",
            ],
            vec!["select-layout", "-t", "=kazoo-demo:", "tiled"],
            vec![
                "split-window",
                "-t",
                "=kazoo-demo:",
                "exec /usr/bin/env 'XDG_RUNTIME_DIR=/run/user/501' '/path with spaces/kazoo' 'mini'",
            ],
            vec!["select-layout", "-t", "=kazoo-demo:", "tiled"],
            vec!["attach", "-t", "=kazoo-demo"],
        ];
        assert_eq!(
            plan.commands
                .iter()
                .map(|command| command.args.iter().map(String::as_str).collect::<Vec<_>>())
                .collect::<Vec<_>>(),
            expected
        );
    }

    #[test]
    fn shell_metacharacters_remain_inside_one_argument() {
        let words = [
            "/path/kazoo",
            "dx",
            "--phrase",
            "it's $(printf injected); `printf x`",
        ];
        let command = shell_command(words).unwrap();
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &format!("set -- {command}; printf '%s\\n' \"$@\"")])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!("{}\n", words.join("\n"))
        );
        assert!(shell_command(["bad\0word"]).is_err());
    }

    #[test]
    fn invalid_session_names_cannot_select_paths_or_other_tmux_targets() {
        for name in ["", "../demo", "a.b", "a:b", "a/b", "x\ny", "é"] {
            assert!(validate_name(name).is_err(), "{name}");
        }
        assert!(validate_name(&"a".repeat(65)).is_err());
        assert!(validate_name("demo_2-live").is_ok());
    }

    #[test]
    fn invalid_rates_and_non_instrument_commands_are_rejected() {
        for contents in [
            "[desk]\nrate = 0\n",
            "[desk]\nrate = 384001\n",
            "[[instrument]]\nname = 'mix'\n",
            "[[instrument]]\nname = 'wall'\n",
            "[[instrument]]\nname = 'unknown'\n",
            "[[instrument]]\nname = '808'\nargs = ['--phrase', 'c4/8']\n",
            "[[instrument]]\nname = 'mini'\nargs = ['--help']\n",
        ] {
            assert!(StudioFile::parse(contents).is_err(), "{contents}");
        }
    }

    #[test]
    fn a_session_with_more_instruments_than_desk_strips_is_rejected() {
        let one = "[[instrument]]\nname = '808'\n";
        let full = one.repeat(kazoo_mix::shared::DESK_CHANNELS);
        assert_eq!(
            StudioFile::parse(&full).unwrap().instrument.len(),
            kazoo_mix::shared::DESK_CHANNELS
        );
        let error = StudioFile::parse(&format!("{full}{one}"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("strips"), "{error}");
    }

    #[test]
    fn path_entries_that_are_files_are_skipped_when_finding_tmux() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../target/kz-logs")
            .join(format!("find-tmux-{}", std::process::id()));
        fs::create_dir_all(directory.join("bin")).unwrap();
        fs::write(directory.join("file"), "").unwrap();
        fs::write(directory.join("bin/tmux"), "#!/bin/sh\n").unwrap();
        fs::set_permissions(directory.join("bin/tmux"), fs::Permissions::from_mode(0o755))
            .unwrap();
        let path = std::env::join_paths([directory.join("file"), directory.join("bin")]).unwrap();
        let found = find_tmux(Some(&path));
        fs::remove_dir_all(&directory).unwrap();
        assert_eq!(
            found.unwrap(),
            Some(std::path::absolute(directory.join("bin/tmux")).unwrap())
        );
    }

    #[test]
    fn studio_commands_parse_without_loading_session_files() {
        for operation in ["up", "down"] {
            let parsed = Cli::try_parse_from(["kazoo", "studio", operation, "demo"]).unwrap();
            let Command::Studio { command } = parsed.command else {
                panic!("expected studio");
            };
            match command {
                StudioCommand::Up { name } => {
                    assert_eq!(operation, "up");
                    assert_eq!(name, "demo");
                }
                StudioCommand::Down { name } => {
                    assert_eq!(operation, "down");
                    assert_eq!(name, "demo");
                }
            }
        }
        assert!(Cli::try_parse_from(["kazoo", "studio", "up"]).is_err());
    }
}
