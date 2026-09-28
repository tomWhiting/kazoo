//! kazoo-303 — TB-303 inspired acid bassline synthesizer.
//!
//! This instrument is fully procedural: no samples, wavetables, or recordings are
//! played back. The sound is generated from oscillator math, envelopes, glide,
//! accent dynamics, and a resonant low-pass filter.

mod audio;
mod sequencer;
mod synth;
mod ui;

use std::io;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use kazoo_core::ipc::link::{HubLink, LinkConfig, LinkStatus, hub_link};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use audio::{
    AudioCommand, AudioEngine, COMMAND_CAPACITY, EngineShared, MAX_BLOCK, SendFailure, apply_edit,
};
use sequencer::{Sequencer, SequencerClock};
use synth::{AcidSynth, AcidSynthParam};

/// How long the UI waits for input before redrawing.
const FRAME_POLL: Duration = Duration::from_millis(16);

/// Swing change per key press.
const SWING_STEP: f64 = 0.01;

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| color_eyre::eyre::eyre!("no audio output device found"))?;
    let config = device.default_output_config()?;
    let sample_rate = config.sample_rate();
    let channels = usize::from(config.channels());
    if channels == 0 {
        return Err(color_eyre::eyre::eyre!(
            "output device reports zero channels"
        ));
    }

    let shared = Arc::new(EngineShared::default());
    let (cmd_tx, cmd_rx) = crossbeam_channel::bounded::<AudioCommand>(COMMAND_CAPACITY);

    // Plug into the kazoo-mix desk whenever it is running.
    let (hub, hub_audio) = hub_link(LinkConfig::new(
        "kazoo-303",
        2,
        sample_rate,
        MAX_BLOCK as u32,
    ))?;

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

    // Device rates are whole numbers of Hz, well inside f32's exact range.
    let mut app = App::new(sample_rate as f32);
    let result = run_event_loop(&mut terminal, &mut app, &cmd_tx, &shared, &hub);

    // Always restore the terminal, even if the loop failed.
    let restored = restore_terminal(&mut terminal);
    // Stop audio, then leave the desk.
    drop(stream);
    drop(hub);

    match (result, restored) {
        (Ok(()), restored) => restored.map_err(Into::into),
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(restore_error)) => Err(error.wrap_err(format!(
            "restoring the terminal also failed: {restore_error}"
        ))),
    }
}

/// Enter raw mode and the alternate screen. On failure, whatever was already
/// changed is undone before returning.
fn setup_terminal() -> color_eyre::Result<Terminal<CrosstermBackend<io::Stdout>>> {
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    if let Err(error) = execute!(stdout, EnterAlternateScreen) {
        return Err(undo_setup(error, false));
    }
    Terminal::new(CrosstermBackend::new(stdout)).map_err(|error| undo_setup(error, true))
}

/// Undo a partial [`setup_terminal`], attaching any cleanup failure to `error`.
fn undo_setup(error: io::Error, alternate: bool) -> color_eyre::Report {
    let leave = if alternate {
        execute!(io::stdout(), LeaveAlternateScreen)
    } else {
        Ok(())
    };
    let raw = terminal::disable_raw_mode();
    let report = color_eyre::Report::new(error).wrap_err("could not set up the terminal");
    match leave.and(raw) {
        Ok(()) => report,
        Err(cleanup_error) => report.wrap_err(format!(
            "and could not restore it afterwards: {cleanup_error}"
        )),
    }
}

/// Undo every terminal change. Every step is attempted even if an earlier one
/// fails; the first failure is returned.
fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
    let raw = terminal::disable_raw_mode();
    let leave = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let cursor = terminal.show_cursor();
    raw.and(leave).and(cursor)
}

/// Audio stream health, read from the engine each frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct StreamHealth {
    /// Errors cpal reported for the output stream.
    errors: u64,
    /// The output device is gone: no more audio until restart.
    lost: bool,
}

