use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::app::{App, StreamHealth};
use crate::audio::{
    AudioCommand, AudioEngine, COMMAND_CAPACITY, DISPLAY_CAPACITY, DisplaySnapshot, EngineShared,
    MAX_CALLBACK_FRAMES,
};
use crossbeam_channel::TrySendError;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use kazoo_core::host::audio::{OpenedOutput, OutputRequest};
use kazoo_core::host::link::{LinkSetup, connect_or_direct};
use kazoo_core::ipc::link::HubLink;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::ui;
use kazoo_host::keys::{self, PianoKey};

/// What the command line gave: nothing yet for this instrument.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Args;

/// What `--help` prints.
pub const USAGE: &str = "kazoo-prophet: Sequential Prophet-5 inspired polyphonic analog synth

usage: kazoo prophet
       kazoo-prophet

It plugs into the kazoo-mix desk whenever one is running, and plays on its
own otherwise. Keys are on the panel.
";

/// Read the arguments after the program name. `Ok(None)` means help was
/// asked for; `Err` is one sentence saying what was wrong (the caller adds
/// the usage).
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
    unknown.map_or(Ok(Some(Args)), |other| {
        Err(format!("unknown argument '{other}'"))
    })
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

const TARGET_FPS: u64 = 30;

type Tui = Terminal<CrosstermBackend<io::Stdout>>;

