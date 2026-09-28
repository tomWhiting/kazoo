//! kazoo-prophet — Sequential Prophet-5 inspired polyphonic synth.

mod app;
mod audio;
mod input;
mod ui;

use std::io;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use app::{App, StreamHealth};
use audio::{
    AudioCommand, AudioEngine, COMMAND_CAPACITY, DISPLAY_CAPACITY, DisplaySnapshot, EngineShared,
    MAX_CALLBACK_FRAMES,
};
use color_eyre::eyre::WrapErr;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::TrySendError;
use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    self, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use kazoo_core::ipc::link::{HubLink, LinkConfig, hub_link};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

const TARGET_FPS: u64 = 30;

type Tui = Terminal<CrosstermBackend<io::Stdout>>;

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| color_eyre::eyre::eyre!("no audio output device found"))?;
    let supported_config = device.default_output_config()?;
    let sample_rate = supported_config.sample_rate() as f32;
    let channels = usize::from(supported_config.channels());
    if channels == 0 {
        return Err(color_eyre::eyre::eyre!(
            "output device reports zero channels"
        ));
    }

    let (cmd_tx, cmd_rx) = crossbeam_channel::bounded::<AudioCommand>(COMMAND_CAPACITY);
    let (display_tx, display_rx) = crossbeam_channel::bounded::<DisplaySnapshot>(DISPLAY_CAPACITY);
    let shared = Arc::new(EngineShared::default());

    // Plug into the kazoo-mix desk whenever it is running.
    let (hub, hub_audio) = hub_link(LinkConfig::new(
        "kazoo-prophet",
        2,
        supported_config.sample_rate(),
        MAX_CALLBACK_FRAMES as u32,
    ))?;

    let mut engine = AudioEngine::new(
        sample_rate,
        channels,
        cmd_rx,
        display_tx,
        hub_audio,
        Arc::clone(&shared),
    );
    let error_shared = Arc::clone(&shared);
    let stream = device.build_output_stream(
        &supported_config.into(),
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| engine.render(data),
        move |err| error_shared.record_stream_error(&err),
        None,
    )?;
    stream.play().wrap_err("failed to start audio stream")?;

    let (mut terminal, keyboard) = setup_terminal()?;
    let release_events = matches!(keyboard, KeyReleases::Reported);

    let mut app = App::new(sample_rate as u32);
    app.key_release_note = keyboard.note();
    let result = run_event_loop(&mut terminal, &mut app, &cmd_tx, &display_rx, &shared, &hub);

    // Always restore the terminal, even if the loop failed. Dropping the
    // stream silences every voice; the desk is left once audio has stopped.
    let restored = restore_terminal(&mut terminal, release_events);
    drop(stream);
    drop(hub);

    match (result, restored) {
        (Ok(()), restored) => restored.map_err(Into::into),
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(restore_error)) => {
            // The loop error is the one that matters; the terminal state is
            // still reported so the user knows why their shell looks wrong.
            eprintln!("kazoo-prophet: could not fully restore the terminal: {restore_error}");
            Err(error)
        }
    }
}

fn run_event_loop(
    terminal: &mut Tui,
    app: &mut App,
    cmd_tx: &crossbeam_channel::Sender<AudioCommand>,
    display_rx: &crossbeam_channel::Receiver<DisplaySnapshot>,
    shared: &EngineShared,
    hub: &HubLink,
) -> color_eyre::Result<()> {
    let frame_duration = Duration::from_millis(1000 / TARGET_FPS);
    loop {
        let frame_start = Instant::now();

        while let Ok(snapshot) = display_rx.try_recv() {
            app.voice_status = snapshot.voice_status;
            app.waveform_buf = snapshot.waveform;
        }
        app.display_dropped = shared.display_dropped.load(Ordering::Relaxed);
        app.stream = StreamHealth {
            errors: shared.stream_errors.load(Ordering::Relaxed),
            lost: shared.stream_lost.load(Ordering::Acquire),
        };
        app.hub = Some(hub.status());
        flush_params(app, cmd_tx);

        terminal.draw(|frame| ui::draw(frame, app))?;

        let timeout = frame_duration.saturating_sub(frame_start.elapsed());
        if event::poll(timeout)? {
            process_event(app, cmd_tx)?;
            while event::poll(Duration::ZERO)? {
                process_event(app, cmd_tx)?;
            }
        }

        if app.should_quit {
            return Ok(());
        }
    }
}

