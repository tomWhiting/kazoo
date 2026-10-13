use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;
use kazoo::cli::{Cli, Command, WallCommand, console_serve};

fn main() -> ExitCode {
    let cli = Cli::parse();
    // A04 replaces this with kazoo_host::reports::install.
    if let Err(err) = color_eyre::install() {
        eprintln!("kazoo: {err}");
        return ExitCode::FAILURE;
    }
    match cli.command {
        Command::Mix(args) => finish(kazoo_mix::run(args.into())),
        Command::Studio { command } => match kazoo::studio::run(&command) {
            Ok(code) => code,
            Err(report) => {
                eprintln!("kazoo: {report:#}");
                ExitCode::FAILURE
            }
        },
        Command::Wall { command } => finish(wall(command)),
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
                eprintln!("kazoo-play: {err}");
                ExitCode::from(err.exit_code())
            }
        },
        Command::Mouth => finish(kazoo_tui::run(kazoo_tui::Args)),
    }
}

fn finish(result: color_eyre::Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(report) => {
            eprintln!("kazoo: {report:?}");
            ExitCode::FAILURE
        }
    }
}

fn wall(command: Option<WallCommand>) -> color_eyre::Result<()> {
    match command {
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
            eprintln!("kazoo: stopped");
            Ok(())
        }
    }
}