/// UI state. `sequencer` and `synth` mirror the engine's: edits are applied
/// here only once the engine has accepted them, and the transport (play,
/// tempo) is read back from the engine, which may be following the desk.
#[derive(Debug)]
struct App {
    sequencer: Sequencer,
    synth: AcidSynth,
    cursor_step: usize,
    selected_param: usize,
    playback_step: usize,
    playing: bool,
    show_help: bool,
    /// Edits the audio thread did not accept, and so were not applied.
    commands_dropped: u64,
    /// Why the most recent edit was not applied, shown until the next one lands.
    delivery_warning: Option<&'static str>,
    /// Desk link state; `None` until the first poll.
    hub: Option<LinkStatus>,
    /// Messages lost between the 303 and the desk.
    desk_lost: u64,
    /// Note the desk is playing on the 303.
    hub_note: Option<u8>,
    /// Output stream health.
    stream: StreamHealth,
}

impl App {
    fn new(sample_rate: f32) -> Self {
        Self {
            sequencer: Sequencer::new(sample_rate),
            synth: AcidSynth::new(sample_rate),
            cursor_step: 0,
            selected_param: 0,
            playback_step: 0,
            playing: false,
            show_help: false,
            commands_dropped: 0,
            delivery_warning: None,
            hub: None,
            desk_lost: 0,
            hub_note: None,
            stream: StreamHealth::default(),
        }
    }

    const fn selected_param(&self) -> AcidSynthParam {
        AcidSynthParam::ALL[self.selected_param]
    }

    /// Follow what the engine publishes.
    fn follow_engine(&mut self, shared: &EngineShared) {
        self.playback_step = shared.playback_step.load(Ordering::Acquire);
        self.playing = shared.playing.load(Ordering::Acquire);
        // The desk can change the tempo too.
        self.sequencer.clock.set_bpm(shared.bpm());
        self.desk_lost = shared.desk_lost.load(Ordering::Relaxed);
        self.hub_note = shared.hub_note();
        self.stream = StreamHealth {
            errors: shared.stream_errors.load(Ordering::Relaxed),
            lost: shared.stream_lost.load(Ordering::Acquire),
        };
    }

    /// Where the 303's sound goes, for the header.
    fn link_label(&self) -> String {
        match &self.hub {
            None => String::from("\u{25cf} local"),
            Some(hub) if hub.connected => hub.strip.map_or_else(
                || String::from("\u{2192} kazoo-mix"),
                |strip| format!("\u{2192} kazoo-mix strip {}", u16::from(strip) + 1),
            ),
            Some(hub) => hub.last_refusal.as_ref().map_or_else(
                || String::from("\u{25cf} local (no desk running)"),
                |why| format!("\u{25cf} local (desk: {why})"),
            ),
        }
    }

    /// Send a command to the audio thread without blocking. Only an accepted
    /// edit is applied to the on-screen mirror; a refused one is counted and
    /// shown.
    fn dispatch(&mut self, tx: &crossbeam_channel::Sender<AudioCommand>, cmd: AudioCommand) {
        match audio::send(tx, cmd) {
            Ok(()) => {
                apply_edit(cmd, &mut self.sequencer, &mut self.synth);
                self.delivery_warning = None;
            }
            Err(SendFailure::QueueFull) => {
                self.commands_dropped = self.commands_dropped.saturating_add(1);
                self.delivery_warning = Some("audio engine busy: last edit not applied, try again");
            }
            Err(SendFailure::EngineGone) => {
                self.commands_dropped = self.commands_dropped.saturating_add(1);
                self.delivery_warning = Some("audio engine stopped: edits cannot be applied");
            }
        }
    }
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    cmd_tx: &crossbeam_channel::Sender<AudioCommand>,
    shared: &EngineShared,
    hub: &HubLink,
) -> color_eyre::Result<()> {
    loop {
        app.follow_engine(shared);
        app.hub = Some(hub.status());
        terminal.draw(|frame| ui::draw(frame, app))?;

        if event::poll(FRAME_POLL)? {
            if let Event::Key(key) = event::read()? {
                // Terminals with key-release reporting would otherwise run
                // every action twice.
                if key.kind != KeyEventKind::Release && handle_key_event(app, key, cmd_tx) {
                    return Ok(());
                }
            }
        }
    }
}

