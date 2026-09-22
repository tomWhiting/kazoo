//! kazoo-dx — four-operator FM synthesizer for the terminal.
//!
//! Plays from the computer keyboard, from the kazoo hub (so kazoo-arp can
//! drive it), or from a looped `kazoo-play` notation phrase given with
//! `--phrase "<notation>"`. All sound is sine-wave phase modulation computed in
//! the cpal output callback. Nothing is sampled.

mod ipc;
mod phrase;
mod synth;
mod ui;

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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

use ipc::HubLink;
use phrase::Phrase;
use synth::{ALGORITHMS, FmSynth, OPERATORS, OperatorField, PATCHES, Patch, SCOPE_LEN, VOICES};

/// Largest block rendered in one pass. Device buffers larger than this are
/// rendered in several passes, so every frame is always written.
const MAX_BLOCK_FRAMES: usize = 1_024;
/// Display snapshots per second pushed from the audio thread.
const DISPLAY_HZ: f32 = 60.0;
/// Without key-release reporting, a key auto-repeating is released this long
/// after its last repeat.
const REPEAT_HOLD: Duration = Duration::from_millis(140);
/// Fallback OS key-repeat delay when the system setting can't be read.
const DEFAULT_REPEAT_DELAY: Duration = Duration::from_millis(250);
/// Default phrase tempo.
const PHRASE_BPM: f64 = 112.0;

/// UI to audio-thread messages. Every variant is `Copy`, so sending never allocates.
#[derive(Debug, Clone, Copy)]
enum AudioCommand {
    NoteOn { note: u8, velocity: u8 },
    NoteOff { note: u8 },
    AllNotesOff,
    SetPatch(Patch),
    SetMaster(f32),
    PhrasePlaying(bool),
}

/// Audio-thread to UI snapshot.
#[derive(Debug, Clone, Copy)]
pub struct DisplaySnapshot {
    pub scope: [f32; SCOPE_LEN],
    pub voices: [Option<(u8, bool)>; VOICES],
    pub peak: f32,
}

