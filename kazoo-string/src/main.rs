//! kazoo-string — plucked-string physical-model synthesizer for the terminal.
//!
//! Plays from the computer keyboard, from the kazoo hub (so kazoo-arp can
//! drive it), or from a looped `kazoo-play` notation phrase given with
//! `--phrase "<notation>"`. All sound is a Karplus-Strong digital waveguide
//! computed in the cpal output callback. Nothing is sampled.

mod audio;
mod body;
mod patch;
mod phrase;
mod synth;
mod ui;
mod waveguide;

use std::io;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use audio::{
    AudioCommand, AudioEngine, AudioStats, DEFAULT_BPM, DisplaySnapshot, MAX_BLOCK_FRAMES,
};
use kazoo_core::ipc::link::{HubLink, LinkConfig, LinkStatus, hub_link};
use patch::{PATCHES, ParamField, Patch};
use phrase::{MAX_BPM, MIN_BPM, Phrase};

/// Without key-release reporting, a key auto-repeating is released this long
/// after its last repeat.
const REPEAT_HOLD: Duration = Duration::from_millis(140);
/// Fallback OS key-repeat delay when the system setting can't be read.
const DEFAULT_REPEAT_DELAY: Duration = Duration::from_millis(250);

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    let phrase_text = parse_args(std::env::args().skip(1))?;

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

    // The phrase is parsed and allocated here, before the audio thread starts.
    let phrase = match &phrase_text {
        Some(text) => Some(Phrase::parse(text, DEFAULT_BPM, sample_rate).map_err(|e| {
            color_eyre::eyre::eyre!(
                "could not parse phrase at token {}: {}",
                e.token_index + 1,
                e.message
            )
        })?),
        None => None,
    };
    let has_phrase = phrase.is_some();

    let (cmd_tx, cmd_rx) = crossbeam_channel::bounded::<AudioCommand>(256);
    let (display_tx, display_rx) = crossbeam_channel::bounded::<DisplaySnapshot>(2);

    // Plug into the kazoo-mix desk whenever it is running.
    let (hub, hub_audio) = hub_link(LinkConfig::new(
        "kazoo-string",
        2,
        sample_rate,
        MAX_BLOCK_FRAMES as u32,
    ))?;
    let stats = Arc::new(AudioStats::default());

    let mut engine = AudioEngine::new(
        sample_rate,
        channels,
        cmd_rx,
        display_tx,
        hub_audio,
        phrase,
        Arc::clone(&stats),
    );
    let error_stats = Arc::clone(&stats);
    let stream = device.build_output_stream(
        &config.into(),
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| engine.render(data),
        move |err| error_stats.record_stream_error(&err),
        None,
    )?;
    stream.play()?;

    let (mut terminal, keyboard) = setup_terminal()?;
    let release_events = matches!(keyboard, KeyReleases::Reported);

    let mut app = App::new(&keyboard, has_phrase, hub.status(), Arc::clone(&stats));
    let result = run_event_loop(&mut terminal, &mut app, &hub, &cmd_tx, &display_rx);

    // Always restore the terminal, even if the loop failed.
    let restored = restore_terminal(&mut terminal, release_events);
    drop(stream);
    drop(hub);

    match (result, restored) {
        (Ok(()), restored) => restored.map_err(Into::into),
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(restore_error)) => {
            // The loop error is the one that matters; the terminal state is
            // still reported so the user knows why their shell looks wrong.
            eprintln!("kazoo-string: could not fully restore the terminal: {restore_error}");
            Err(error)
        }
    }
}

/// Whether the terminal reports key releases, or why it does not.
#[derive(Debug)]
enum KeyReleases {
    Reported,
    /// The terminal has no keyboard enhancement protocol.
    Unsupported,
    /// Support could not be detected or enabled; the reason is shown to the user.
    Failed(String),
}

