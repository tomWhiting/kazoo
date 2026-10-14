use std::io::Write;

use color_eyre::Result;
use kazoo_core::engine::EngineConfig;
use kazoo_core::io::StreamConfig;

use crate::app;

/// Microphone selection, or input enumeration without starting the engine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Args {
    pub input: Option<String>,
    pub input_channel: Option<u16>,
    pub list_inputs: bool,
}

/// Help text for both ways to start the application.
pub const USAGE: &str = "kazoo-tui: mouth synth, tracking views, mixing desk

usage: kazoo mouth
       kazoo-tui

  --input <name>       Choose an input device by name.
  --input-channel <n>  Choose one channel (1 to 64); otherwise average all.
  --list-inputs        List input devices and exit without opening a stream.

It plugs into the kazoo-mix desk whenever one is running, and plays on its
own otherwise. Keys are on the panel.
";

/// Read arguments after the program name. `Ok(None)` requests help,
/// wherever it stands; an error names the first unknown argument.
pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<Args>, String> {
    let args: Vec<String> = args.into_iter().collect();
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        return Ok(None);
    }
    let mut words = args.into_iter();
    let mut parsed = Args::default();
    while let Some(arg) = words.next() {
        match arg.as_str() {
            "--input" => {
                let name = words
                    .next()
                    .filter(|value| !value.starts_with("--"))
                    .ok_or_else(|| "--input needs a device name".to_owned())?;
                if name.is_empty() {
                    return Err("--input needs a device name".to_owned());
                }
                parsed.input = Some(name);
            }
            "--input-channel" => {
                let text = words
                    .next()
                    .ok_or_else(|| "--input-channel needs a number from 1 to 64".to_owned())?;
                let channel: u16 = text
                    .parse()
                    .map_err(|_| "--input-channel needs a number from 1 to 64".to_owned())?;
                if !(1..=64).contains(&channel) {
                    return Err("--input-channel needs a number from 1 to 64".to_owned());
                }
                parsed.input_channel = Some(channel);
            }
            "--list-inputs" => parsed.list_inputs = true,
            _ => return Err(format!("unknown argument '{arg}'")),
        }
    }
    Ok(Some(parsed))
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
    if args.list_inputs {
        let devices = kazoo_core::io::enumerate_input_devices()?;
        let mut stdout = std::io::stdout().lock();
        for device in devices {
            let default = if device.is_default { "  (default)" } else { "" };
            let rates = device
                .supported_sample_rates
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(
                stdout,
                "{}{default}  up to {} channels  rates: {rates}",
                device.name, device.max_input_channels
            )?;
        }
        stdout.flush()?;
        return Ok(());
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(main_loop(args))
}

/// The name this synth plugs into the kazoo-mix desk under.
const DESK_NAME: &str = "kazoo-tui";

async fn main_loop(args: Args) -> Result<()> {
    #[cfg(test)]
    if tests::STAND_IN.get() {
        return tests::stand_in(args).await;
    }

    // Start the audio engine *before* entering the alternate screen so that
    // initialisation errors (missing audio device, etc.) are printed in the
    // user's normal terminal. The engine plugs into the kazoo-mix desk (the
    // studio's hub) whenever one is running, and plays standalone otherwise.
    let engine = kazoo_core::engine::start(EngineConfig {
        stream: StreamConfig {
            input_device: args.input,
            input_channel: args.input_channel,
            ..StreamConfig::default()
        },
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

    pub(super) async fn stand_in(args: Args) -> color_eyre::Result<()> {
        assert_eq!(args, Args::default());
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
        assert_eq!(parse_args([]), Ok(Some(Args::default())));
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
        run(Args::default())?;
        run(Args::default())?;
        STAND_IN.set(false);
        Ok(())
    }

    #[test]
    fn mic_arguments_choose_the_device_and_channel() {
        assert_eq!(
            parse_args(["--input", "AirPods Pro", "--input-channel", "2"].map(String::from)),
            Ok(Some(Args {
                input: Some("AirPods Pro".into()),
                input_channel: Some(2),
                list_inputs: false
            }))
        );
    }

    #[test]
    fn invalid_mic_arguments_are_refused_by_name() {
        for args in [
            vec!["--input"],
            vec!["--input", "--list-inputs"],
            vec!["--input-channel", "0"],
            vec!["--input-channel", "x"],
            vec!["--input-channel", "65"],
        ] {
            let flag = args[0];
            let error = parse_args(args.into_iter().map(String::from)).unwrap_err();
            assert!(error.contains(flag), "{error}");
        }
    }

    #[test]
    fn list_inputs_is_an_argument_and_empty_arguments_use_defaults() {
        assert_eq!(
            parse_args(["--list-inputs".into()]),
            Ok(Some(Args {
                list_inputs: true,
                ..Args::default()
            }))
        );
        assert_eq!(parse_args([]), Ok(Some(Args::default())));
        for flag in ["--input", "--input-channel", "--list-inputs"] {
            assert!(USAGE.contains(flag));
        }
    }

    #[test]
    fn help_wins_over_invalid_mic_arguments() {
        for args in [
            vec!["--input", "--help"],
            vec!["--input-channel", "x", "-h"],
        ] {
            assert_eq!(parse_args(args.into_iter().map(String::from)), Ok(None));
        }
    }
}