/// Returns true when the app should quit.
fn handle_key_event(
    app: &mut App,
    key: KeyEvent,
    cmd_tx: &crossbeam_channel::Sender<AudioCommand>,
) -> bool {
    if is_quit_key(key.code, key.modifiers) {
        return true;
    }

    if app.show_help {
        app.show_help = false;
        return false;
    }

    let step = app.cursor_step;
    let command = match key.code {
        KeyCode::Char('?') => {
            app.show_help = true;
            None
        }
        KeyCode::Left => {
            app.cursor_step = app.cursor_step.saturating_sub(1);
            None
        }
        KeyCode::Right => {
            app.cursor_step = (app.cursor_step + 1).min(sequencer::STEPS_PER_PATTERN - 1);
            None
        }
        KeyCode::Up => {
            app.selected_param = app.selected_param.saturating_sub(1);
            None
        }
        KeyCode::Down => {
            app.selected_param = (app.selected_param + 1).min(AcidSynthParam::ALL.len() - 1);
            None
        }
        KeyCode::Enter => Some(if app.playing {
            AudioCommand::Stop
        } else {
            AudioCommand::Play
        }),
        KeyCode::Char(' ') => Some(AudioCommand::ToggleStep(step)),
        KeyCode::Char('a') => Some(AudioCommand::ToggleAccent(step)),
        KeyCode::Char('s') => Some(AudioCommand::ToggleSlide(step)),
        KeyCode::Char('z') => Some(AudioCommand::TransposeStep {
            step,
            semitones: -1,
        }),
        KeyCode::Char('x') => Some(AudioCommand::TransposeStep { step, semitones: 1 }),
        KeyCode::Char('+' | '=') => Some(AudioCommand::SetBpm(app.sequencer.clock.bpm() + 1.0)),
        KeyCode::Char('-') => Some(AudioCommand::SetBpm(app.sequencer.clock.bpm() - 1.0)),
        KeyCode::Char(']') => Some(AudioCommand::SetSwing(
            (app.sequencer.clock.swing() + SWING_STEP).min(SequencerClock::MAX_SWING),
        )),
        KeyCode::Char('[') => Some(AudioCommand::SetSwing(
            (app.sequencer.clock.swing() - SWING_STEP).max(SequencerClock::MIN_SWING),
        )),
        KeyCode::Char(',') => Some(param_nudge(app, -0.03)),
        KeyCode::Char('.') => Some(param_nudge(app, 0.03)),
        KeyCode::Char('w') => Some(AudioCommand::SetWaveform(app.synth.waveform().toggled())),
        KeyCode::Char('r') => Some(AudioCommand::RandomizePattern),
        _ => None,
    };
    if let Some(cmd) = command {
        app.dispatch(cmd_tx, cmd);
    }
    false
}

/// Nudge the selected voice parameter; the synth clamps it to 0..=1.
fn param_nudge(app: &App, delta: f32) -> AudioCommand {
    let param = app.selected_param();
    AudioCommand::SetParam {
        param,
        value: app.synth.param_value(param) + delta,
    }
}

