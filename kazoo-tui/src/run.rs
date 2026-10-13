use std::io::Write;

use color_eyre::Result;
use kazoo_core::engine::EngineConfig;

use crate::app;

/// What the command line gave: this application takes no arguments.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Args;

/// Help text for both ways to start the application.
pub const USAGE: &str = "kazoo-tui: mouth synth, tracking views, mixing desk

usage: kazoo mouth
       kazoo-tui

It plugs into the kazoo-mix desk whenever one is running, and plays on its
own otherwise. Keys are on the panel.
";

/// Read arguments after the program name. `Ok(None)` requests help,
/// wherever it stands; an error names the first unknown argument.
pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<Args>, String> {
    let mut unknown = None;
    for arg in args {
        if matches!(arg.as_str(), "-h" | "--help") {
            return Ok(None);
        }
        if unknown.is_none() {
            unknown = Some(arg);
        }
    }
    unknown.map_or(Ok(Some(Args)), |arg| {
        Err(format!("unknown argument '{arg}'"))
    })
}

/// Print help or start the application from command-line arguments.
///
/// # Errors
///
/// Returns argument, output, runtime, audio, or terminal errors.
pub fn cli(args: impl IntoIterator<Item = String>) -> Result<()> {
    match parse_args(args) {
        Ok(None) => {
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(USAGE.as_bytes())?;
            stdout.flush()?;
            Ok(())
        }
        Err(why) => Err(color_eyre::eyre::eyre!("{why}\n\n{USAGE}")),
        Ok(Some(args)) => run(args),
    }
}

/// Run the application with its own runtime until it quits.
///
/// The caller installs error reports once with `kazoo_host::reports::install`.
/// This drops the runtime before returning, permitting subsequent instruments.
///
/// # Errors
///
/// Returns runtime, audio, or terminal errors.
pub fn run(args: Args) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(main_loop(args))
}

/// The name this synth plugs into the kazoo-mix desk under.
const DESK_NAME: &str = "kazoo-tui";

async fn main_loop(Args: Args) -> Result<()> {
    #[cfg(test)]
    if tests::STAND_IN.get() {
        return tests::stand_in(Args).await;
    }

    // Start the audio engine *before* entering the alternate screen so that
    // initialisation errors (missing audio device, etc.) are printed in the
    // user's normal terminal. The engine plugs into the kazoo-mix desk (the
    // studio's hub) whenever one is running, and plays standalone otherwise.
    let engine = kazoo_core::engine::start(EngineConfig {
        desk_instrument_name: Some(DESK_NAME.to_owned()),
        ..EngineConfig::default()
    })?;

    // Enumerate audio devices for the Audio I/O view. Done here, not inside
    // `App`, so constructing an `App` never touches audio hardware.
    let devices = app::AudioDevices::enumerate();

    // The header shows the desk link's state, including why it could not
    // start if so; stderr would be hidden by the alternate screen.
    let mut app = app::App::new(engine, devices);

    // Enter the alternate screen and enable raw mode.
    let mut terminal = kazoo_host::terminal::enter()?;

    // Run the application event loop.
    let result = app
        .run(&mut terminal)
        .await
        .map_err(color_eyre::Report::from);

    // Always restore the terminal, even if the event loop errored.
    let restored = terminal.leave();
    match (result, restored) {
        (Ok(()), restored) => restored.map_err(Into::into),
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(restore_error)) => Err(error.wrap_err(format!(
            "restoring the terminal also failed: {restore_error}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::{Args, USAGE, parse_args, run};

    thread_local! {
        pub(super) static STAND_IN: Cell<bool> = const { Cell::new(false) };
    }

    pub(super) async fn stand_in(Args: Args) -> color_eyre::Result<()> {
        tokio::spawn(async { tokio::runtime::Handle::current() }).await?;
        Ok(())
    }

    #[test]
    fn help_asks_for_the_usage() {
        for args in [
            vec!["-h"],
            vec!["--help"],
            vec!["--bogus", "--help"],
            vec!["-h", "--bogus"],
        ] {
            assert_eq!(parse_args(args.into_iter().map(String::from)), Ok(None));
        }
        assert_eq!(parse_args([]), Ok(Some(Args)));
    }

    #[test]
    fn an_unknown_argument_is_refused_by_name() {
        assert_eq!(
            parse_args(["--bogus", "--other"].map(String::from)),
            Err("unknown argument '--bogus'".to_owned())
        );
    }

    #[test]
    fn usage_names_both_ways_to_start_it() {
        assert!(USAGE.contains("kazoo mouth"));
        assert!(USAGE.contains("kazoo-tui"));
        assert!(USAGE.starts_with("kazoo-tui: "));
    }

    #[test]
    fn usage_opens_with_the_name_and_description() {
        assert!(USAGE.starts_with("kazoo-tui: mouth synth, tracking views, mixing desk\n"));
    }

    #[test]
    fn run_builds_its_own_runtime() -> color_eyre::Result<()> {
        STAND_IN.set(true);
        run(Args)?;
        run(Args)?;
        STAND_IN.set(false);
        Ok(())
    }
}