/// Run the instrument until it quits: audio, desk link, terminal, event loop.
///
/// Error reports must already be installed by the caller
/// (`kazoo_host::reports::install`); this never installs them, so one process can
/// run several instruments one after another.
pub fn run(args: Args) -> color_eyre::Result<()> {
    let Args = args;

    let request = OutputRequest::default();
    let prepared = request.prepare_output()?;
    let output_info = prepared.info();
    let sample_rate = output_info.rate as f32;
    let channels = usize::from(output_info.channels);

    let (cmd_tx, cmd_rx) = crossbeam_channel::bounded::<AudioCommand>(COMMAND_CAPACITY);
    let (display_tx, display_rx) = crossbeam_channel::bounded::<DisplaySnapshot>(DISPLAY_CAPACITY);
    let shared = Arc::new(EngineShared::default());

    // Plug into the kazoo-mix desk whenever it is running.
    let (hub, hub_audio) = connect_or_direct(&LinkSetup {
        name: "kazoo-prophet",
        channels: 2,
        rate: output_info.rate,
        block: MAX_CALLBACK_FRAMES,
    })?;

    let mut engine = AudioEngine::new(
        sample_rate,
        channels,
        cmd_rx,
        display_tx,
        hub_audio,
        Arc::clone(&shared),
    );
    let stream = prepared.open(move |data, _| engine.render(data))?;

    let mut terminal = kazoo_host::terminal::enter_with(kazoo_host::terminal::Options {
        mouse: false,
        key_release_events: true,
        panics: kazoo_host::terminal::Panics::AnyThread,
    })?;
    let keyboard = match terminal.key_releases() {
        kazoo_host::terminal::KeyReleases::Reported => KeyReleases::Reported,
        kazoo_host::terminal::KeyReleases::NotAsked
        | kazoo_host::terminal::KeyReleases::Unsupported => KeyReleases::Unsupported,
        kazoo_host::terminal::KeyReleases::Failed(why) => KeyReleases::Failed(why.clone()),
    };

    let mut app = App::new(sample_rate as u32);
    app.key_release_note = keyboard.note();
    let result = run_event_loop(
        &mut terminal,
        &mut app,
        &cmd_tx,
        &display_rx,
        &shared,
        &hub,
        &stream,
    );

    // Always restore the terminal, even if the loop failed. Dropping the
    // stream silences every voice; the desk is left once audio has stopped.
    let restored = terminal.leave();
    drop(stream);
    drop(hub);
    if let Some(report) = kazoo_core::host::link::exit_report() {
        eprintln!("{report}");
    }

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
    stream: &OpenedOutput,
) -> color_eyre::Result<()> {
    let frame_duration = Duration::from_millis(1000 / TARGET_FPS);
    loop {
        stream.check_error_with_handler(|error| shared.record_stream_error(&error))?;
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

fn release_all_notes(app: &mut App, tx: &crossbeam_channel::Sender<AudioCommand>) -> bool {
    if !send_command(app, tx, AudioCommand::AllNotesOff) {
        return false;
    }
    app.held_notes.fill(None);
    true
}

fn handle_key_press(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
    cmd_tx: &crossbeam_channel::Sender<AudioCommand>,
) {
    if is_quit_key(code, modifiers) {
        app.should_quit = true;
        return;
    }

    let piano = match code {
        KeyCode::Char(ch) => keys::piano_key(ch, app.octave),
        _ => None,
    };
    match piano {
        Some(PianoKey::Note(note)) => {
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
        Some(PianoKey::OctaveDown) => {
            if release_all_notes(app, cmd_tx) {
                app.octave = app.octave.down();
            }
            return;
        }
        Some(PianoKey::OctaveUp) => {
            if release_all_notes(app, cmd_tx) {
                app.octave = app.octave.up();
            }
            return;
        }
        None => {}
    }

    match code {
        KeyCode::Tab => app.next_section(),
        KeyCode::BackTab => app.prev_section(),
        KeyCode::Down => app.next_param(),
        KeyCode::Up => app.prev_param(),
        KeyCode::Right => update_param(app, 1.0, cmd_tx),
        KeyCode::Left => update_param(app, -1.0, cmd_tx),
        KeyCode::Char(' ') => {
            release_all_notes(app, cmd_tx);
        }
        _ => {}
    }
}

fn handle_key_release(
    app: &mut App,
    code: KeyCode,
    cmd_tx: &crossbeam_channel::Sender<AudioCommand>,
) {
    let KeyCode::Char(ch) = code else {
        return;
    };
    let Some(note) = keys::note(ch, app.octave) else {
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
    fn octave_moves_release_held_notes_and_change_the_note() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let mut app = App::new(48_000);
        press(&mut app, &tx, 'a');
        assert!(matches!(
            rx.try_recv(),
            Ok(AudioCommand::NoteOn { note: 48, .. })
        ));
        press(&mut app, &tx, 'x');
        assert_eq!(app.octave, keys::Octave::new(4));
        assert!(matches!(rx.try_recv(), Ok(AudioCommand::AllNotesOff)));
        assert!(app.held_notes.iter().all(Option::is_none));
        press(&mut app, &tx, 'a');
        assert!(matches!(
            rx.try_recv(),
            Ok(AudioCommand::NoteOn { note: 60, .. })
        ));
    }

    #[test]
    fn a_key_released_with_shift_changed_still_releases() {
        for (pressed, released) in [(':', ':'), (':', ';'), (';', ':'), ('A', 'a'), ('a', 'A')] {
            let (tx, rx) = crossbeam_channel::bounded(16);
            let mut app = App::new(48_000);
            let note = keys::note(pressed, app.octave).expect("a piano key");
            handle_key_press(&mut app, KeyCode::Char(pressed), KeyModifiers::SHIFT, &tx);
            assert!(
                matches!(rx.try_recv(), Ok(AudioCommand::NoteOn { note: n, .. }) if n == note),
                "{pressed}"
            );
            handle_key_release(&mut app, KeyCode::Char(released), &tx);
            assert!(
                matches!(rx.try_recv(), Ok(AudioCommand::NoteOff { note: n }) if n == note),
                "{pressed} then {released}"
            );
            assert!(app.held_notes.iter().all(Option::is_none), "{pressed}");
        }
    }

    #[test]
    fn the_piano_is_the_shared_one() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let mut app = App::new(48_000);
        press(&mut app, &tx, ';');
        assert!(matches!(
            rx.try_recv(),
            Ok(AudioCommand::NoteOn { note: 64, .. })
        ));
        press(&mut app, &tx, 'q');
        assert!(rx.is_empty());
        assert!(!app.should_quit);
    }

    #[test]
    fn refused_octave_release_keeps_the_held_note() {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let mut app = App::new(48_000);
        press(&mut app, &tx, 'a');
        assert!(app.held_notes.contains(&Some(48)));
        press(&mut app, &tx, 'x');
        assert_eq!(app.octave, keys::Octave::DEFAULT);
        assert!(app.held_notes.contains(&Some(48)));
        assert_eq!(app.commands_dropped, 1);
        assert!(matches!(
            rx.try_recv(),
            Ok(AudioCommand::NoteOn { note: 48, .. })
        ));
        press(&mut app, &tx, 'x');
        assert_eq!(app.octave, keys::Octave::new(4));
        assert!(matches!(rx.try_recv(), Ok(AudioCommand::AllNotesOff)));
        assert!(app.held_notes.iter().all(Option::is_none));
    }

    #[test]
    fn accepted_note_is_held_and_released() {
        let (tx, rx) = crossbeam_channel::bounded(4);
        let mut app = App::new(48_000);
        press(&mut app, &tx, 'a');
        assert!(matches!(
            rx.try_recv(),
            Ok(AudioCommand::NoteOn { note: 48, .. })
        ));
        assert!(app.held_notes.contains(&Some(48)));
        handle_key_release(&mut app, KeyCode::Char('a'), &tx);
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
        press(&mut app, &tx, 'a');
        press(&mut app, &tx, 's');
        assert_eq!(rx.try_iter().count(), 1);
        assert_eq!(app.commands_dropped, 1);
        assert_eq!(app.delivery_warning, Some("audio engine busy"));
        assert!(!app.held_notes.contains(&Some(50)));
        // The refused key can be pressed again once the engine keeps up.
        press(&mut app, &tx, 's');
        assert!(app.held_notes.contains(&Some(50)));
        assert_eq!(app.delivery_warning, None);
    }

    #[test]
    fn refused_note_off_keeps_note_held() {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let mut app = App::new(48_000);
        press(&mut app, &tx, 'a');
        handle_key_release(&mut app, KeyCode::Char('a'), &tx);
        assert!(app.held_notes.contains(&Some(48)));
        assert_eq!(app.commands_dropped, 1);
        assert!(matches!(rx.try_recv(), Ok(AudioCommand::NoteOn { .. })));
        handle_key_release(&mut app, KeyCode::Char('a'), &tx);
        assert!(!app.held_notes.contains(&Some(48)));
    }

    #[test]
    fn refused_params_stay_pending_until_delivered() {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let mut app = App::new(48_000);
        press(&mut app, &tx, 'a');
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
        // Control quit bindings take precedence over piano notes.
        for code in [KeyCode::Char('q'), KeyCode::Char('c'), KeyCode::Char('d')] {
            let mut app = App::new(48_000);
            handle_key_press(&mut app, code, KeyModifiers::CONTROL, &tx);
            assert!(app.should_quit, "{code:?}");
        }
        let mut app = App::new(48_000);
        handle_key_press(&mut app, KeyCode::Esc, KeyModifiers::NONE, &tx);
        assert!(app.should_quit);
        assert!(rx.try_recv().is_err(), "a quit key must not play a note");
        // Unmapped plain letters stay idle; piano keys still play.
        let mut app = App::new(48_000);
        handle_key_press(&mut app, KeyCode::Char('q'), KeyModifiers::NONE, &tx);
        assert!(!app.should_quit);
        assert!(rx.is_empty());
        handle_key_press(&mut app, KeyCode::Char('d'), KeyModifiers::NONE, &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(AudioCommand::NoteOn { note: 52, .. })
        ));
    }

    #[test]
    fn key_release_note_explains_failures() {
        assert!(KeyReleases::Reported.note().is_none());
        assert!(KeyReleases::Unsupported.note().is_some());
        let failed = KeyReleases::Failed("could not query the keyboard (boom)".to_owned());
        assert!(failed.note().is_some_and(|n| n.contains("boom")));
    }

    #[test]
    fn help_asks_for_the_usage() {
        assert_eq!(parse_args(["-h".to_owned()]), Ok(None));
        assert_eq!(parse_args(["--help".to_owned()]), Ok(None));
        assert_eq!(parse_args([]), Ok(Some(Args)));
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
        assert!(USAGE.contains("kazoo prophet"));
        assert!(USAGE.contains("kazoo-prophet"));
    }

    #[test]
    fn usage_opens_with_the_name_and_description() {
        assert!(
            USAGE.starts_with(
                "kazoo-prophet: Sequential Prophet-5 inspired polyphonic analog synth\n"
            )
        );
    }
}