/// Enter raw mode and the alternate screen and ask for key-release events.
/// On failure, whatever was already changed is undone before returning.
fn setup_terminal() -> color_eyre::Result<(Terminal<CrosstermBackend<io::Stdout>>, KeyReleases)> {
    terminal::enable_raw_mode()?;
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
            Err(error) => KeyReleases::Failed(format!("could not enable key releases: {error}")),
        },
        Ok(false) => KeyReleases::Unsupported,
        Err(error) => KeyReleases::Failed(format!("could not query the keyboard: {error}")),
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
    let raw = terminal::disable_raw_mode();
    let cleanup = pop.and(leave).and(raw);
    let report = color_eyre::Report::new(error).wrap_err("could not set up the terminal");
    match cleanup {
        Ok(()) => report,
        Err(cleanup_error) => report.wrap_err(format!(
            "and could not restore it afterwards: {cleanup_error}"
        )),
    }
}

/// Undo every terminal change. Every step is attempted even if an earlier one
/// fails; the first failure is returned.
fn restore_terminal(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    release_events: bool,
) -> io::Result<()> {
    let pop = if release_events {
        execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags)
    } else {
        Ok(())
    };
    let raw = terminal::disable_raw_mode();
    let leave = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let cursor = terminal.show_cursor();
    pop.and(raw).and(leave).and(cursor)
}

/// Accept `--phrase "<notation>"` or `--phrase=<notation>`.
fn parse_args(mut args: impl Iterator<Item = String>) -> color_eyre::Result<Option<String>> {
    let mut phrase = None;
    while let Some(arg) = args.next() {
        if arg == "--phrase" {
            let text = args.next().ok_or_else(|| {
                color_eyre::eyre::eyre!(
                    "--phrase needs notation, e.g. --phrase \"c4/8 e4/8 g4/8 c5/8\""
                )
            })?;
            phrase = Some(text);
        } else if let Some(text) = arg.strip_prefix("--phrase=") {
            phrase = Some(text.to_owned());
        } else if arg == "-h" || arg == "--help" {
            println!(
                "kazoo-string: plucked-string physical-model synth\n\nUSAGE: kazoo-string [--phrase \"<kazoo-play notation>\"]\n\nPress ? inside the app for keys."
            );
            std::process::exit(0);
        } else {
            return Err(color_eyre::eyre::eyre!(
                "unknown argument: {arg} (try --help)"
            ));
        }
    }
    Ok(phrase)
}

/// Rows on the panel: every patch control, then the master volume.
pub const ROWS: usize = ParamField::ALL.len() + 1;
/// The master volume row, below the patch controls.
pub const VOLUME_ROW: usize = ParamField::ALL.len();

#[derive(Debug, Clone, Copy)]
struct HeldKey {
    code: char,
    note: u8,
    /// Time of the last press or repeat seen for this key.
    last: Instant,
    /// Auto-repeat has started, so repeats arrive every few tens of ms.
    repeating: bool,
    /// The note is still on (it may have auto-released while the key is held).
    sounding: bool,
}

/// Whether a `--phrase` was given, and whether it is looping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhraseState {
    Absent,
    Stopped,
    Looping,
}

/// UI-thread state.
#[derive(Debug)]
pub struct App {
    pub patch: Patch,
    pub patch_index: usize,
    pub master: f32,
    /// The selected panel row: a [`ParamField`] index, or [`VOLUME_ROW`].
    pub focus: usize,
    pub octave: i8,
    pub velocity: u8,
    pub show_help: bool,
    pub release_events: bool,
    pub phrase: PhraseState,
    /// Phrase tempo, followed from the engine (the desk can change it).
    pub bpm: f64,
    pub display: DisplaySnapshot,
    /// The desk link, refreshed every UI frame.
    pub link: LinkStatus,
    pub status: String,
    /// Shared with the audio thread.
    pub stats: Arc<AudioStats>,
    /// UI commands dropped because the audio thread's queue was full or gone.
    pub commands_dropped: u64,
    held: Vec<HeldKey>,
    /// The OS delay before a held key starts repeating. Used to tell a held
    /// key from a quick second tap when the terminal can't report releases.
    repeat_delay: Duration,
}