/// Whether the terminal reports key releases, or why it does not.
#[derive(Debug)]
enum KeyReleases {
    Reported,
    /// The terminal has no keyboard enhancement protocol.
    Unsupported,
    /// Support could not be detected or enabled.
    Failed(String),
}

impl KeyReleases {
    /// What the user needs to know when notes cannot follow key releases.
    fn note(&self) -> Option<String> {
        match self {
            Self::Reported => None,
            Self::Unsupported => {
                Some("terminal can't report key releases: notes hold until Space".to_owned())
            }
            Self::Failed(why) => Some(format!("{why}: notes hold until Space")),
        }
    }
}

/// Enter raw mode and the alternate screen and ask for key-release events.
/// On failure, whatever was already changed is undone before returning.
fn setup_terminal() -> color_eyre::Result<(Tui, KeyReleases)> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    if let Err(error) = execute!(stdout, EnterAlternateScreen) {
        return Err(undo_setup(error, false, false));
    }
    let keyboard = match terminal::supports_keyboard_enhancement() {
        Ok(true) => match execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::REPORT_EVENT_TYPES)
        ) {
            Ok(()) => KeyReleases::Reported,
            Err(error) => KeyReleases::Failed(format!("could not enable key releases ({error})")),
        },
        Ok(false) => KeyReleases::Unsupported,
        Err(error) => KeyReleases::Failed(format!("could not query the keyboard ({error})")),
    };
    let pushed = matches!(keyboard, KeyReleases::Reported);
    match Terminal::new(CrosstermBackend::new(stdout)) {
        Ok(terminal) => Ok((terminal, keyboard)),
        Err(error) => Err(undo_setup(error, true, pushed)),
    }
}

/// Undo a partial [`setup_terminal`], attaching any cleanup failure to `error`.
fn undo_setup(error: io::Error, alternate: bool, pushed: bool) -> color_eyre::Report {
    let mut stdout = io::stdout();
    let pop = if pushed {
        execute!(stdout, PopKeyboardEnhancementFlags)
    } else {
        Ok(())
    };
    let leave = if alternate {
        execute!(stdout, LeaveAlternateScreen)
    } else {
        Ok(())
    };
    let raw = disable_raw_mode();
    let report = color_eyre::Report::new(error).wrap_err("could not set up the terminal");
    match pop.and(leave).and(raw) {
        Ok(()) => report,
        Err(cleanup_error) => report.wrap_err(format!(
            "and could not restore it afterwards: {cleanup_error}"
        )),
    }
}

/// Undo every terminal change. Every step is attempted even if an earlier one
/// fails; the first failure is returned.
fn restore_terminal(terminal: &mut Tui, release_events: bool) -> io::Result<()> {
    let pop = if release_events {
        execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags)
    } else {
        Ok(())
    };
    let raw = disable_raw_mode();
    let leave = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let cursor = terminal.show_cursor();
    pop.and(raw).and(leave).and(cursor)
}

fn process_event(
    app: &mut App,
    cmd_tx: &crossbeam_channel::Sender<AudioCommand>,
) -> color_eyre::Result<()> {
    if let Event::Key(key) = event::read()? {
        match key.kind {
            KeyEventKind::Press => {
                handle_key_press(app, key.code, key.modifiers, cmd_tx);
            }
            KeyEventKind::Release => handle_key_release(app, key.code, cmd_tx),
            KeyEventKind::Repeat => {}
        }
    }
    Ok(())
}

/// Why a command could not be handed to the audio thread.
const fn refusal_reason<T>(error: &TrySendError<T>) -> &'static str {
    match error {
        TrySendError::Full(_) => "audio engine busy",
        TrySendError::Disconnected(_) => "audio engine stopped",
    }
}

