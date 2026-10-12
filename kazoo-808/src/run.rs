use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use kazoo_core::host::audio::{OpenedOutput, OutputRequest, RateChoice, open_output};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::app::{App, StreamHealth};
use crate::audio::{
    AudioEngine, COMMAND_CAPACITY, CommandSender, EngineShared, MAX_CALLBACK_FRAMES,
};
use crate::ui;
use kazoo_core::ipc::link::{HubLink, LinkConfig, hub_link};

/// What the command line gave: nothing yet for this instrument.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Args;

/// What `--help` prints.
pub const USAGE: &str = "kazoo-808: TR-808 drum machine, all synthesis, no samples\n\nusage: kazoo 808\n       kazoo-808\n\nIt plugs into the kazoo-mix desk whenever one is running and follows its\ntempo and play/stop; alone, it keeps its own. Press ? inside for the keys.\n";

/// Read arguments after the program name. `Ok(None)` requests help; an error
/// is one sentence naming the first unknown argument.
pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<Args>, String> {
    let args: Vec<_> = args.into_iter().collect();
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        return Ok(None);
    }
    args.first().map_or(Ok(Some(Args)), |other| {
        Err(format!("unknown argument '{other}'"))
    })
}

/// The standalone binary's entry: parse, print the usage on help, and run.
pub fn cli(args: impl IntoIterator<Item = String>) -> color_eyre::Result<()> {
    match parse_args(args) {
        Ok(None) => {
            let mut stdout = io::stdout().lock();
            stdout.write_all(USAGE.as_bytes())?;
            stdout.flush()?;
            Ok(())
        }
        Ok(Some(args)) => run(args),
        Err(why) => Err(color_eyre::eyre::eyre!("{why}\n\n{USAGE}")),
    }
}

/// How long the UI waits for input before redrawing.
const FRAME_POLL: Duration = Duration::from_millis(16);

/// Run the instrument until it quits: audio, desk link, terminal, event loop.
/// Error reports must already be installed by the caller with `color_eyre::install`.
pub fn run(args: Args) -> color_eyre::Result<()> {
    let Args = args;

    // Set up audio output.
    let mut request = OutputRequest::default();
    let output_info = request.info()?;
    request.rate = RateChoice::Hz(output_info.rate);
    let sample_rate = output_info.rate as f32;
    let channels = usize::from(output_info.channels);

    let shared = Arc::new(EngineShared::default());
    let (cmd_tx, cmd_rx) = crossbeam_channel::bounded(COMMAND_CAPACITY);

    // Plug into the kazoo-mix desk whenever it is running.
    let (hub, hub_audio) = hub_link(LinkConfig::new(
        "kazoo-808",
        2,
        output_info.rate,
        MAX_CALLBACK_FRAMES as u32,
    ))?;

    // Build and start the audio stream.
    let mut engine = AudioEngine::new(
        sample_rate,
        channels,
        cmd_rx,
        hub_audio,
        Arc::clone(&shared),
    );
    let stream = open_output(&request, move |data, _| engine.render(data))?;

    let mut terminal = setup_terminal()?;

    // Run the TUI event loop.
    let mut app = App::new(sample_rate, CommandSender::new(cmd_tx));
    let result = run_event_loop(&mut terminal, &mut app, &shared, &hub, &stream);
    let restored = restore_terminal(&mut terminal);

    // Stop audio, then leave the desk.
    drop(stream);
    drop(hub);

    match (result, restored) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(err), Ok(())) => Err(err),
        (Ok(()), Err(restore_err)) => Err(restore_err.into()),
        (Err(err), Err(restore_err)) => {
            Err(err.wrap_err(format!("restoring the terminal also failed: {restore_err}")))
        }
    }
}

type Tui = Terminal<CrosstermBackend<io::Stdout>>;

/// Put the terminal into raw mode on the alternate screen. On failure the
/// terminal is put back as it was.
fn setup_terminal() -> io::Result<Tui> {
    terminal::enable_raw_mode()?;
    match enter_screen() {
        Ok(terminal) => Ok(terminal),
        Err(err) => match terminal::disable_raw_mode() {
            Ok(()) => Err(err),
            Err(raw_err) => Err(io::Error::other(format!(
                "{err}; leaving raw mode also failed: {raw_err}"
            ))),
        },
    }
}

fn enter_screen() -> io::Result<Tui> {
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    match Terminal::new(CrosstermBackend::new(stdout)) {
        Ok(terminal) => Ok(terminal),
        Err(err) => match execute!(io::stdout(), LeaveAlternateScreen) {
            Ok(()) => Err(err),
            Err(undo_err) => Err(io::Error::other(format!(
                "{err}; leaving the alternate screen also failed: {undo_err}"
            ))),
        },
    }
}

/// Undo the terminal setup. Every step runs even if an earlier one fails;
/// the first failure is returned.
fn restore_terminal(terminal: &mut Tui) -> io::Result<()> {
    let raw = terminal::disable_raw_mode();
    let screen = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let cursor = terminal.show_cursor();
    raw.and(screen).and(cursor)
}

/// Main TUI event loop.
fn run_event_loop(
    terminal: &mut Tui,
    app: &mut App,
    shared: &EngineShared,
    hub: &HubLink,
    stream: &OpenedOutput,
) -> color_eyre::Result<()> {
    while !app.should_quit {
        stream.check_error_with_handler(|error| shared.record_stream_error(&error))?;
        // Update display state from the audio thread and the desk link.
        app.playback_step = shared.playback_step.load(Ordering::Acquire);
        app.sequencer.playing = shared.playing.load(Ordering::Acquire);
        // The desk can change the tempo too.
        app.sequencer
            .clock
            .set_bpm(f64::from_bits(shared.bpm.load(Ordering::Acquire)));
        app.desk_lost = shared.desk_lost.load(Ordering::Relaxed);
        app.stream = StreamHealth {
            errors: shared.stream_errors.load(Ordering::Relaxed),
            lost: shared.stream_lost.load(Ordering::Acquire),
        };
        app.hub = Some(hub.status());

        terminal.draw(|frame| ui::draw(frame, app))?;

        // Poll for events with a timeout for smooth playback animation.
        if event::poll(FRAME_POLL)? {
            if let Event::Key(key) = event::read()? {
                // Terminals with key-release reporting would otherwise run
                // every action twice.
                if key.kind != KeyEventKind::Release {
                    app.handle_key(key);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_asks_for_the_usage() {
        for flag in ["-h", "--help"] {
            assert_eq!(parse_args([flag.to_string()]), Ok(None));
        }
        assert_eq!(parse_args([]), Ok(Some(Args)));
        assert_eq!(
            parse_args(["--bogus".to_string(), "--help".to_string()]),
            Ok(None)
        );
    }

    #[test]
    fn an_unknown_argument_is_refused_by_name() {
        let error = parse_args(["--bogus".to_string(), "extra".to_string()]).unwrap_err();
        assert_eq!(error, "unknown argument '--bogus'");
    }

    #[test]
    fn usage_names_both_ways_to_start_it() {
        assert!(USAGE.contains("kazoo 808"));
        assert!(USAGE.contains("kazoo-808"));
    }
}
