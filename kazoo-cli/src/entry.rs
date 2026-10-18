//! The program itself, shared by its three names, `kzoo`, `kazoo` and
//! `kzhz` (see `src/bin` and the kzhz crate).

use std::process::ExitCode;
use std::time::Duration;

use clap::{CommandFactory, FromArgMatches};

use crate::cli::{Cli, Command, TreeCommand, WallCommand, console_serve};

mod desk;

/// Run the program as `name` (`kzoo`, `kazoo` or `kzhz`: the name its help and
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
    if let Some(address) = cli.desk.as_deref()
        && let Err(error) = desk::apply(address)
    {
        eprintln!("{name}: cannot select desk {address}: {error}");
        return ExitCode::FAILURE;
    }
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
        Command::Tree(args) => finish(tree(name, &args)),
        Command::Setup(args) => finish(crate::setup::run(name, &args)),
    }
}

/// The exit code for `result`, with its report said under the program's
/// `name` when it failed (see [`kazoo_host::reports::finish`]).
fn finish_as(name: &str, result: color_eyre::Result<()>) -> ExitCode {
    kazoo_host::reports::finish(name, result)
}

fn tree(name: &str, args: &crate::cli::TreeArgs) -> color_eyre::Result<()> {
    let dir = kazoo_tree::tree_dir()?;
    let state = kazoo_wall::paths::state_dir()?;
    if plant(name, &state, &dir, &kazoo_wall::paths::socket_path())? == TreeIs::ForTheWall {
        return Err(color_eyre::eyre::eyre!(
            "the tree grows from what the wall has played, and the wall grows it as it starts: start it ({name} wall)"
        ));
    }
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

/// What is to be done about the tree, as [`assess_tree`] finds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TreeIs {
    /// A tree is there.
    There,
    /// No tree is there and no wall has played: one can be planted.
    ToPlant,
    /// No tree was there, and one has just been planted.
    Planted,
    /// The wall has played, and grows the tree from what it has played when
    /// it next starts.
    ForTheWall,
}

/// What is to be done about the tree at `dir`, for the wall whose state is
/// `state` and whose socket is `socket`. A wall that is playing is asked
/// about its tree: one playing without a tree, or still starting, is an
/// error saying what to do, as planting a tree under it would never be
/// grown.
pub(crate) fn assess_tree(
    name: &str,
    state: &std::path::Path,
    dir: &std::path::Path,
    socket: &std::path::Path,
) -> color_eyre::Result<TreeIs> {
    tree_is(name, state, dir, || {
        kazoo_wall::tree::plant::wall_tree(socket, Duration::from_secs(2))
    })
}

/// [`assess_tree`], with `ask` saying what the wall says about its tree. It
/// is asked only when there is no tree at `dir`.
pub(crate) fn tree_is(
    name: &str,
    state: &std::path::Path,
    dir: &std::path::Path,
    ask: impl FnOnce() -> kazoo_wall::tree::plant::WallTree,
) -> color_eyre::Result<TreeIs> {
    use color_eyre::eyre::eyre;
    use kazoo_wall::tree::plant::WallTree;
    if kazoo_wall::tree::git::is_bare(dir)
        .map_err(|err| eyre!("the tree at {} cannot be read ({err})", dir.display()))?
    {
        return Ok(TreeIs::There);
    }
    match ask() {
        WallTree::NoWall => {}
        WallTree::Planted => {
            return Err(eyre!(
                "the wall that is playing keeps its tree somewhere other than {}: run {name} tree with the KAZOO_TREE_DIR the wall was started with",
                dir.display()
            ));
        }
        WallTree::Starting => {
            return Err(eyre!(
                "a wall is starting, and plants the tree as it does: look again in a moment"
            ));
        }
        WallTree::Unplanted(why) => {
            let why = why.map_or_else(String::new, |why| format!(" ({why})"));
            return Err(eyre!(
                "the wall is playing without a tree{why}: stop it ({name} wall stop) and start it again ({name} wall), and it plants the tree as it starts"
            ));
        }
        WallTree::Unclear(why) => {
            return Err(eyre!(
                "a wall is there but cannot be asked about its tree ({why}): look again in a moment"
            ));
        }
    }
    if kazoo_wall::tree::plant::has_played(state).map_err(|why| eyre!("{why}"))? {
        Ok(TreeIs::ForTheWall)
    } else {
        Ok(TreeIs::ToPlant)
    }
}

/// Plant the tree at `dir` when [`assess_tree`] finds it is to be planted,
/// so it is there to look at before any wall has played. A tree that is
/// there is never touched, and one the wall grows from what it has played
/// is left to the wall. Says what it found.
pub(crate) fn plant(
    name: &str,
    state: &std::path::Path,
    dir: &std::path::Path,
    socket: &std::path::Path,
) -> color_eyre::Result<TreeIs> {
    use kazoo_wall::tree::plant::ToLook;
    match assess_tree(name, state, dir, socket)? {
        TreeIs::ToPlant => {}
        found => return Ok(found),
    }
    match kazoo_wall::tree::plant::plant_to_look(state, dir) {
        // Someone else planted it while this looked.
        ToLook::There => Ok(TreeIs::There),
        ToLook::Planted => Ok(TreeIs::Planted),
        ToLook::ForTheWall => Ok(TreeIs::ForTheWall),
        ToLook::Failed(why) => Err(color_eyre::eyre::eyre!("{why}")),
    }
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

#[cfg(test)]
mod tests;
