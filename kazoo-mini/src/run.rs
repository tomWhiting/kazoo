use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::Receiver;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use kazoo_core::host::audio::{OpenedOutput, OutputRequest};
use kazoo_core::host::link::{LinkSetup, connect_or_direct};
use kazoo_core::ipc::link::HubLink;

use crate::app::App;
use crate::audio::{
    AudioSetup, AudioStats, DISPLAY_BUF_SIZE, DisplaySnapshot, MAX_CALLBACK_FRAMES,
};
use crate::command::CommandLink;
use crate::params::MiniParams;
use kazoo_host::terminal::Terminal as Tui;

use crate::{audio, input, ui};

/// What the command line gave: nothing yet for this instrument.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Args;

/// What `--help` prints.
pub const USAGE: &str =
    "kazoo-mini: Moog Minimoog inspired bass/lead synth, monophonic, ladder filter

usage: kazoo mini
       kazoo-mini

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
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(USAGE.as_bytes())?;
            stdout.flush()?;
            Ok(())
        }
        Err(why) => Err(color_eyre::eyre::eyre!("{why}\n\n{USAGE}")),
        Ok(Some(args)) => run(args),
    }
}

/// UI frame period (~60 FPS).
const TICK_RATE: Duration = Duration::from_millis(16);

/// Backend error messages queued for the UI before further ones are only
/// counted.
const STREAM_ERROR_BACKLOG: usize = 8;

/// UI-side ends of the audio plumbing, polled once per frame.
struct UiChannels<'a> {
    commands: CommandLink,
    display_rx: Receiver<DisplaySnapshot>,
    stream_error_rx: Receiver<cpal::StreamError>,
    stream_error_tx: crossbeam_channel::Sender<cpal::StreamError>,
    stats: Arc<AudioStats>,
    hub: &'a HubLink,
}

/// Run the instrument until it quits: audio, desk link, terminal, event loop.
///
/// Error reports must already be installed by the caller
/// (`kazoo_host::reports::install`); this never installs them, so one process can
/// run several instruments one after another.
pub fn run(args: Args) -> color_eyre::Result<()> {
    let Args = args;

    // -----------------------------------------------------------------------
    // Audio setup — direct cpal output, no input stream needed
    // -----------------------------------------------------------------------

    let request = OutputRequest::default();
    let prepared = request.prepare_output()?;
    let output_info = prepared.info();
    let sample_rate = output_info.rate;
    let channels = usize::from(output_info.channels);

    // Command channel: UI -> Audio (lock-free bounded MPSC).
    let (cmd_tx, cmd_rx) = crossbeam_channel::bounded(256);

    // Display channel: Audio -> UI. The audio side evicts the oldest unread
    // snapshot when full, so the UI always sees the newest.
    let (display_tx, display_rx) = crossbeam_channel::bounded::<DisplaySnapshot>(2);

    // Backend errors: Audio -> UI, counted when the backlog is full.
    let (stream_error_tx, stream_error_rx) = crossbeam_channel::bounded(STREAM_ERROR_BACKLOG);
    let stats = Arc::new(AudioStats::default());

    // Plug into the kazoo-mix desk whenever it is running.
    let (hub, hub_audio) = connect_or_direct(&LinkSetup {
        name: "kazoo-mini",
        channels: 2,
        rate: sample_rate,
        block: MAX_CALLBACK_FRAMES,
    })?;

    let stream = audio::build_audio_stream(
        prepared,
        AudioSetup {
            sample_rate: sample_rate as f32,
            channels,
            cmd_rx,
            display_tx,
            display_evict: display_rx.clone(),
            hub_audio,
            stats: Arc::clone(&stats),
        },
    )?;

    // -----------------------------------------------------------------------
    // Terminal + event loop
    // -----------------------------------------------------------------------

    let mut tui = kazoo_host::terminal::enter_with(kazoo_host::terminal::Options {
        mouse: false,
        key_release_events: true,
    })?;
    let keyboard = tui.key_releases().clone();

    let mut app = App::new(sample_rate);
    app.key_releases = keyboard;
    let mut channels = UiChannels {
        commands: CommandLink::new(cmd_tx),
        display_rx,
        stream_error_rx,
        stream_error_tx,
        stats,
        hub: &hub,
    };
    let result = event_loop(&mut tui, &mut app, &mut channels, &stream);

    // -----------------------------------------------------------------------
    // Cleanup: always restore the terminal, then stop audio and leave the desk.
    // -----------------------------------------------------------------------

    let restored = tui.leave();
    drop(stream);
    drop(channels);
    drop(hub);
    if let Some(report) = kazoo_core::host::link::exit_report() {
        eprintln!("{report}");
    }
    match (result, restored) {
        (Ok(()), restored) => restored.map_err(Into::into),
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(restore_error)) => Err(error.wrap_err(format!(
            "could not fully restore the terminal either: {restore_error}"
        ))),
    }
}