impl DisplaySnapshot {
    const EMPTY: Self = Self {
        scope: [0.0; SCOPE_LEN],
        voices: [None; VOICES],
        peak: 0.0,
    };
}

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
        Some(text) => Some(Phrase::parse(text, PHRASE_BPM, sample_rate).map_err(|e| {
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

    #[allow(clippy::cast_possible_truncation)]
    let hub_link = HubLink::new(2, sample_rate, MAX_BLOCK_FRAMES as u32);
    let hub_connected = hub_link.connection_flag();
    let phrase_flag = Arc::new(AtomicBool::new(false));

    let stream = build_audio_stream(
        &device,
        &config.into(),
        AudioSetup {
            sample_rate: sample_rate as f32,
            channels,
            cmd_rx,
            display_tx,
            hub_link,
            phrase,
            phrase_flag: Arc::clone(&phrase_flag),
        },
    )?;
    stream.play()?;

    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let release_events = terminal::supports_keyboard_enhancement().unwrap_or(false)
        && execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::REPORT_EVENT_TYPES)
        )
        .is_ok();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(release_events, has_phrase, hub_connected);
    let result = run_event_loop(&mut terminal, &mut app, &cmd_tx, &display_rx, &phrase_flag);

    // Always restore the terminal, even if the loop failed.
    if release_events {
        let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    }
    let _ = terminal::disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();
    drop(stream);

    result
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
                "kazoo-dx: four-operator FM synth\n\nUSAGE: kazoo-dx [--phrase \"<kazoo-play notation>\"]\n\nPress ? inside the app for keys."
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

/// Everything the audio callback takes ownership of.
struct AudioSetup {
    sample_rate: f32,
    channels: usize,
    cmd_rx: crossbeam_channel::Receiver<AudioCommand>,
    display_tx: crossbeam_channel::Sender<DisplaySnapshot>,
    hub_link: HubLink,
    phrase: Option<Phrase>,
    /// Whether the phrase is looping; the hub transport can change it too.
    phrase_flag: Arc<AtomicBool>,
}

fn build_audio_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    setup: AudioSetup,
) -> color_eyre::Result<cpal::Stream> {
    let AudioSetup {
        sample_rate,
        channels,
        cmd_rx,
        display_tx,
        mut hub_link,
        mut phrase,
        phrase_flag,
    } = setup;

    let mut synth = FmSynth::new(sample_rate);
    let mut mono = vec![0.0_f32; MAX_BLOCK_FRAMES];
    let mut stereo = vec![0.0_f32; MAX_BLOCK_FRAMES * 2];
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let display_interval = (sample_rate / DISPLAY_HZ).max(1.0) as usize;
    let mut since_display = 0_usize;
    let mut peak = 0.0_f32;
    let mut phrase_playing = false;

    let stream = device.build_output_stream(
        config,
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
            while let Ok(cmd) = cmd_rx.try_recv() {
                match cmd {
                    AudioCommand::NoteOn { note, velocity } => synth.note_on(note, velocity),
                    AudioCommand::NoteOff { note } => synth.note_off(note),
                    AudioCommand::AllNotesOff => synth.all_notes_off(),
                    AudioCommand::SetPatch(patch) => synth.set_patch(patch),
                    AudioCommand::SetMaster(value) => synth.set_master(value),
                    AudioCommand::PhrasePlaying(on) => {
                        phrase_playing = on && phrase.is_some();
                        phrase_flag.store(phrase_playing, Ordering::Release);
                        if let Some(p) = phrase.as_mut() {
                            p.rewind();
                        }
                        synth.all_notes_off();
                    }
                }
            }

            while let Some(msg) = hub_link.try_recv() {
                phrase_playing =
                    handle_hub_message(&msg, &mut synth, phrase.as_mut(), phrase_playing);
                phrase_flag.store(phrase_playing, Ordering::Release);
            }

            for chunk in data.chunks_mut(MAX_BLOCK_FRAMES * channels) {
                let frames = chunk.len() / channels;
                for sample in &mut mono[..frames] {
                    if phrase_playing {
                        if let Some(p) = phrase.as_mut() {
                            p.advance(&mut synth);
                        }
                    }
                    *sample = synth.process();
                    peak = peak.max(sample.abs());
                }

                if hub_link.is_connected() {
                    for (i, &s) in mono[..frames].iter().enumerate() {
                        stereo[i * 2] = s;
                        stereo[i * 2 + 1] = s;
                    }
                    #[allow(clippy::cast_possible_truncation)]
                    hub_link.send_audio(frames as u32, &stereo[..frames * 2]);
                }

                for (frame, &s) in chunk.chunks_mut(channels).zip(mono[..frames].iter()) {
                    frame.fill(s);
                }
                // A trailing partial frame (never expected) is silenced.
                let written = frames * channels;
                chunk[written..].fill(0.0);

                since_display += frames;
                if since_display >= display_interval {
                    since_display = 0;
                    let (ring, pos) = synth.scope();
                    let mut scope = [0.0_f32; SCOPE_LEN];
                    let (older, newer) = ring.split_at(pos);
                    scope[..newer.len()].copy_from_slice(newer);
                    scope[newer.len()..].copy_from_slice(older);
                    let _ = display_tx.try_send(DisplaySnapshot {
                        scope,
                        voices: synth.voice_states(),
                        peak,
                    });
                    peak = 0.0;
                }
            }
        },
        |err| eprintln!("audio stream error: {err}"),
        None,
    )?;

    Ok(stream)
}

/// Apply one hub message in the audio callback. Returns whether the phrase
/// should be looping afterwards. Never allocates.
fn handle_hub_message(
    msg: &kazoo_core::ipc::client::HubMessage,
    synth: &mut FmSynth,
    phrase: Option<&mut Phrase>,
    phrase_playing: bool,
) -> bool {
    use kazoo_core::ipc::client::HubMessage;
    use kazoo_core::ipc::types::{NOTE_OFF, NOTE_ON, TRANSPORT_PLAYING, TRANSPORT_RECORDING};

    match msg {
        HubMessage::NoteEvent(event) => {
            match event.event_type {
                NOTE_ON => synth.note_on(event.note, event.velocity),
                NOTE_OFF => synth.note_off(event.note),
                _ => {}
            }
            phrase_playing
        }
        // The phrase follows the hub: its tempo, and its play/stop.
        HubMessage::TransportSync(sync) => {
            let Some(p) = phrase else {
                return false;
            };
            p.set_bpm(f64::from(sync.bpm));
            let rolling = matches!(sync.state, TRANSPORT_PLAYING | TRANSPORT_RECORDING);
            if rolling != phrase_playing {
                p.rewind();
                synth.all_notes_off();
            }
            rolling
        }
        HubMessage::ParameterChange(_) | HubMessage::Shutdown => phrase_playing,
    }
}