impl App {
    fn new(
        keyboard: &KeyReleases,
        has_phrase: bool,
        link: LinkStatus,
        stats: Arc<AudioStats>,
    ) -> Self {
        let release_events = matches!(keyboard, KeyReleases::Reported);
        // The OS repeat delay only matters when releases must be inferred.
        let (repeat_delay, delay_note) = if release_events {
            (DEFAULT_REPEAT_DELAY, String::new())
        } else {
            match system_repeat_delay() {
                Ok(delay) => (delay, String::new()),
                Err(why) => (
                    DEFAULT_REPEAT_DELAY,
                    format!(
                        " Key-repeat delay unknown ({why}); assuming {} ms.",
                        DEFAULT_REPEAT_DELAY.as_millis()
                    ),
                ),
            }
        };
        let status_line = match keyboard {
            KeyReleases::Reported => {
                "Keyboard reports key release: notes hold while you hold the key.".to_owned()
            }
            KeyReleases::Unsupported => format!(
                "Terminal can't report key release: notes release automatically.{delay_note}"
            ),
            KeyReleases::Failed(why) => {
                format!("Key release unavailable ({why}): notes release automatically.{delay_note}")
            }
        };
        Self {
            patch: PATCHES[0],
            patch_index: 0,
            master: 0.8,
            focus: 0,
            octave: 4,
            velocity: 100,
            show_help: false,
            release_events,
            phrase: if has_phrase {
                PhraseState::Stopped
            } else {
                PhraseState::Absent
            },
            bpm: DEFAULT_BPM,
            display: DisplaySnapshot::EMPTY,
            link,
            status: status_line,
            stats,
            commands_dropped: 0,
            held: Vec::with_capacity(32),
            repeat_delay,
        }
    }

    #[must_use]
    pub fn held_chars(&self) -> Vec<char> {
        self.held
            .iter()
            .filter(|h| h.sounding)
            .map(|h| h.code)
            .collect()
    }

    fn load_patch(&mut self, index: usize, tx: &crossbeam_channel::Sender<AudioCommand>) {
        self.patch_index = index % PATCHES.len();
        self.patch = PATCHES[self.patch_index];
        self.status = format!("Loaded patch {}: {}", self.patch_index + 1, self.patch.name);
        send_command(
            &mut self.commands_dropped,
            tx,
            AudioCommand::SetPatch(self.patch),
        );
    }

    /// Change the selected row by `steps` of its own step size.
    fn adjust(&mut self, steps: f32, tx: &crossbeam_channel::Sender<AudioCommand>) {
        if self.focus >= VOLUME_ROW {
            self.master = steps.mul_add(0.02, self.master).clamp(0.0, 1.0);
            send_command(
                &mut self.commands_dropped,
                tx,
                AudioCommand::SetMaster(self.master),
            );
            return;
        }
        let field = ParamField::ALL[self.focus];
        let value = steps.mul_add(ParamField::STEP, self.patch.get(field));
        self.patch.set(field, value);
        send_command(
            &mut self.commands_dropped,
            tx,
            AudioCommand::SetPatch(self.patch),
        );
    }

    /// Move the selection up or down the panel, stopping at either end.
    fn move_focus(&mut self, rows: isize) {
        self.focus = self.focus.saturating_add_signed(rows).min(ROWS - 1);
    }

    fn set_status(&mut self, text: &str) {
        self.status.clear();
        self.status.push_str(text);
    }

    fn note_for(&self, key: char) -> Option<u8> {
        let offset = piano_offset(key)?;
        let note = i16::from(self.octave + 1) * 12 + i16::from(offset);
        // Outside the MIDI range the key simply plays nothing.
        if (0..=127).contains(&note) {
            Some(note as u8)
        } else {
            None
        }
    }