/// The UI event loop.
fn event_loop(
    tui: &mut Tui,
    app: &mut App,
    channels: &mut UiChannels<'_>,
    stream: &OpenedOutput,
) -> color_eyre::Result<()> {
    loop {
        stream.check_error_with_handler(|error| {
            let queued = channels.stream_error_tx.try_send(error).is_ok();
            channels.stats.record_stream_error(queued);
        })?;
        poll_audio(app, channels);

        tui.draw(|f| ui::draw(f, app))?;

        if event::poll(TICK_RATE)? {
            if let Event::Key(key) = event::read()? {
                handle_key_event(app, key, &mut channels.commands);
            }
        }

        if app.should_quit {
            return Ok(());
        }
    }
}

/// Pull everything the audio side has produced into the app state, and
/// retry commands the audio callback could not take earlier.
fn poll_audio(app: &mut App, channels: &mut UiChannels<'_>) {
    // Keep only the latest display snapshot.
    if let Some(snap) = channels.display_rx.try_iter().last() {
        app.voice.set_display_note(snap.current_note);
        app.voice.set_display_write_pos(snap.write_pos);
        let app_display = app.voice.display_samples_mut();
        let copy_len = app_display.len().min(DISPLAY_BUF_SIZE);
        app_display[..copy_len].copy_from_slice(&snap.waveform[..copy_len]);
    }
    if let Some(err) = channels.stream_error_rx.try_iter().last() {
        app.health.last_stream_error = Some(err.to_string());
    }

    channels
        .commands
        .flush(|| MiniParams::from_voice(&app.voice));
    app.health.queue = channels.commands.status();
    app.health.display_dropped = channels.stats.display_dropped();
    app.health.stream_errors = channels.stats.stream_errors();
    app.health.stream_errors_unreported = channels.stats.stream_errors_unreported();
    app.hub = Some(channels.hub.status());
}

