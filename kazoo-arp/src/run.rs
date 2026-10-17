use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event};
use kazoo_core::host::audio::{OpenedOutput, OutputRequest};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ringbuf::traits::Split;

use crate::app::{App, KeyMode, StreamHealth};
use crate::audio::{
    AudioEngine, COMMAND_CAPACITY, CommandSender, DISPLAY_CAPACITY, EngineShared,
    MAX_CALLBACK_FRAMES,
};
use kazoo_core::host::link::{LinkSetup, connect_or_direct};
use kazoo_core::ipc::link::{HubLink, RequestError};

use crate::ui;

/// Destination and audition settings for the standalone arpeggiator.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Args {
    pub target: [u8; 16],
    pub silent: bool,
}

/// What `--help` prints.
pub const USAGE: &str = "kazoo-arp: Jupiter-8 style arpeggiator, note scheduler for any synth

usage: kazoo arp [--target all|UUID] [--silent]
       kazoo-arp [--target all|UUID] [--silent]

It plugs into the kazoo-mix desk whenever one is running, and plays on its
own otherwise. Keys are on the panel.
Notes drive every other instrument by default. --target selects a registered
instrument UUID; this desk protocol cannot resolve instrument names.
--silent mutes the audition voice while continuing to drive the desk.
";

/// Read the arguments after the program name. `Ok(None)` means help was
/// asked for; `Err` is one sentence saying what was wrong (the caller adds
/// the usage).
pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<Args>, String> {
    let args: Vec<_> = args.into_iter().collect();
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        return Ok(None);
    }
    let mut options = Args::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--target" => {
                let target = args
                    .next()
                    .ok_or("--target requires all or an instrument UUID")?;
                options.target = parse_target(&target)?;
            }
            "--silent" => options.silent = true,
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    Ok(Some(options))
}

/// Read a `--target`: `all` (every instrument) or a 32-digit instrument UUID,
/// with or without hyphens. The error is one sentence naming the refused value.
///
/// # Errors
///
/// Refuses anything else, including the all-zero UUID.
pub fn parse_target(target: &str) -> Result<[u8; 16], String> {
    if target == "all" {
        return Ok([0; 16]);
    }
    let malformed = || {
        format!(
            "invalid target '{target}': use all or a 32-digit instrument UUID; the desk cannot resolve names"
        )
    };
    if target.len() != 32 && target.len() != 36 {
        return Err(malformed());
    }
    let mut digits = target.bytes().enumerate().filter_map(|(index, byte)| {
        if target.len() == 36 && matches!(index, 8 | 13 | 18 | 23) {
            (byte != b'-').then_some(byte)
        } else {
            Some(byte)
        }
    });
    let mut destination = [0; 16];
    for byte in &mut destination {
        let high = digits
            .next()
            .and_then(|digit| char::from(digit).to_digit(16))
            .ok_or_else(&malformed)?;
        let low = digits
            .next()
            .and_then(|digit| char::from(digit).to_digit(16))
            .ok_or_else(&malformed)?;
        *byte = ((high << 4) | low) as u8;
    }
    if digits.next().is_some() || destination == [0; 16] {
        return Err(malformed());
    }
    Ok(destination)
}

/// The standalone binary's entry: parse, print the usage on help, run.
pub fn cli(args: impl IntoIterator<Item = String>) -> color_eyre::Result<()> {
    match parse_args(args) {
        Ok(None) => {
            let mut stdout = io::stdout().lock();
            stdout.write_all(USAGE.as_bytes())?;
            stdout.flush()?;
            Ok(())
        }
        Err(why) => Err(color_eyre::eyre::eyre!("{why}\n\n{USAGE}")),
        Ok(Some(args)) => run(args),
    }
}

/// Target frame rate for the TUI.
const FPS: u64 = 30;

type Tui = Terminal<CrosstermBackend<io::Stdout>>;