    /// A key-down. With release reporting, every press is a new note. Without
    /// it, the timing decides whether this is auto-repeat of a held key or a
    /// fresh tap that must retrigger.
    fn press_note(&mut self, key: char, tx: &crossbeam_channel::Sender<AudioCommand>) {
        let now = Instant::now();
        let delay = self.repeat_delay;
        let velocity = self.velocity;
        if let Some(held) = self.held.iter_mut().find(|h| h.code == key) {
            let gap = now.duration_since(held.last);
            let is_repeat = !self.release_events
                && if held.repeating {
                    gap < REPEAT_HOLD
                } else {
                    gap >= delay * 7 / 10 && gap <= delay * 2
                };
            if is_repeat {
                // Held key: keep it alive and never retrigger, even if it
                // already auto-released.
                held.last = now;
                held.repeating = true;
                return;
            }
            if held.sounding {
                send_command(
                    &mut self.commands_dropped,
                    tx,
                    AudioCommand::NoteOff { note: held.note },
                );
            }
            held.last = now;
            held.repeating = false;
            held.sounding = true;
            send_command(
                &mut self.commands_dropped,
                tx,
                AudioCommand::NoteOn {
                    note: held.note,
                    velocity,
                },
            );
            return;
        }
        let Some(note) = self.note_for(key) else {
            return;
        };
        self.held.push(HeldKey {
            code: key,
            note,
            last: now,
            repeating: false,
            sounding: true,
        });
        send_command(
            &mut self.commands_dropped,
            tx,
            AudioCommand::NoteOn { note, velocity },
        );
    }

    /// A terminal-reported auto-repeat: the key is still down.
    fn repeat_note(&mut self, key: char) {
        if let Some(held) = self.held.iter_mut().find(|h| h.code == key) {
            held.last = Instant::now();
            held.repeating = true;
        }
    }

    fn release_note(&mut self, key: char, tx: &crossbeam_channel::Sender<AudioCommand>) {
        if let Some(i) = self.held.iter().position(|h| h.code == key) {
            let held = self.held.swap_remove(i);
            if held.sounding {
                send_command(
                    &mut self.commands_dropped,
                    tx,
                    AudioCommand::NoteOff { note: held.note },
                );
            }
        }
    }

    fn expire_held(&mut self, tx: &crossbeam_channel::Sender<AudioCommand>) {
        if self.release_events {
            return;
        }
        let now = Instant::now();
        let first_hold = self.repeat_delay * 2;
        let dropped = &mut self.commands_dropped;
        self.held.retain_mut(|h| {
            let idle = now.duration_since(h.last);
            let limit = if h.repeating { REPEAT_HOLD } else { first_hold };
            if h.sounding && idle >= limit {
                send_command(dropped, tx, AudioCommand::NoteOff { note: h.note });
                h.sounding = false;
            }
            // Forget a silent key once no repeat could still be on its way.
            h.sounding || idle < first_hold
        });
    }

    /// Ask for the phrase to start or stop. The engine decides: plugged
    /// into the desk it asks the desk (which starts every instrument on the
    /// same frame), standalone it plays at once. The screen follows what the
    /// engine then reports.
    fn toggle_phrase(&mut self, tx: &crossbeam_channel::Sender<AudioCommand>) {
        let (start, status) = match (self.phrase, self.link.connected) {
            (PhraseState::Absent, _) => {
                self.set_status("No phrase loaded. Start with --phrase \"c4/8 e4/8 g4/4\".");
                return;
            }
            (PhraseState::Stopped, true) => (true, "Asking the desk to play."),
            (PhraseState::Stopped, false) => (true, "Phrase looping."),
            (PhraseState::Looping, true) => (false, "Asking the desk to stop."),
            (PhraseState::Looping, false) => (false, "Phrase stopped."),
        };
        send_command(
            &mut self.commands_dropped,
            tx,
            AudioCommand::PlayPhrase(start),
        );
        self.set_status(status);
    }

    /// Ask for the phrase tempo to move by `delta` BPM: the studio's tempo
    /// when plugged into the desk, the phrase's own when not.
    fn nudge_bpm(&mut self, delta: f64, tx: &crossbeam_channel::Sender<AudioCommand>) {
        if self.phrase == PhraseState::Absent {
            self.set_status("No phrase loaded: tempo keys set the phrase tempo.");
            return;
        }
        let bpm = (self.bpm + delta).round().clamp(MIN_BPM, MAX_BPM);
        send_command(&mut self.commands_dropped, tx, AudioCommand::SetBpm(bpm));
        self.status = if self.link.connected {
            format!("Asking the desk for {bpm:.0} BPM.")
        } else {
            format!("Phrase tempo {bpm:.0} BPM.")
        };
    }