/// Send a command without ever blocking the UI. A refused command is counted
/// and its reason kept for the footer. Returns whether it was accepted.
fn send_command(
    app: &mut App,
    cmd_tx: &crossbeam_channel::Sender<AudioCommand>,
    cmd: AudioCommand,
) -> bool {
    match cmd_tx.try_send(cmd) {
        Ok(()) => {
            app.delivery_warning = None;
            true
        }
        Err(error) => {
            app.commands_dropped = app.commands_dropped.saturating_add(1);
            app.delivery_warning = Some(refusal_reason(&error));
            false
        }
    }
}

/// Hand the latest parameters to the audio thread. Only the newest set
/// matters, so a refused send stays pending and is retried every frame.
fn flush_params(app: &mut App, cmd_tx: &crossbeam_channel::Sender<AudioCommand>) {
    if !app.params_pending {
        return;
    }
    match cmd_tx.try_send(AudioCommand::UpdateParams(app.params.clone())) {
        Ok(()) => {
            app.params_pending = false;
            app.delivery_warning = None;
        }
        Err(error) => app.delivery_warning = Some(refusal_reason(&error)),
    }
}

fn handle_key_press(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
    cmd_tx: &crossbeam_channel::Sender<AudioCommand>,
) {
    // Quit is checked first: several quit keys are also note keys.
    if is_quit_key(code, modifiers) {
        app.should_quit = true;
        return;
    }

    if let Some(note) = input::key_to_note(code) {
        let velocity = if modifiers.contains(KeyModifiers::SHIFT) {
            1.0
        } else {
            0.82
        };
        // Only a note the engine accepted is shown as held.
        if send_command(app, cmd_tx, AudioCommand::NoteOn { note, velocity }) {
            app.add_held_note(note);
        }
        return;
    }

    match code {
        KeyCode::Tab => app.next_section(),
        KeyCode::BackTab => app.prev_section(),
        KeyCode::Down | KeyCode::Char('j') => app.next_param(),
        KeyCode::Up | KeyCode::Char('k') => app.prev_param(),
        KeyCode::Right | KeyCode::Char('l') => update_param(app, 1.0, cmd_tx),
        KeyCode::Left | KeyCode::Char('h') => update_param(app, -1.0, cmd_tx),
        // Held notes are cleared only once the engine accepted the release.
        KeyCode::Char(' ') if send_command(app, cmd_tx, AudioCommand::AllNotesOff) => {
            app.held_notes.fill(None);
        }
        _ => {}
    }
}

fn handle_key_release(
    app: &mut App,
    code: KeyCode,
    cmd_tx: &crossbeam_channel::Sender<AudioCommand>,
) {
    let Some(note) = input::key_to_note(code) else {
        return;
    };
    // A refused note-off keeps the note shown as held, so the screen matches
    // what sounds and Space can still release it.
    if send_command(app, cmd_tx, AudioCommand::NoteOff { note }) {
        app.remove_held_note(note);
    }
}

/// Esc, or Ctrl with Q, C or D. Plain letters are note keys, so quitting never
/// takes a bare letter.
const fn is_quit_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    if matches!(code, KeyCode::Esc) {
        return true;
    }
    modifiers.contains(KeyModifiers::CONTROL)
        && matches!(code, KeyCode::Char('q' | 'Q' | 'c' | 'C' | 'd' | 'D'))
}