/// Which part of the screen the arrow keys edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// Global row: algorithm, feedback, master.
    Global(usize),
    /// Operator grid cell: (operator, field).
    Operator(usize, usize),
}

pub const GLOBAL_FIELDS: [&str; 5] = ["algorithm", "feedback", "lfo rate", "vibrato", "volume"];

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

/// UI-thread state. The flags are independent display toggles, not a state machine.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug)]
pub struct App {
    pub patch: Patch,
    pub patch_index: usize,
    pub master: f32,
    pub focus: Focus,
    pub octave: i8,
    pub velocity: u8,
    pub show_help: bool,
    pub release_events: bool,
    pub has_phrase: bool,
    pub phrase_playing: bool,
    pub display: DisplaySnapshot,
    pub hub_connected: Arc<AtomicBool>,
    pub status: String,
    held: Vec<HeldKey>,
    /// The OS delay before a held key starts repeating. Used to tell a held
    /// key from a quick second tap when the terminal can't report releases.
    repeat_delay: Duration,
}

impl App {
    fn new(release_events: bool, has_phrase: bool, hub_connected: Arc<AtomicBool>) -> Self {
        let status = if release_events {
            "Keyboard reports key release: notes hold while you hold the key.".to_owned()
        } else {
            "Terminal can't report key release: notes release automatically.".to_owned()
        };
        Self {
            patch: PATCHES[0],
            patch_index: 0,
            master: 0.8,
            focus: Focus::Operator(0, 0),
            octave: 4,
            velocity: 100,
            show_help: false,
            release_events,
            has_phrase,
            phrase_playing: false,
            display: DisplaySnapshot::EMPTY,
            hub_connected,
            status,
            held: Vec::with_capacity(32),
            repeat_delay: system_repeat_delay(),
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

    fn send(tx: &crossbeam_channel::Sender<AudioCommand>, cmd: AudioCommand) {
        // A full queue means the audio thread is stalled. Dropping one UI
        // command is safer than blocking the UI; state is resent on the next edit.
        let _ = tx.try_send(cmd);
    }

    fn load_patch(&mut self, index: usize, tx: &crossbeam_channel::Sender<AudioCommand>) {
        self.patch_index = index % PATCHES.len();
        self.patch = PATCHES[self.patch_index];
        self.status = format!("Loaded patch {}: {}", self.patch_index + 1, self.patch.name);
        Self::send(tx, AudioCommand::SetPatch(self.patch));
    }

    fn adjust(&mut self, steps: f32, tx: &crossbeam_channel::Sender<AudioCommand>) {
        match self.focus {
            Focus::Global(0) => {
                let count = ALGORITHMS.len() as f32;
                let next = (self.patch.algorithm as f32 + steps.signum()).rem_euclid(count);
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                {
                    self.patch.algorithm = next as usize;
                }
            }
            Focus::Global(1) => {
                self.patch.feedback = steps.mul_add(0.02, self.patch.feedback).clamp(0.0, 1.0);
            }
            Focus::Global(2) => {
                self.patch.lfo_rate = steps.mul_add(0.02, self.patch.lfo_rate).clamp(0.0, 1.0);
            }
            Focus::Global(3) => {
                self.patch.vibrato = steps.mul_add(0.02, self.patch.vibrato).clamp(0.0, 1.0);
            }
            Focus::Global(_) => {
                self.master = steps.mul_add(0.02, self.master).clamp(0.0, 1.0);
                Self::send(tx, AudioCommand::SetMaster(self.master));
                return;
            }
            Focus::Operator(op, field) => {
                let field = OperatorField::ALL[field];
                let params = &mut self.patch.operators[op];
                let value = steps.mul_add(field.step(), params.get(field));
                params.set(field, value);
            }
        }
        Self::send(tx, AudioCommand::SetPatch(self.patch));
    }

    /// Move the edit cursor. The global row sits above operator rows 1 to 4.
    fn move_focus(&mut self, rows: isize, cols: isize) {
        // Row 0 is the global row; rows 1..=OPERATORS are operators.
        let (row, col) = match self.focus {
            Focus::Global(c) => (0, c),
            Focus::Operator(o, c) => (o + 1, c),
        };
        let row = row.saturating_add_signed(rows).min(OPERATORS);
        let col = col.saturating_add_signed(cols);
        self.focus = if row == 0 {
            Focus::Global(col.min(GLOBAL_FIELDS.len() - 1))
        } else {
            Focus::Operator(row - 1, col.min(OperatorField::ALL.len() - 1))
        };
    }

    fn set_status(&mut self, text: &str) {
        self.status.clear();
        self.status.push_str(text);
    }

    fn note_for(&self, key: char) -> Option<u8> {
        let offset = piano_offset(key)?;
        let note = i16::from(self.octave + 1) * 12 + i16::from(offset);
        u8::try_from(note).ok().filter(|n| *n <= 127)
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
                Self::send(tx, AudioCommand::NoteOff { note: held.note });
            }
            held.last = now;
            held.repeating = false;
            held.sounding = true;
            Self::send(
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
        Self::send(tx, AudioCommand::NoteOn { note, velocity });
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
                Self::send(tx, AudioCommand::NoteOff { note: held.note });
            }
        }
    }