/// Route one key event: notes, panic, then navigation / parameter edits.
fn handle_key_event(app: &mut App, key: KeyEvent, commands: &mut CommandLink) {
    match key.kind {
        KeyEventKind::Press => {
            let piano = match key.code {
                KeyCode::Char(ch) => kazoo_host::keys::piano_key(ch, app.octave),
                _ => None,
            };
            if let Some(piano) = piano {
                match piano {
                    kazoo_host::keys::PianoKey::Note(note) => {
                        let held = &mut app.held_notes[usize::from(note)];
                        if !*held {
                            *held = true;
                            commands.note_on(note);
                        }
                    }
                    direction => {
                        app.release_all_notes();
                        commands.all_notes_off();
                        app.octave = match direction {
                            kazoo_host::keys::PianoKey::OctaveDown => app.octave.down(),
                            kazoo_host::keys::PianoKey::OctaveUp => app.octave.up(),
                            kazoo_host::keys::PianoKey::Note(_) => app.octave,
                        };
                    }
                }
            } else if key.code == KeyCode::Backspace {
                // Panic: silence the voice, forget every held key.
                app.release_all_notes();
                commands.all_notes_off();
                app.set_status("All notes off", false);
            } else if input::handle_key(app, key.code, key.modifiers) {
                // Sync parameter changes to the audio thread.
                commands.params(&MiniParams::from_voice(&app.voice));
            }
        }
        KeyEventKind::Release => {
            if let KeyCode::Char(ch) = key.code
                && let Some(note) = kazoo_host::keys::note(ch, app.octave)
            {
                let held = &mut app.held_notes[usize::from(note)];
                if *held {
                    *held = false;
                    commands.note_off(note);
                }
            }
        }
        // Auto-repeat: a held note is already sounding.
        KeyEventKind::Repeat => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::AudioCommand;
    use crossterm::event::{KeyEventState, KeyModifiers};

    fn key(code: KeyCode, kind: KeyEventKind) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind,
            state: KeyEventState::NONE,
        }
    }

    fn link() -> (CommandLink, Receiver<AudioCommand>) {
        let (tx, rx) = crossbeam_channel::bounded(16);
        (CommandLink::new(tx), rx)
    }

    #[test]
    fn octave_moves_release_held_notes_and_change_the_note() {
        let (mut commands, rx) = link();
        let mut app = App::new(44100);
        let ch = 'a';
        handle_key_event(
            &mut app,
            key(KeyCode::Char(ch), KeyEventKind::Press),
            &mut commands,
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(AudioCommand::NoteOn { note: 48, .. })
        ));
        let ch = 'x';
        handle_key_event(
            &mut app,
            key(KeyCode::Char(ch), KeyEventKind::Press),
            &mut commands,
        );
        assert_eq!(app.octave, kazoo_host::keys::Octave::new(4));
        assert!(
            rx.try_iter()
                .any(|cmd| matches!(cmd, AudioCommand::AllNotesOff))
        );
        assert!(app.held_notes.iter().all(|held| !held));
        let ch = 'a';
        handle_key_event(
            &mut app,
            key(KeyCode::Char(ch), KeyEventKind::Press),
            &mut commands,
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(AudioCommand::NoteOn { note: 60, .. })
        ));
    }

    #[test]
    fn note_key_press_and_release() {
        let (mut commands, rx) = link();
        let mut app = App::new(44100);
        handle_key_event(
            &mut app,
            key(KeyCode::Char('a'), KeyEventKind::Press),
            &mut commands,
        );
        handle_key_event(
            &mut app,
            key(KeyCode::Char('a'), KeyEventKind::Press),
            &mut commands,
        );
        handle_key_event(
            &mut app,
            key(KeyCode::Char('a'), KeyEventKind::Release),
            &mut commands,
        );
        let got: Vec<AudioCommand> = rx.try_iter().collect();
        assert_eq!(got.len(), 2, "repeat press must not retrigger");
        assert!(matches!(got[0], AudioCommand::NoteOn { note: 48 }));
        assert!(matches!(got[1], AudioCommand::NoteOff { note: 48 }));
    }

    #[test]
    fn backspace_releases_everything() {
        let (mut commands, rx) = link();
        let mut app = App::new(44100);
        handle_key_event(
            &mut app,
            key(KeyCode::Char('a'), KeyEventKind::Press),
            &mut commands,
        );
        handle_key_event(
            &mut app,
            key(KeyCode::Backspace, KeyEventKind::Press),
            &mut commands,
        );
        let got: Vec<AudioCommand> = rx.try_iter().collect();
        assert!(matches!(got.last(), Some(AudioCommand::AllNotesOff)));
        assert!(app.held_notes.iter().all(|&h| !h));
        // The key plays again straight away.
        handle_key_event(
            &mut app,
            key(KeyCode::Char('a'), KeyEventKind::Press),
            &mut commands,
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(AudioCommand::NoteOn { note: 48 })
        ));
    }

    #[test]
    fn param_edit_sends_params_and_unhandled_key_does_not() {
        let (mut commands, rx) = link();
        let mut app = App::new(44100);
        handle_key_event(
            &mut app,
            key(KeyCode::Right, KeyEventKind::Press),
            &mut commands,
        );
        assert!(matches!(rx.try_recv(), Ok(AudioCommand::UpdateParams(_))));
        handle_key_event(
            &mut app,
            key(KeyCode::F(9), KeyEventKind::Press),
            &mut commands,
        );
        assert!(rx.try_recv().is_err());
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
        assert!(USAGE.contains("kazoo mini"));
        assert!(USAGE.contains("kazoo-mini"));
    }

    #[test]
    fn usage_opens_with_the_name_and_description() {
        assert!(USAGE.starts_with(
            "kazoo-mini: Moog Minimoog inspired bass/lead synth, monophonic, ladder filter\n"
        ));
    }
}