fn update_param(app: &mut App, delta: f32, cmd_tx: &crossbeam_channel::Sender<AudioCommand>) {
    app.adjust_param(delta);
    app.params_pending = true;
    flush_params(app, cmd_tx);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(app: &mut App, tx: &crossbeam_channel::Sender<AudioCommand>, c: char) {
        handle_key_press(app, KeyCode::Char(c), KeyModifiers::NONE, tx);
    }

    #[test]
    fn accepted_note_is_held_and_released() {
        let (tx, rx) = crossbeam_channel::bounded(4);
        let mut app = App::new(48_000);
        press(&mut app, &tx, 'z');
        assert!(matches!(
            rx.try_recv(),
            Ok(AudioCommand::NoteOn { note: 48, .. })
        ));
        assert!(app.held_notes.contains(&Some(48)));
        handle_key_release(&mut app, KeyCode::Char('z'), &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(AudioCommand::NoteOff { note: 48 })
        ));
        assert!(!app.held_notes.contains(&Some(48)));
        assert_eq!(app.commands_dropped, 0);
    }

    #[test]
    fn refused_note_is_counted_and_not_shown() {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let mut app = App::new(48_000);
        press(&mut app, &tx, 'z');
        press(&mut app, &tx, 'x');
        assert_eq!(rx.try_iter().count(), 1);
        assert_eq!(app.commands_dropped, 1);
        assert_eq!(app.delivery_warning, Some("audio engine busy"));
        assert!(!app.held_notes.contains(&Some(50)));
        // The refused key can be pressed again once the engine keeps up.
        press(&mut app, &tx, 'x');
        assert!(app.held_notes.contains(&Some(50)));
        assert_eq!(app.delivery_warning, None);
    }

    #[test]
    fn refused_note_off_keeps_note_held() {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let mut app = App::new(48_000);
        press(&mut app, &tx, 'z');
        handle_key_release(&mut app, KeyCode::Char('z'), &tx);
        assert!(app.held_notes.contains(&Some(48)));
        assert_eq!(app.commands_dropped, 1);
        assert!(matches!(rx.try_recv(), Ok(AudioCommand::NoteOn { .. })));
        handle_key_release(&mut app, KeyCode::Char('z'), &tx);
        assert!(!app.held_notes.contains(&Some(48)));
    }

    #[test]
    fn refused_params_stay_pending_until_delivered() {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let mut app = App::new(48_000);
        press(&mut app, &tx, 'z');
        update_param(&mut app, 1.0, &tx);
        assert!(app.params_pending);
        assert_eq!(app.delivery_warning, Some("audio engine busy"));
        assert!(matches!(rx.try_recv(), Ok(AudioCommand::NoteOn { .. })));
        flush_params(&mut app, &tx);
        assert!(!app.params_pending);
        assert!(matches!(rx.try_recv(), Ok(AudioCommand::UpdateParams(_))));
    }

    #[test]
    fn stopped_engine_is_reported() {
        let (tx, rx) = crossbeam_channel::bounded(4);
        drop(rx);
        let mut app = App::new(48_000);
        press(&mut app, &tx, ' ');
        assert_eq!(app.delivery_warning, Some("audio engine stopped"));
        assert_eq!(app.commands_dropped, 1);
    }

    #[test]
    fn quit_keys_win_over_notes() {
        let (tx, rx) = crossbeam_channel::bounded(4);
        // 'q' and 'c' are note keys; with Ctrl they quit instead of playing.
        for code in [KeyCode::Char('q'), KeyCode::Char('c'), KeyCode::Char('d')] {
            let mut app = App::new(48_000);
            handle_key_press(&mut app, code, KeyModifiers::CONTROL, &tx);
            assert!(app.should_quit, "{code:?}");
        }
        let mut app = App::new(48_000);
        handle_key_press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &tx);
        assert!(app.should_quit);
        assert!(rx.try_recv().is_err(), "a quit key must not play a note");
        // A bare letter still plays.
        let mut app = App::new(48_000);
        handle_key_press(&mut app, KeyCode::Char('q'), KeyModifiers::NONE, &tx);
        assert!(!app.should_quit);
        assert!(matches!(
            rx.try_recv(),
            Ok(AudioCommand::NoteOn { note: 60, .. })
        ));
    }

    #[test]
    fn key_release_note_explains_failures() {
        assert!(KeyReleases::Reported.note().is_none());
        assert!(KeyReleases::Unsupported.note().is_some());
        let failed = KeyReleases::Failed("could not query the keyboard (boom)".to_owned());
        assert!(failed.note().is_some_and(|n| n.contains("boom")));
    }
}
