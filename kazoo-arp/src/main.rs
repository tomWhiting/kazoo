//! kazoo-arp — Jupiter-8 style arpeggiator standalone TUI.
//!
//! The arpeggiator engine lives in `lib.rs` for embedding in other crates.
//! This binary provides a standalone terminal interface with a simple
//! audition synth so arpeggiated notes are audible.

mod app;
mod audio;
mod ui;

use std::io;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossterm::event::{
    self, Event, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ringbuf::traits::Split;

use crate::app::{App, KeyMode, StreamHealth};
use crate::audio::{
    AudioEngine, COMMAND_CAPACITY, CommandSender, DISPLAY_CAPACITY, EngineShared,
    MAX_CALLBACK_FRAMES,
};
use kazoo_core::ipc::link::{HubLink, LinkConfig, hub_link};

/// Target frame rate for the TUI.
const FPS: u64 = 30;

type Tui = Terminal<CrosstermBackend<io::Stdout>>;

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    // -----------------------------------------------------------------------
    // Audio setup
    // -----------------------------------------------------------------------

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| color_eyre::eyre::eyre!("no audio output device found"))?;
    let supported_config = device.default_output_config()?;
    let sample_rate = supported_config.sample_rate();
    let channels = usize::from(supported_config.channels());
    if channels == 0 {
        return Err(color_eyre::eyre::eyre!(
            "audio output device reports zero channels"
        ));
    }

    let (cmd_tx, cmd_rx) = crossbeam_channel::bounded(COMMAND_CAPACITY);
    // Display events: Audio -> UI (lock-free SPSC).
    let (display_prod, display_cons) = ringbuf::HeapRb::new(DISPLAY_CAPACITY).split();
    let shared = Arc::new(EngineShared::default());

    // Plug into the kazoo-mix desk whenever it is running.
    let (hub, hub_audio) = hub_link(LinkConfig::new(
        "kazoo-arp",
        2,
        sample_rate,
        MAX_CALLBACK_FRAMES as u32,
    ))?;

    // Build audio stream with arp engine + audition voice.
    let mut engine = AudioEngine::new(
        sample_rate,
        channels,
        cmd_rx,
        hub_audio,
        display_prod,
        Arc::clone(&shared),
    );
    let error_shared = Arc::clone(&shared);
    let stream = device.build_output_stream(
        &supported_config.into(),
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| engine.render(data),
        move |err| error_shared.record_stream_error(&err),
        None,
    )?;
    stream.play()?;

    // -----------------------------------------------------------------------
    // Terminal setup
    // -----------------------------------------------------------------------

    let (mut terminal, key_mode, key_mode_reason) = setup_terminal()?;

    let mut app = App::new(
        f64::from(sample_rate),
        CommandSender::new(cmd_tx),
        display_cons,
        key_mode,
    );
    app.key_mode_reason = key_mode_reason;
    let result = run_event_loop(&mut terminal, &mut app, &shared, &hub);

    // Cleanup.
    drop(stream);
    drop(hub);
    let restored = restore_terminal(&mut terminal, key_mode);

    match (result, restored) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(err), Ok(())) => Err(err),
        (Ok(()), Err(restore_err)) => Err(restore_err.into()),
        (Err(err), Err(restore_err)) => {
            Err(err.wrap_err(format!("restoring the terminal also failed: {restore_err}")))
        }
    }
}

/// Put the terminal into raw mode on the alternate screen, with key
/// releases when the terminal can report them. On failure the terminal is
/// put back as it was.
fn setup_terminal() -> io::Result<(Tui, KeyMode, Option<String>)> {
    terminal::enable_raw_mode()?;
    match enter_screen() {
        Ok(setup) => Ok(setup),
        Err(err) => match terminal::disable_raw_mode() {
            Ok(()) => Err(err),
            Err(raw_err) => Err(io::Error::other(format!(
                "{err}; leaving raw mode also failed: {raw_err}"
            ))),
        },
    }
}

fn enter_screen() -> io::Result<(Tui, KeyMode, Option<String>)> {
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    // Key releases (kitty keyboard protocol) let notes sound while held.
    // Without them, notes fall back to tap-on / tap-off.
    let (key_mode, reason) = match terminal::supports_keyboard_enhancement() {
        Ok(true) => match execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::REPORT_EVENT_TYPES)
        ) {
            Ok(()) => (KeyMode::Hold, None),
            Err(err) => (
                KeyMode::Toggle,
                Some(format!("could not turn on key releases: {err}")),
            ),
        },
        Ok(false) => (KeyMode::Toggle, None),
        Err(err) => (
            KeyMode::Toggle,
            Some(format!(
                "could not ask the terminal for key releases: {err}"
            )),
        ),
    };

    match Terminal::new(CrosstermBackend::new(stdout)) {
        Ok(terminal) => Ok((terminal, key_mode, reason)),
        Err(err) => {
            let mut stdout = io::stdout();
            let flags = match key_mode {
                KeyMode::Hold => execute!(stdout, PopKeyboardEnhancementFlags),
                KeyMode::Toggle => Ok(()),
            };
            let screen = execute!(stdout, LeaveAlternateScreen);
            match flags.and(screen) {
                Ok(()) => Err(err),
                Err(undo_err) => Err(io::Error::other(format!(
                    "{err}; leaving the alternate screen also failed: {undo_err}"
                ))),
            }
        }
    }
}

/// Undo the terminal setup. Every step runs even if an earlier one fails;
/// the first failure is returned.
fn restore_terminal(terminal: &mut Tui, key_mode: KeyMode) -> io::Result<()> {
    let flags = match key_mode {
        KeyMode::Hold => execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags),
        KeyMode::Toggle => Ok(()),
    };
    let raw = terminal::disable_raw_mode();
    let screen = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let cursor = terminal.show_cursor();
    flags.and(raw).and(screen).and(cursor)
}

/// Main TUI event loop.
fn run_event_loop(
    terminal: &mut Tui,
    app: &mut App,
    shared: &EngineShared,
    hub: &HubLink,
) -> color_eyre::Result<()> {
    let frame_duration = Duration::from_millis(1000 / FPS);

    while !app.should_quit {
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