    fn release_all(&mut self, tx: &crossbeam_channel::Sender<AudioCommand>) {
        self.held.clear();
        send_command(&mut self.commands_dropped, tx, AudioCommand::AllNotesOff);
    }
}

/// Queue a command for the audio thread without ever blocking the UI. A full
/// queue means the audio thread is stalled (a disconnected one that it has
/// stopped); the command is dropped and counted so the footer can tell the
/// user, who can clear any stuck note with Space once audio recovers.
fn send_command(
    dropped: &mut u64,
    tx: &crossbeam_channel::Sender<AudioCommand>,
    cmd: AudioCommand,
) {
    if tx.try_send(cmd).is_err() {
        *dropped = dropped.saturating_add(1);
    }
}

/// The OS key-repeat delay: the macOS `InitialKeyRepeat` setting. The error
/// says why it could not be read, for the status line.
fn system_repeat_delay() -> Result<Duration, String> {
    if !cfg!(target_os = "macos") {
        return Err("not readable on this OS".to_owned());
    }
    let out = std::process::Command::new("defaults")
        .args(["read", "-g", "InitialKeyRepeat"])
        .output()
        .map_err(|e| format!("could not run `defaults`: {e}"))?;
    if !out.status.success() {
        // An unset key is the normal case on a fresh system.
        return Err("InitialKeyRepeat is not set".to_owned());
    }
    let text = String::from_utf8(out.stdout)
        .map_err(|_| "InitialKeyRepeat is not valid text".to_owned())?;
    parse_initial_key_repeat(&text)
}

/// Parse `InitialKeyRepeat` (units of 15 ms), clamped to a sane range.
fn parse_initial_key_repeat(text: &str) -> Result<Duration, String> {
    let ticks = text
        .trim()
        .parse::<u64>()
        .map_err(|e| format!("InitialKeyRepeat {:?} is not a number: {e}", text.trim()))?;
    Ok(Duration::from_millis(ticks.saturating_mul(15))
        .clamp(Duration::from_millis(100), Duration::from_secs(2)))
}

/// Tracker-style piano: the home row is white keys, the row above is black keys.
#[must_use]
pub const fn piano_offset(key: char) -> Option<u8> {
    Some(match key {
        'a' => 0,
        'w' => 1,
        's' => 2,
        'e' => 3,
        'd' => 4,
        'f' => 5,
        't' => 6,
        'g' => 7,
        'y' => 8,
        'h' => 9,
        'u' => 10,
        'j' => 11,
        'k' => 12,
        'o' => 13,
        'l' => 14,
        'p' => 15,
        ';' => 16,
        _ => return None,
    })
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    hub: &HubLink,
    cmd_tx: &crossbeam_channel::Sender<AudioCommand>,
    display_rx: &crossbeam_channel::Receiver<DisplaySnapshot>,
) -> color_eyre::Result<()> {
    loop {
        while let Ok(snapshot) = display_rx.try_recv() {
            app.display = snapshot;
        }
        app.bpm = app.stats.bpm();
        if app.phrase != PhraseState::Absent {
            app.phrase = if app.stats.phrase_playing.load(Ordering::Acquire) {
                PhraseState::Looping
            } else {
                PhraseState::Stopped
            };
        }
        app.link = hub.status();
        app.expire_held(cmd_tx);
        terminal.draw(|frame| ui::draw(frame, app))?;

        if event::poll(Duration::from_millis(12))? {
            if let Event::Key(key) = event::read()? {
                if handle_key(app, key, cmd_tx) {
                    app.release_all(cmd_tx);
                    return Ok(());
                }
            }
        }
    }
}

