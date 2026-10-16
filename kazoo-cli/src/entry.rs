//! The program itself, shared by its two names (see `src/bin`).

use std::process::ExitCode;
use std::time::Duration;

use clap::{CommandFactory, FromArgMatches};

use crate::cli::{Cli, Command, TreeCommand, WallCommand, console_serve};

/// Run the program as `name` (`kzoo` or `kazoo`: the name its help and
/// errors use), with this process's arguments.
#[must_use]
pub fn main(name: &'static str) -> ExitCode {
    let matches = Cli::command().name(name).bin_name(name).get_matches();
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(err) => err
            .format(&mut Cli::command().name(name).bin_name(name))
            .exit(),
    };
    if let Err(err) = kazoo_host::reports::install() {
        eprintln!("{name}: {err}");
        return ExitCode::FAILURE;
    }
    let finish = |result| finish_as(name, result);
    match cli.command {
        Command::Mix(args) => finish(kazoo_mix::run(args.into())),
        Command::Studio { command } => match crate::studio::run(&command) {
            Ok(code) => code,
            Err(report) => {
                eprintln!("{name}: {report:#}");
                ExitCode::FAILURE
            }
        },
        Command::Wall { command } => finish(wall(name, command)),
        Command::Mcp(args) => finish(kazoo_mcp::run(args.into())),
        Command::EightOhEight => finish(kazoo_808::run(kazoo_808::Args)),
        Command::ThreeOhThree => finish(kazoo_303::run(kazoo_303::Args)),
        Command::Cs80 => finish(kazoo_cs80::run(kazoo_cs80::Args)),
        Command::Mini => finish(kazoo_mini::run(kazoo_mini::Args)),
        Command::Juno => finish(kazoo_juno::run(kazoo_juno::Args)),
        Command::Prophet => finish(kazoo_prophet::run(kazoo_prophet::Args)),
        Command::Dx(args) => finish(kazoo_dx::run(args.into())),
        Command::String(args) => finish(kazoo_string::run(args.into())),
        Command::Glass(args) => finish(kazoo_glass::run(args.into())),
        Command::Arp(args) => finish(kazoo_arp::run(args.into())),
        Command::Play(args) => match kazoo_play::run(args.into()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(kazoo_play::Error::Output(err)) if err.kind() == std::io::ErrorKind::BrokenPipe => {
                ExitCode::from(1)
            }
            Err(err) => {
                eprintln!("{name} play: {err}");
                ExitCode::from(err.exit_code())
            }
        },
        Command::Mouth(args) => finish(kazoo_mouth::run(args.into())),
        Command::Tui => finish(kazoo_tui::run(kazoo_tui::Args::default())),
        Command::Tree(args) => finish(tree(&args)),
    }
}

/// The exit code for `result`, with its report said under the program's
/// `name` when it failed.
fn finish_as(name: &str, result: color_eyre::Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(report) => {
            eprintln!("{name}: {report:?}");
            ExitCode::FAILURE
        }
    }
}

fn tree(args: &crate::cli::TreeArgs) -> color_eyre::Result<()> {
    let dir = kazoo_tree::tree_dir()?;
    if args.print {
        let text = kazoo_tree::print(dir, args.cols, args.rows)?;
        let mut out = std::io::stdout().lock();
        let written = std::io::Write::write_all(&mut out, text.as_bytes())
            .and_then(|()| std::io::Write::flush(&mut out));
        // A reader that stops early (`| head`) took all it wanted: done.
        let stopped_early = written
            .as_ref()
            .is_err_and(|err| err.kind() == std::io::ErrorKind::BrokenPipe);
        if !stopped_early {
            written?;
        }
    } else {
        kazoo_tree::run(dir)?;
    }
    Ok(())
}

fn wall(name: &str, command: Option<WallCommand>) -> color_eyre::Result<()> {
    match command {
        Some(WallCommand::Tree {
            command: TreeCommand::Import(args),
        }) => {
            let state = kazoo_wall::paths::state_dir()?;
            let tree = kazoo_wall::tree::git::tree_dir(&state).ok_or_else(|| {
                kazoo_wall::tree::rings::RingsError::NotATree("the wall's state directory has no parent to keep the tree beside; set KAZOO_TREE_DIR".into())
            })?;
            println!(
                "{}",
                kazoo_wall::tree::rings::grow(&state, &tree, args.dry_run)?
            );
            Ok(())
        }
        None | Some(WallCommand::Console) => {
            kazoo_wall::tui::run(console_serve(std::env::current_exe().map_err(|err| {
                color_eyre::eyre::eyre!("cannot find this program to start the wall with: {err}")
            })?))
        }
        Some(WallCommand::Serve(args)) => {
            kazoo_wall::daemon::serve(args.try_into()?)?;
            Ok(())
        }
        Some(WallCommand::Stop) => {
            kazoo_wall::daemon::stop_daemon(
                &kazoo_wall::paths::socket_path(),
                Duration::from_secs(10),
            )?;
            eprintln!("{name}: stopped");
            Ok(())
        }
    }
}