/// Run the instrument until it quits: audio, desk link, terminal, event loop.
///
/// Error reports must already be installed by the caller
/// (`kazoo_host::reports::install`); this never installs them, so one process can
/// run several instruments one after another.
pub fn run(args: Args) -> color_eyre::Result<()> {
    // -----------------------------------------------------------------------
    // Audio setup
    // -----------------------------------------------------------------------

    let request = OutputRequest::default();
    let prepared = request.prepare_output()?;
    let output_info = prepared.info();
    let sample_rate = output_info.rate;
    let channels = usize::from(output_info.channels);

    let (cmd_tx, cmd_rx) = crossbeam_channel::bounded(COMMAND_CAPACITY);
    // Display events: Audio -> UI (lock-free SPSC).
    let (display_prod, display_cons) = ringbuf::HeapRb::new(DISPLAY_CAPACITY).split();
    let shared = Arc::new(EngineShared::default());

    // Plug into the kazoo-mix desk whenever it is running.
    let (hub, hub_audio) = connect_or_direct(&LinkSetup {
        name: "kazoo-arp",
        channels: 2,
        rate: sample_rate,
        block: MAX_CALLBACK_FRAMES,
    })?;

    // Build audio stream with arp engine + audition voice.
    let mut engine = AudioEngine::new(
        sample_rate,
        channels,
        cmd_rx,
        hub_audio,
        display_prod,
        Arc::clone(&shared),
    );
    engine.configure_output(args);
    let stream = prepared.open(move |data, _| engine.render(data))?;

    // -----------------------------------------------------------------------
    // Terminal setup
    // -----------------------------------------------------------------------

    let mut terminal = kazoo_host::terminal::enter_with(kazoo_host::terminal::Options {
        mouse: false,
        key_release_events: true,
        panics: kazoo_host::terminal::Panics::AnyThread,
    })?;
    let (key_mode, key_mode_reason) = match terminal.key_releases() {
        kazoo_host::terminal::KeyReleases::Reported => (KeyMode::Hold, None),
        kazoo_host::terminal::KeyReleases::NotAsked
        | kazoo_host::terminal::KeyReleases::Unsupported => (KeyMode::Toggle, None),
        kazoo_host::terminal::KeyReleases::Failed(why) => (KeyMode::Toggle, Some(why.clone())),
    };

    let mut app = App::new(
        f64::from(sample_rate),
        CommandSender::new(cmd_tx),
        display_cons,
        key_mode,
    );
    app.key_mode_reason = key_mode_reason;
    let result = run_event_loop(&mut terminal, &mut app, &shared, &hub, &stream);

    // Cleanup. With the stream stopped no new note can start, so the note
    // the driven synth still holds is released before the link closes.
    let restored = terminal.leave();
    drop(stream);
    let released = release_driven(&shared, args.target, &hub);
    drop(hub);
    if let Some(report) = kazoo_core::host::link::exit_report() {
        eprintln!("{report}");
    }
    let result = result.and_then(|()| released.map_err(|why| color_eyre::eyre::eyre!(why)));

    match (result, restored) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(err), Ok(())) => Err(err),
        (Ok(()), Err(restore_err)) => Err(restore_err.into()),
        (Err(err), Err(restore_err)) => {
            Err(err.wrap_err(format!("restoring the terminal also failed: {restore_err}")))
        }
    }
}

/// How long the link is given to deliver the last release before it closes:
/// forty of its half millisecond polls.
const LINK_DRAIN: Duration = Duration::from_millis(20);

/// Release the note the driven synth still holds, and give the link time to
/// deliver it (and any release the engine queued last) before it closes.
fn release_driven(shared: &EngineShared, target: [u8; 16], hub: &HubLink) -> Result<(), String> {
    let connected = hub.is_connected();
    let outcome = crate::arp::release_on_quit(shared, target, |message| hub.send_note(message));
    if connected {
        std::thread::sleep(LINK_DRAIN);
    }
    match outcome {
        Ok(_) | Err(RequestError::NotConnected) => Ok(()),
        Err(RequestError::Full) => Err(
            "the last driven note could not be released: the desk queue was full, so the target synth may still hold it".to_owned(),
        ),
        Err(RequestError::Invalid) => Err(
            "the last driven note could not be released: the link refused it as invalid".to_owned(),
        ),
        Err(RequestError::Unsupported) => Err(
            "the last driven note could not be released: the desk is too old for that request".to_owned(),
        ),
    }
}

/// Main TUI event loop.
fn run_event_loop(
    terminal: &mut Tui,
    app: &mut App,
    shared: &EngineShared,
    hub: &HubLink,
    stream: &OpenedOutput,
) -> color_eyre::Result<()> {
    let frame_duration = Duration::from_millis(1000 / FPS);

    while !app.should_quit {
        stream.check_error_with_handler(|error| shared.record_stream_error(&error))?;
        let frame_start = Instant::now();

        // Follow the audio thread and the desk link.
        app.drain_display_events();
        app.retry_pending_note_offs();
        app.stream = StreamHealth {
            errors: shared.stream_errors.load(Ordering::Relaxed),
            lost: shared.stream_lost.load(Ordering::Acquire),
            display_dropped: shared.display_dropped.load(Ordering::Relaxed),
        };
        app.follow_engine(shared);
        app.hub = Some(hub.status());

        terminal.draw(|frame| ui::draw(frame, app))?;

        // Handle input events.
        let poll_time = frame_duration.saturating_sub(frame_start.elapsed());
        if event::poll(poll_time)? {
            if let Event::Key(key) = event::read()? {
                app.handle_key(key);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Args, USAGE, parse_args};

    #[test]
    fn help_asks_for_the_usage() {
        assert_eq!(parse_args(["-h".to_owned()]), Ok(None));
        assert_eq!(parse_args(["--help".to_owned()]), Ok(None));
        assert_eq!(parse_args([]), Ok(Some(Args::default())));
        assert_eq!(
            parse_args(["--bogus", "--help"].map(String::from)),
            Ok(None)
        );
        assert_eq!(parse_args(["-h", "--bogus"].map(String::from)), Ok(None));
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
        assert!(USAGE.contains("kazoo arp"));
        assert!(USAGE.contains("kazoo-arp"));
    }

    #[test]
    fn usage_opens_with_the_name_and_description() {
        assert!(
            USAGE.starts_with(
                "kazoo-arp: Jupiter-8 style arpeggiator, note scheduler for any synth\n"
            )
        );
    }
}