/// Returns true when the app should quit.
fn handle_key(app: &mut App, key: KeyEvent, tx: &crossbeam_channel::Sender<AudioCommand>) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if ctrl && matches!(key.code, KeyCode::Char('c' | 'q' | 'd')) {
        return true;
    }

    if let KeyCode::Char(c) = key.code {
        let c = c.to_ascii_lowercase();
        if piano_offset(c).is_some() {
            // Releases always count, whatever modifiers are held by then,
            // so a note can never stick.
            match key.kind {
                KeyEventKind::Release => {
                    app.release_note(c, tx);
                    return false;
                }
                KeyEventKind::Repeat => {
                    app.repeat_note(c);
                    return false;
                }
                KeyEventKind::Press if !ctrl => {
                    app.press_note(c, tx);
                    return false;
                }
                KeyEventKind::Press => {}
            }
        }
    }

    if key.kind == KeyEventKind::Release {
        return false;
    }

    if app.show_help {
        app.show_help = false;
        return false;
    }

    let coarse = key.modifiers.contains(KeyModifiers::SHIFT);
    match key.code {
        KeyCode::Esc | KeyCode::Char('q' | 'Q') => return true,
        KeyCode::Char('?') => app.show_help = true,
        KeyCode::Up => app.move_focus(-1),
        KeyCode::Down => app.move_focus(1),
        KeyCode::Left | KeyCode::Char('-' | '_') => {
            app.adjust(if coarse { -5.0 } else { -1.0 }, tx);
        }
        KeyCode::Right | KeyCode::Char('=' | '+') => app.adjust(if coarse { 5.0 } else { 1.0 }, tx),
        KeyCode::Tab => app.load_patch(app.patch_index + 1, tx),
        KeyCode::BackTab => app.load_patch(app.patch_index + PATCHES.len() - 1, tx),
        KeyCode::Char('1'..='6') => {
            if let KeyCode::Char(c) = key.code {
                app.load_patch(usize::from(c as u8 - b'1'), tx);
            }
        }
        KeyCode::Char('z' | 'Z') => {
            app.release_all(tx);
            app.octave = (app.octave - 1).max(0);
        }
        KeyCode::Char('x' | 'X') => {
            app.release_all(tx);
            app.octave = (app.octave + 1).min(8);
        }
        KeyCode::Char('c' | 'C') => app.velocity = app.velocity.saturating_sub(10).max(10),
        KeyCode::Char('v' | 'V') => app.velocity = app.velocity.saturating_add(10).min(127),
        KeyCode::Char(' ') => {
            app.release_all(tx);
            app.set_status("All notes off.");
        }
        KeyCode::Char('n' | 'N') => app.toggle_phrase(tx),
        KeyCode::Char('[') => app.nudge_bpm(-1.0, tx),
        KeyCode::Char(']') => app.nudge_bpm(1.0, tx),
        KeyCode::Char('{') => app.nudge_bpm(-10.0, tx),
        KeyCode::Char('}') => app.nudge_bpm(10.0, tx),
        _ => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn standalone() -> LinkStatus {
        LinkStatus {
            connected: false,
            strip: None,
            blocks_sent: 0,
            blocks_dropped: 0,
            messages_dropped: 0,
            connections: 0,
            last_refusal: None,
        }
    }

    fn app() -> App {
        App::new(
            &KeyReleases::Reported,
            false,
            standalone(),
            Arc::new(AudioStats::default()),
        )
    }

    #[test]
    fn piano_maps_an_octave_and_a_third() {
        let app = app();
        assert_eq!(app.note_for('a'), Some(60));
        assert_eq!(app.note_for('k'), Some(72));
        assert_eq!(app.note_for(';'), Some(76));
        assert_eq!(app.note_for('q'), None);
    }

    #[test]
    fn repeat_does_not_retrigger() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let mut app = app();
        app.press_note('a', &tx);
        app.repeat_note('a');
        assert_eq!(rx.try_iter().count(), 1);
        app.release_note('a', &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(AudioCommand::NoteOff { note: 60 })
        ));
    }

    #[test]
    fn auto_release_without_key_up_events() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let mut app = App::new(
            &KeyReleases::Unsupported,
            false,
            standalone(),
            Arc::new(AudioStats::default()),
        );
        app.press_note('a', &tx);
        app.held[0].last -= app.repeat_delay * 3;
        app.expire_held(&tx);
        assert!(app.held.is_empty());
        assert!(
            rx.try_iter()
                .any(|c| matches!(c, AudioCommand::NoteOff { note: 60 }))
        );
    }

    #[test]
    fn focus_moves_down_the_panel_and_stops_at_the_ends() {
        let mut app = app();
        app.move_focus(-1);
        assert_eq!(app.focus, 0);
        app.move_focus(100);
        assert_eq!(app.focus, VOLUME_ROW);
        app.move_focus(-1);
        assert_eq!(app.focus, VOLUME_ROW - 1);
    }

    #[test]
    fn volume_row_sends_the_master_level() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let mut app = app();
        app.focus = VOLUME_ROW;
        app.master = 0.5;
        app.adjust(5.0, &tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(AudioCommand::SetMaster(v)) if (v - 0.6).abs() < 1.0e-6
        ));
        app.adjust(1000.0, &tx);
        assert_eq!(rx.try_recv(), Ok(AudioCommand::SetMaster(1.0)));
    }

    #[test]
    fn every_row_edits_its_own_control() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let mut app = app();
        app.patch = PATCHES[0];
        for (row, field) in ParamField::ALL.into_iter().enumerate() {
            app.patch.set(field, 0.5);
            app.focus = row;
            app.adjust(1.0, &tx);
            assert!((app.patch.get(field) - 0.52).abs() < 1.0e-6, "{field:?}");
            assert!(matches!(rx.try_recv(), Ok(AudioCommand::SetPatch(_))));
        }
    }

    #[test]
    fn arrows_change_values_and_shift_takes_big_steps() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let mut app = app();
        app.patch.set(ParamField::Decay, 0.5);
        let right = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);
        let big_left = KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT);
        assert!(!handle_key(&mut app, right, &tx));
        assert!((app.patch.decay - 0.52).abs() < 1.0e-6);
        assert!(!handle_key(&mut app, big_left, &tx));
        assert!((app.patch.decay - 0.42).abs() < 1.0e-6);
        assert_eq!(rx.try_iter().count(), 2);
    }

    #[test]
    fn editing_marks_patch_modified_and_reload_restores() {
        let (tx, _rx) = crossbeam_channel::bounded(16);
        let mut app = app();
        app.focus = 2;
        app.adjust(-1.0, &tx);
        assert_ne!(app.patch, PATCHES[0]);
        app.load_patch(0, &tx);
        assert_eq!(app.patch, PATCHES[0]);
    }

    fn no_release_app() -> App {
        let mut app = App::new(
            &KeyReleases::Unsupported,
            false,
            standalone(),
            Arc::new(AudioStats::default()),
        );
        app.repeat_delay = Duration::from_millis(300);
        app
    }

    #[test]
    fn quick_second_tap_retriggers() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let mut app = no_release_app();
        app.press_note('a', &tx);
        app.held[0].last -= Duration::from_millis(60);
        app.press_note('a', &tx);
        let cmds: Vec<_> = rx.try_iter().collect();
        assert_eq!(cmds.len(), 3, "{cmds:?}");
        assert!(matches!(cmds[2], AudioCommand::NoteOn { note: 60, .. }));
    }

    #[test]
    fn first_os_repeat_does_not_retrigger() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let mut app = no_release_app();
        app.press_note('a', &tx);
        app.held[0].last -= Duration::from_millis(300);
        app.press_note('a', &tx);
        app.press_note('a', &tx);
        assert_eq!(rx.try_iter().count(), 1);
        assert!(app.held[0].repeating);
    }

    #[test]
    fn release_with_ctrl_held_still_releases() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let mut app = app();
        app.press_note('a', &tx);
        let release = KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Release,
            state: crossterm::event::KeyEventState::NONE,
        };
        assert!(!handle_key(&mut app, release, &tx));
        assert!(app.held.is_empty());
        assert!(
            rx.try_iter()
                .any(|c| matches!(c, AudioCommand::NoteOff { note: 60 }))
        );
    }

    #[test]
    fn full_command_queue_is_counted_not_hidden() {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let mut app = app();
        app.press_note('a', &tx);
        app.press_note('s', &tx);
        app.release_all(&tx);
        assert_eq!(rx.try_iter().count(), 1);
        assert_eq!(app.commands_dropped, 2);
    }

    #[test]
    fn disconnected_audio_thread_is_counted() {
        let (tx, rx) = crossbeam_channel::bounded(4);
        drop(rx);
        let mut app = app();
        app.load_patch(1, &tx);
        assert_eq!(app.commands_dropped, 1);
    }

    #[test]
    fn phrase_toggle_follows_state() {
        let (tx, rx) = crossbeam_channel::bounded(4);
        let mut app = app();
        let n = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE);
        assert!(!handle_key(&mut app, n, &tx));
        assert_eq!(app.phrase, PhraseState::Absent);
        assert!(rx.try_recv().is_err());
        app.phrase = PhraseState::Stopped;
        assert!(!handle_key(&mut app, n, &tx));
        // Asking is not playing: the engine reports what happened.
        assert_eq!(app.phrase, PhraseState::Stopped);
        assert_eq!(rx.try_recv(), Ok(AudioCommand::PlayPhrase(true)));
        assert_eq!(app.status, "Phrase looping.");
        app.phrase = PhraseState::Looping;
        app.link.connected = true;
        assert!(!handle_key(&mut app, n, &tx));
        assert_eq!(rx.try_recv(), Ok(AudioCommand::PlayPhrase(false)));
        assert_eq!(app.status, "Asking the desk to stop.");
    }

    #[test]
    fn tempo_keys_ask_for_whole_tempos_in_range() {
        let (tx, rx) = crossbeam_channel::bounded(8);
        let mut app = app();
        let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        assert!(!handle_key(&mut app, key(']'), &tx));
        assert!(rx.try_recv().is_err(), "no phrase, no tempo");
        app.phrase = PhraseState::Stopped;
        app.bpm = 112.4;
        assert!(!handle_key(&mut app, key(']'), &tx));
        assert_eq!(rx.try_recv(), Ok(AudioCommand::SetBpm(113.0)));
        app.bpm = 295.0;
        assert!(!handle_key(&mut app, key('}'), &tx));
        assert_eq!(rx.try_recv(), Ok(AudioCommand::SetBpm(MAX_BPM)));
        app.bpm = MIN_BPM;
        assert!(!handle_key(&mut app, key('{'), &tx));
        assert_eq!(rx.try_recv(), Ok(AudioCommand::SetBpm(MIN_BPM)));
    }

    #[test]
    fn key_repeat_setting_parses() {
        assert_eq!(
            parse_initial_key_repeat("15\n"),
            Ok(Duration::from_millis(225))
        );
        // Clamped to a sane range.
        assert_eq!(
            parse_initial_key_repeat("1"),
            Ok(Duration::from_millis(100))
        );
        assert_eq!(
            parse_initial_key_repeat("100000"),
            Ok(Duration::from_secs(2))
        );
        assert!(parse_initial_key_repeat("fast").is_err());
    }

    #[test]
    fn out_of_range_notes_play_nothing() {
        let mut app = app();
        app.octave = 8;
        assert_eq!(app.note_for(';'), Some(124));
        app.octave = 9;
        assert_eq!(app.note_for(';'), None);
    }

    #[test]
    fn args_parse() {
        let args = |v: &[&str]| parse_args(v.iter().map(|s| (*s).to_owned()));
        assert_eq!(args(&[]).unwrap(), None);
        assert_eq!(
            args(&["--phrase", "c4 e"]).unwrap().as_deref(),
            Some("c4 e")
        );
        assert_eq!(args(&["--phrase=c4"]).unwrap().as_deref(), Some("c4"));
        assert!(args(&["--phrase"]).is_err());
        assert!(args(&["--bogus"]).is_err());
    }
}
