//! kazoo-808 — TR-808 drum machine.
//!
//! All sounds synthesized, no samples. Plugs into the kazoo-mix desk when
//! it is running, following its tempo and play/stop.
//! See `studio/kazoo-808.md` for full specification.

mod app;
mod audio;
mod ui;

use std::io;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use app::{App, StreamHealth};
use audio::{AudioEngine, COMMAND_CAPACITY, CommandSender, EngineShared, MAX_CALLBACK_FRAMES};
use kazoo_core::ipc::link::{HubLink, LinkConfig, hub_link};

/// How long the UI waits for input before redrawing.
const FRAME_POLL: Duration = Duration::from_millis(16);

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    // Set up audio output.
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| color_eyre::eyre::eyre!("no audio output device found"))?;
    let config = device.default_output_config()?;
    let sample_rate = config.sample_rate() as f32;
    let channels = usize::from(config.channels());
    if channels == 0 {
        return Err(color_eyre::eyre::eyre!(
            "audio output device reports zero channels"
        ));
    }

    let shared = Arc::new(EngineShared::default());
    let (cmd_tx, cmd_rx) = crossbeam_channel::bounded(COMMAND_CAPACITY);

    // Plug into the kazoo-mix desk whenever it is running.
    let (hub, hub_audio) = hub_link(LinkConfig::new(
        "kazoo-808",
        2,
        config.sample_rate(),
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
    let error_shared = Arc::clone(&shared);
    let stream = device.build_output_stream(
        &config.into(),
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| engine.render(data),
        move |err| error_shared.record_stream_error(&err),
        None,
    )?;
    stream.play()?;

    let mut terminal = setup_terminal()?;

    // Run the TUI event loop.
    let mut app = App::new(sample_rate, CommandSender::new(cmd_tx));
    let result = run_event_loop(&mut terminal, &mut app, &shared, &hub);
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
) -> color_eyre::Result<()> {
    while !app.should_quit {
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