    fn expire_held(&mut self, tx: &crossbeam_channel::Sender<AudioCommand>) {
        if self.release_events {
            return;
        }
        let now = Instant::now();
        let first_hold = self.repeat_delay * 2;
        self.held.retain_mut(|h| {
            let idle = now.duration_since(h.last);
            let limit = if h.repeating { REPEAT_HOLD } else { first_hold };
            if h.sounding && idle >= limit {
                let _ = tx.try_send(AudioCommand::NoteOff { note: h.note });
                h.sounding = false;
            }
            // Forget a silent key once no repeat could still be on its way.
            h.sounding || idle < first_hold
        });
    }

    fn release_all(&mut self, tx: &crossbeam_channel::Sender<AudioCommand>) {
        self.held.clear();
        Self::send(tx, AudioCommand::AllNotesOff);
    }
}

/// The OS key-repeat delay. On macOS this is the `InitialKeyRepeat` setting
/// (units of 15 ms); elsewhere, or if unset, a typical default.
fn system_repeat_delay() -> Duration {
    let from_macos = std::process::Command::new("defaults")
        .args(["read", "-g", "InitialKeyRepeat"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .and_then(|text| text.trim().parse::<u64>().ok())
        .map(|ticks| Duration::from_millis(ticks.saturating_mul(15)));
    from_macos
        .unwrap_or(DEFAULT_REPEAT_DELAY)
        .clamp(Duration::from_millis(100), Duration::from_secs(2))
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
    cmd_tx: &crossbeam_channel::Sender<AudioCommand>,
    display_rx: &crossbeam_channel::Receiver<DisplaySnapshot>,
    phrase_flag: &AtomicBool,
) -> color_eyre::Result<()> {
    loop {
        while let Ok(snapshot) = display_rx.try_recv() {
            app.display = snapshot;
        }
        app.phrase_playing = phrase_flag.load(Ordering::Acquire);
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
        KeyCode::Up => app.move_focus(-1, 0),
        KeyCode::Down => app.move_focus(1, 0),
        KeyCode::Left => app.move_focus(0, -1),
        KeyCode::Right => app.move_focus(0, 1),
        KeyCode::Char('-' | '_') => app.adjust(if coarse { -5.0 } else { -1.0 }, tx),
        KeyCode::Char('=' | '+') => app.adjust(if coarse { 5.0 } else { 1.0 }, tx),
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
        KeyCode::Char('n' | 'N') => {
            if app.has_phrase {
                app.phrase_playing = !app.phrase_playing;
                App::send(tx, AudioCommand::PhrasePlaying(app.phrase_playing));
                app.set_status(if app.phrase_playing {
                    "Phrase looping."
                } else {
                    "Phrase stopped."
                });
            } else {
                app.set_status("No phrase loaded. Start with --phrase \"c4/8 e4/8 g4/4\".");
            }
        }
        _ => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App::new(true, false, Arc::new(AtomicBool::new(false)))
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
        let mut app = App::new(false, false, Arc::new(AtomicBool::new(false)));
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
    fn focus_moves_between_global_and_grid() {
        let mut app = app();
        app.move_focus(-1, 0);
        assert_eq!(app.focus, Focus::Global(0));
        app.move_focus(0, 10);
        assert_eq!(app.focus, Focus::Global(GLOBAL_FIELDS.len() - 1));
        app.move_focus(10, 10);
        assert_eq!(
            app.focus,
            Focus::Operator(OPERATORS - 1, OperatorField::ALL.len() - 1)
        );
    }

    #[test]
    fn algorithm_wraps() {
        let (tx, _rx) = crossbeam_channel::bounded(16);
        let mut app = app();
        app.focus = Focus::Global(0);
        app.patch.algorithm = 0;
        app.adjust(-1.0, &tx);
        assert_eq!(app.patch.algorithm, ALGORITHMS.len() - 1);
    }

    fn note_msg(event_type: u8, note: u8, velocity: u8) -> kazoo_core::ipc::client::HubMessage {
        kazoo_core::ipc::client::HubMessage::NoteEvent(kazoo_core::ipc::types::NoteEventMsg {
            source: [0; 16],
            target: [0; 16],
            event_type,
            channel: 0,
            note,
            velocity,
        })
    }

    fn transport_msg(state: u8, bpm: f32) -> kazoo_core::ipc::client::HubMessage {
        kazoo_core::ipc::client::HubMessage::TransportSync(
            kazoo_core::ipc::types::TransportSyncMsg {
                state,
                bpm,
                position: 0,
                timestamp: 0,
            },
        )
    }

    #[test]
    fn hub_notes_play_the_synth() {
        use kazoo_core::ipc::types::{NOTE_OFF, NOTE_ON};
        let mut synth = FmSynth::new(48_000.0);
        handle_hub_message(&note_msg(NOTE_ON, 64, 100), &mut synth, None, false);
        assert!(
            synth
                .voice_states()
                .iter()
                .flatten()
                .any(|&(n, held)| n == 64 && held)
        );
        handle_hub_message(&note_msg(NOTE_OFF, 64, 0), &mut synth, None, false);
        assert!(
            synth
                .voice_states()
                .iter()
                .flatten()
                .all(|&(_, held)| !held)
        );
    }

    #[test]
    fn hub_transport_starts_and_stops_phrase() {
        use kazoo_core::ipc::types::{TRANSPORT_PAUSED, TRANSPORT_PLAYING, TRANSPORT_STOPPED};
        let mut synth = FmSynth::new(48_000.0);
        let mut phrase = Phrase::parse("c4/4 e4/4", 120.0, 48_000).expect("valid");
        let playing = handle_hub_message(
            &transport_msg(TRANSPORT_PLAYING, 140.0),
            &mut synth,
            Some(&mut phrase),
            false,
        );
        assert!(playing);
        let playing = handle_hub_message(
            &transport_msg(TRANSPORT_PAUSED, 140.0),
            &mut synth,
            Some(&mut phrase),
            playing,
        );
        assert!(!playing);
        // Without a phrase, transport never turns looping on.
        assert!(!handle_hub_message(
            &transport_msg(TRANSPORT_PLAYING, 140.0),
            &mut synth,
            None,
            false
        ));
        assert!(!handle_hub_message(
            &transport_msg(TRANSPORT_STOPPED, 140.0),
            &mut synth,
            Some(&mut phrase),
            false
        ));
    }

    #[test]
    fn editing_marks_patch_modified_and_reload_restores() {
        let (tx, _rx) = crossbeam_channel::bounded(16);
        let mut app = app();
        app.focus = Focus::Operator(0, 2);
        app.adjust(-1.0, &tx);
        assert_ne!(app.patch, PATCHES[0]);
        app.load_patch(0, &tx);
        assert_eq!(app.patch, PATCHES[0]);
    }

    fn no_release_app() -> App {
        let mut app = App::new(false, false, Arc::new(AtomicBool::new(false)));
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