const fn is_quit_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    if matches!(code, KeyCode::Esc) {
        return true;
    }
    if matches!(code, KeyCode::Char('q' | 'Q')) {
        return true;
    }
    modifiers.contains(KeyModifiers::CONTROL)
        && matches!(code, KeyCode::Char('q' | 'Q' | 'c' | 'C' | 'd' | 'D'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(app: &mut App, tx: &crossbeam_channel::Sender<AudioCommand>, c: char) -> bool {
        handle_key_event(app, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE), tx)
    }

    #[test]
    fn accepted_edit_updates_mirror() {
        let (tx, rx) = crossbeam_channel::bounded(4);
        let mut app = App::new(44_100.0);
        let before = app.sequencer.current_pattern().steps[0].active;
        assert!(!press(&mut app, &tx, ' '));
        assert!(matches!(rx.try_recv(), Ok(AudioCommand::ToggleStep(0))));
        assert_eq!(app.sequencer.current_pattern().steps[0].active, !before);
        assert_eq!(app.commands_dropped, 0);
        assert!(app.delivery_warning.is_none());
    }

    #[test]
    fn refused_edit_is_counted_and_not_applied() {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let mut app = App::new(44_100.0);
        app.cursor_step = 1;
        // The first edit fills the queue; the audio thread is not draining it.
        assert!(!press(&mut app, &tx, 'w'));
        let step_before = app.sequencer.current_pattern().steps[1];
        assert!(!press(&mut app, &tx, ' '));
        assert_eq!(app.sequencer.current_pattern().steps[1], step_before);
        assert_eq!(app.commands_dropped, 1);
        assert!(app.delivery_warning.is_some());
        assert_eq!(rx.try_iter().count(), 1);
        // The next accepted edit clears the warning but keeps the count.
        assert!(!press(&mut app, &tx, ' '));
        assert!(app.delivery_warning.is_none());
        assert_eq!(app.commands_dropped, 1);
    }

    #[test]
    fn stopped_engine_is_reported() {
        let (tx, rx) = crossbeam_channel::bounded(4);
        drop(rx);
        let mut app = App::new(44_100.0);
        let waveform = app.synth.waveform();
        assert!(!press(&mut app, &tx, 'w'));
        assert_eq!(app.synth.waveform(), waveform);
        assert_eq!(app.commands_dropped, 1);
        assert_eq!(
            app.delivery_warning,
            Some("audio engine stopped: edits cannot be applied")
        );
    }

    #[test]
    fn param_edits_are_clamped_like_the_engine() {
        let (tx, _rx) = crossbeam_channel::bounded(64);
        let mut app = App::new(44_100.0);
        for _ in 0..50 {
            press(&mut app, &tx, '.');
        }
        assert!((app.synth.param_value(AcidSynthParam::Cutoff) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn transport_keys_ask_the_engine_and_the_screen_follows_its_answer() {
        let (tx, rx) = crossbeam_channel::bounded(8);
        let mut app = App::new(44_100.0);
        press(&mut app, &tx, '-');
        assert_eq!(rx.try_recv(), Ok(AudioCommand::SetBpm(131.0)));
        handle_key_event(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &tx,
        );
        assert_eq!(rx.try_recv(), Ok(AudioCommand::Play));
        // Asking changes nothing on screen: the engine (or the desk) decides.
        assert!((app.sequencer.clock.bpm() - 132.0).abs() < 1e-9);
        assert!(!app.playing);

        let shared = EngineShared::default();
        shared.bpm.store(117.0_f64.to_bits(), Ordering::Release);
        shared.playing.store(true, Ordering::Release);
        shared.desk_lost.store(3, Ordering::Relaxed);
        shared.hub_note.store(40, Ordering::Release);
        app.follow_engine(&shared);
        assert!((app.sequencer.clock.bpm() - 117.0).abs() < 1e-9);
        assert!(app.playing);
        assert_eq!(app.desk_lost, 3);
        assert_eq!(app.hub_note, Some(40));
        // Playing, Enter asks to stop.
        handle_key_event(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &tx,
        );
        assert_eq!(rx.try_recv(), Ok(AudioCommand::Stop));
    }

    #[test]
    fn swing_keys_stay_in_range() {
        let (tx, rx) = crossbeam_channel::bounded(64);
        let mut app = App::new(44_100.0);
        press(&mut app, &tx, '[');
        assert_eq!(
            rx.try_recv(),
            Ok(AudioCommand::SetSwing(SequencerClock::MIN_SWING))
        );
        for _ in 0..40 {
            press(&mut app, &tx, ']');
        }
        assert!((app.sequencer.clock.swing() - SequencerClock::MAX_SWING).abs() < 1e-12);
    }

    #[test]
    fn the_link_label_says_where_the_sound_goes() {
        let mut app = App::new(44_100.0);
        assert_eq!(app.link_label(), "\u{25cf} local");
        let mut status = LinkStatus {
            connected: true,
            strip: Some(2),
            blocks_sent: 0,
            blocks_dropped: 0,
            messages_dropped: 0,
            connections: 1,
            last_refusal: None,
        };
        app.hub = Some(status.clone());
        assert_eq!(app.link_label(), "\u{2192} kazoo-mix strip 3");
        status.connected = false;
        status.strip = None;
        status.last_refusal = Some(String::from("every strip is taken"));
        app.hub = Some(status);
        assert_eq!(
            app.link_label(),
            "\u{25cf} local (desk: every strip is taken)"
        );
    }

    #[test]
    fn quit_keys() {
        assert!(is_quit_key(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(is_quit_key(KeyCode::Esc, KeyModifiers::NONE));
        assert!(is_quit_key(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(!is_quit_key(KeyCode::Char('c'), KeyModifiers::NONE));
    }
}
