//! Jam mode: loop phrases on the kazoo-mix desk, locked to its song.
//!
//! `kazoo-play --jam` plugs into the desk like any instrument. It loops a
//! phrase of the text notation on the song's beat grid, following the desk's
//! play, stop and tempo to the frame. Each line read from stdin while it runs
//! is a new phrase, which takes over at the next loop boundary, so a player
//! (or an agent) can change the part live by writing lines to it.
//!
//! With `--drive` it plays no sound of its own: its notes go through the desk
//! to the other instruments, so the phrase plays whatever synth is plugged in.
//!
//! Without a desk it plays on its own at `--bpm` through the local device.
//!
//! The audio callback never allocates or frees: phrases are built on the
//! reader thread, handed in over a ring, and handed back out to be dropped.

use std::f32::consts::TAU;
use std::io::{self, BufRead};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use kazoo_core::ipc::client::HubMessage;
use kazoo_core::ipc::follow::{TransportFollower, beats_in};
use kazoo_core::ipc::link::{HubLink, HubLinkAudio, LinkConfig, LinkStatus, hub_link};
use kazoo_core::ipc::types::{NOTE_OFF, NOTE_ON, NoteEventMsg};
use kazoo_core::notation::{NotationError, parse_notation};
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::{HeapProd, HeapRb};

/// Largest block rendered at once, in frames; the link's block size.
const MAX_BLOCK_FRAMES: usize = 4096;

/// Voices sounding at once.
const VOICES: usize = 16;

/// Notes a phrase may hold.
pub const MAX_PHRASE_NOTES: usize = 256;

/// Beats in a bar: phrases loop in whole bars.
const BEATS_PER_BAR: f64 = 4.0;

/// Phrases waiting to take over, and phrases waiting to be dropped.
const PHRASE_BACKLOG: usize = 8;

/// Voice attack and release, in seconds.
const ATTACK_SECONDS: f32 = 0.004;
const RELEASE_SECONDS: f32 = 0.08;

/// One note of a looping phrase, in beats from the phrase's start.
#[derive(Debug, Clone, Copy, PartialEq)]
struct LoopNote {
    start: f64,
    end: f64,
    note: u8,
    velocity: u8,
}

/// A phrase ready to loop.
#[derive(Debug, Clone, PartialEq)]
pub struct Phrase {
    notes: Vec<LoopNote>,
    /// Loop length in beats: the phrase rounded up to whole bars.
    length: f64,
    /// The notation it came from, for reporting.
    text: String,
}

/// Why a phrase could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhraseError {
    /// The notation did not parse.
    Notation(NotationError),
    /// It holds no notes at all, or no time.
    Empty,
    /// More than [`MAX_PHRASE_NOTES`] notes.
    TooManyNotes(usize),
}

impl std::fmt::Display for PhraseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Notation(err) => write!(f, "token {}: {}", err.token_index + 1, err.message),
            Self::Empty => write!(f, "the phrase has no notes"),
            Self::TooManyNotes(n) => {
                write!(f, "{n} notes; a phrase holds at most {MAX_PHRASE_NOTES}")
            }
        }
    }
}

impl std::error::Error for PhraseError {}

impl Phrase {
    /// Build a phrase from text notation, with every note at `velocity`
    /// (0-127).
    ///
    /// # Errors
    ///
    /// See [`PhraseError`].
    pub fn parse(text: &str, velocity: u8) -> Result<Self, PhraseError> {
        let events = parse_notation(text).map_err(PhraseError::Notation)?;
        let total: f64 = events.iter().map(|event| event.duration_beats).sum();
        let mut notes = Vec::new();
        for event in &events {
            for &note in &event.notes {
                notes.push(LoopNote {
                    start: event.start_beats,
                    end: event.start_beats + event.duration_beats,
                    note: note.min(127),
                    velocity: velocity.min(127),
                });
            }
        }
        if notes.is_empty() || !(total.is_finite() && total > 0.0) {
            return Err(PhraseError::Empty);
        }
        if notes.len() > MAX_PHRASE_NOTES {
            return Err(PhraseError::TooManyNotes(notes.len()));
        }
        let bars = (total / BEATS_PER_BAR).ceil().max(1.0);
        Ok(Self {
            notes,
            length: bars * BEATS_PER_BAR,
            text: text.trim().to_string(),
        })
    }

    /// Loop length in beats.
    #[must_use]
    pub const fn length(&self) -> f64 {
        self.length
    }
}

/// A sounding voice.
#[derive(Debug, Clone, Copy, Default)]
struct Voice {
    note: Option<u8>,
    level: f32,
    phase: f32,
    step: f32,
    gain: f32,
    releasing: bool,
}

/// Counters the main thread reports.
#[derive(Debug, Default)]
pub struct JamStats {
    /// Phrases that took over.
    pub phrases_started: AtomicU64,
    /// Notes the desk would not take in `--drive` mode.
    pub notes_lost: AtomicU64,
    /// Desk transport changes that could not be followed.
    pub syncs_lost: AtomicU64,
    /// Phrases that could not be handed back to be dropped.
    pub phrases_stranded: AtomicU64,
}

/// Everything the audio callback owns.
struct JamEngine {
    hub: HubLinkAudio,
    follower: TransportFollower,
    incoming: ringbuf::HeapCons<Box<Phrase>>,
    retired: HeapProd<Box<Phrase>>,
    current: Option<Box<Phrase>>,
    next: Option<Box<Phrase>>,
    /// Song position of the next frame, while playing.
    beat: Option<f64>,
    bpm: f64,
    sample_rate: f32,
    voices: [Voice; VOICES],
    stereo: Vec<f32>,
    drive: bool,
    channel: u8,
    stats: Arc<JamStats>,
    attack: f32,
    release: f32,
}

impl JamEngine {
    fn render(&mut self, data: &mut [f32], channels: usize) {
        self.drain_hub();
        if let Some(phrase) = self.incoming.try_pop() {
            // A newer phrase replaces one still waiting its turn.
            if let Some(stale) = self.next.replace(phrase) {
                self.retire(stale);
            }
        }
        for chunk in data.chunks_mut(MAX_BLOCK_FRAMES * channels) {
            let frames = chunk.len() / channels;
            let first = self.hub.stream_frame();
            for frame in 0..frames {
                if let Some(change) = self.follower.due(first + frame as u64) {
                    self.bpm = change.bpm;
                    if change.beat.is_none() {
                        self.release_all();
                    }
                    self.beat = change.beat;
                }
                self.step_song();
                let sample = self.voice_sample();
                self.stereo[frame * 2] = sample;
                self.stereo[frame * 2 + 1] = sample;
                for target in &mut chunk[frame * channels..(frame + 1) * channels] {
                    *target = sample;
                }
            }
            // Frames at most MAX_BLOCK_FRAMES: the cast is lossless.
            if self
                .hub
                .send_audio(frames as u32, &self.stereo[..frames * 2])
            {
                // The desk is playing this strip: don't play it twice.
                chunk.fill(0.0);
            }
        }
    }

    fn drain_hub(&mut self) {
        while let Some(message) = self.hub.try_recv() {
            match message {
                HubMessage::TransportSync(sync) => {
                    if self.follower.schedule(&sync).is_err() {
                        self.stats.syncs_lost.fetch_add(1, Ordering::Relaxed);
                    }
                }
                // The jam player takes no notes or parameters; the link
                // handles the desk closing.
                HubMessage::NoteEvent(_)
                | HubMessage::ParameterChange(_)
                | HubMessage::Shutdown => {}
            }
        }
    }

    /// Advance the song one frame, starting and ending notes it crosses.
    fn step_song(&mut self) {
        let Some(beat) = self.beat else {
            return;
        };
        let step = beats_in(1.0, self.bpm, f64::from(self.sample_rate));
        let Some(length) = self.current.as_ref().map(|phrase| phrase.length) else {
            // Nothing playing yet: take the waiting phrase on a bar line.
            if self.next.is_some() && (beat / BEATS_PER_BAR).fract() < step / BEATS_PER_BAR {
                self.swap_phrase();
            }
            self.beat = Some(beat + step);
            return;
        };
        let position = beat.rem_euclid(length);
        // A new phrase takes over where the loop comes round.
        if position < step && self.next.is_some() {
            self.release_all();
            self.swap_phrase();
        }
        let window_end = position + step;
        let count = self.current.as_ref().map_or(0, |phrase| phrase.notes.len());
        for index in 0..count {
            let Some(note) = self.current.as_ref().map(|phrase| phrase.notes[index]) else {
                break;
            };
            let length = self.current.as_ref().map_or(length, |phrase| phrase.length);
            let end = note.end.rem_euclid(length);
            if crosses(end, position, window_end, length) {
                self.note_off(note.note);
            }
            if crosses(note.start, position, window_end, length) {
                self.note_on(note.note, note.velocity);
            }
        }
        self.beat = Some(beat + step);
    }

    fn swap_phrase(&mut self) {
        if let Some(next) = self.next.take() {
            if let Some(old) = self.current.replace(next) {
                self.retire(old);
            }
            self.stats.phrases_started.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn retire(&mut self, phrase: Box<Phrase>) {
        if let Err(phrase) = self.retired.try_push(phrase) {
            // Never expected: the main thread drains this every 50 ms.
            // Leaked rather than freed on the audio thread, and counted.
            self.stats.phrases_stranded.fetch_add(1, Ordering::Relaxed);
            std::mem::forget(phrase);
        }
    }

    fn note_on(&mut self, note: u8, velocity: u8) {
        if self.drive {
            self.send(NOTE_ON, note, velocity);
            return;
        }
        let slot = self
            .voices
            .iter()
            .position(|voice| voice.note.is_none())
            .or_else(|| self.voices.iter().position(|voice| voice.releasing))
            .unwrap_or(0);
        let freq = 440.0 * ((f32::from(note) - 69.0) / 12.0).exp2();
        self.voices[slot] = Voice {
            note: Some(note),
            level: 0.0,
            phase: 0.0,
            step: TAU * freq / self.sample_rate,
            gain: f32::from(velocity) / 127.0,
            releasing: false,
        };
    }

    fn note_off(&mut self, note: u8) {
        if self.drive {
            self.send(NOTE_OFF, note, 0);
            return;
        }
        for voice in &mut self.voices {
            if voice.note == Some(note) && !voice.releasing {
                voice.releasing = true;
            }
        }
    }

    fn release_all(&mut self) {
        if self.drive {
            for index in 0..self.voices.len() {
                if let Some(note) = self.voices[index].note.take() {
                    self.send(NOTE_OFF, note, 0);
                }
            }
            return;
        }
        for voice in &mut self.voices {
            voice.releasing = voice.note.is_some();
        }
    }

    /// Send a note through the desk, remembering it so it can be released.
    fn send(&mut self, kind: u8, note: u8, velocity: u8) {
        let event = NoteEventMsg {
            source: [0; 16],
            target: [0; 16],
            event_type: kind,
            channel: self.channel,
            note,
            velocity,
        };
        if self.hub.send_note(event).is_err() {
            self.stats.notes_lost.fetch_add(1, Ordering::Relaxed);
        }
        if kind == NOTE_ON {
            if let Some(slot) = self.voices.iter_mut().find(|voice| voice.note.is_none()) {
                slot.note = Some(note);
            }
        } else if let Some(slot) = self
            .voices
            .iter_mut()
            .find(|voice| voice.note == Some(note))
        {
            slot.note = None;
        }
    }

    fn voice_sample(&mut self) -> f32 {
        if self.drive {
            return 0.0;
        }
        let mut sum = 0.0_f32;
        for voice in &mut self.voices {
            if voice.note.is_none() {
                continue;
            }
            if voice.releasing {
                voice.level -= self.release;
                if voice.level <= 0.0 {
                    *voice = Voice::default();
                    continue;
                }
            } else {
                voice.level = (voice.level + self.attack).min(1.0);
            }
            let sine = voice.phase.sin();
            let saw = (voice.phase / TAU).mul_add(2.0, -1.0).tanh();
            let tone = sine.mul_add(0.7, saw * 0.3);
            sum = (tone * voice.level).mul_add(voice.gain, sum);
            voice.phase = (voice.phase + voice.step).rem_euclid(TAU);
        }
        kazoo_core::sanitize_sample((sum * 0.3).tanh())
    }
}

/// Whether a note time `at` falls in the frame window `[from, to)` of a loop
/// `length` beats long (the window may run past the loop's end).
fn crosses(at: f64, from: f64, to: f64, length: f64) -> bool {
    (at >= from && at < to) || (to > length && at + length >= from && at + length < to)
}

/// How the jam player was asked to run.
#[derive(Debug, Clone, PartialEq)]
pub struct JamOptions {
    /// Name on the desk strip.
    pub name: String,
    /// Tempo while not plugged into the desk.
    pub bpm: f64,
    /// MIDI channel for driven notes.
    pub channel: u8,
    /// Note velocity, 0-127.
    pub velocity: u8,
    /// Send notes to the other instruments instead of sounding.
    pub drive: bool,
    /// The first phrase, if given on the command line.
    pub phrase: Option<String>,
}

/// Run the jam player until stdin closes and the process is interrupted.
///
/// # Errors
///
/// Fails if the audio device or the desk link cannot be started, or the
/// first phrase is not valid.
pub fn run_jam(options: &JamOptions) -> Result<(), String> {
    let (device, config, channels) = open_output()?;
    let rate = config.sample_rate;

    let (link, hub_audio) = hub_link(LinkConfig::new(
        &options.name,
        2,
        rate,
        MAX_BLOCK_FRAMES as u32,
    ))
    .map_err(|err| format!("failed to start the desk link: {err}"))?;

    let (mut phrase_tx, incoming) = HeapRb::<Box<Phrase>>::new(PHRASE_BACKLOG).split();
    let (retired, mut retired_rx) = HeapRb::<Box<Phrase>>::new(PHRASE_BACKLOG * 2).split();
    if let Some(text) = &options.phrase {
        let phrase = Phrase::parse(text, options.velocity).map_err(|err| err.to_string())?;
        if phrase_tx.try_push(Box::new(phrase)).is_err() {
            return Err("the phrase queue is full".to_string());
        }
    }

    let stats = Arc::new(JamStats::default());
    // Plays on its own at --bpm until the desk says otherwise.
    let mut follower = TransportFollower::new(rate);
    let local = kazoo_core::ipc::types::TransportSyncMsg {
        state: kazoo_core::ipc::types::TRANSPORT_PLAYING,
        // Tempos are 20-300 BPM: f32 holds them exactly enough.
        bpm: options.bpm as f32,
        at_frame: kazoo_core::ipc::types::SYNC_NOW,
        beat: 0.0,
    };
    follower
        .schedule(&local)
        .map_err(|err| format!("bad starting tempo: {err}"))?;
    let rate_f32 = rate as f32;
    let mut engine = JamEngine {
        hub: hub_audio,
        follower,
        incoming,
        retired,
        current: None,
        next: None,
        beat: None,
        bpm: options.bpm,
        sample_rate: rate_f32,
        voices: [Voice::default(); VOICES],
        stereo: vec![0.0; MAX_BLOCK_FRAMES * 2],
        drive: options.drive,
        channel: options.channel.min(15),
        stats: Arc::clone(&stats),
        attack: 1.0 / (ATTACK_SECONDS * rate_f32),
        release: 1.0 / (RELEASE_SECONDS * rate_f32),
    };

    let failed = Arc::new(AtomicBool::new(false));
    let stream_failed = Arc::clone(&failed);
    let stream = device
        .build_output_stream(
            &config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| engine.render(data, channels),
            move |err| {
                eprintln!("kazoo-play: audio stream error: {err}");
                if matches!(
                    err,
                    cpal::StreamError::DeviceNotAvailable | cpal::StreamError::StreamInvalidated
                ) {
                    stream_failed.store(true, Ordering::Release);
                }
            },
            None,
        )
        .map_err(|err| format!("failed to build the output stream: {err}"))?;
    stream
        .play()
        .map_err(|err| format!("failed to start the output stream: {err}"))?;

    let velocity = options.velocity;
    let (line_tx, line_rx) = std::sync::mpsc::channel::<Result<Box<Phrase>, String>>();
    thread::Builder::new()
        .name("kazoo-play-stdin".to_string())
        .spawn(move || read_phrases(&line_tx, velocity))
        .map_err(|err| format!("failed to start the phrase reader: {err}"))?;

    eprintln!(
        "kazoo-play: jamming as {}; write phrases, one per line",
        options.name
    );
    let mut reporter = Reporter::default();
    loop {
        if failed.load(Ordering::Acquire) {
            return Err("the audio device went away".to_string());
        }
        while let Some(old) = retired_rx.try_pop() {
            drop(old);
        }
        take_lines(&line_rx, &mut phrase_tx);
        reporter.report(&link, &stats);
        thread::sleep(Duration::from_millis(50));
    }
}

/// The default output device and its f32 stream config, with its channel count.
fn open_output() -> Result<(cpal::Device, cpal::StreamConfig, usize), String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "no default output audio device found".to_string())?;
    let supported = device
        .default_output_config()
        .map_err(|err| format!("failed to query the output device: {err}"))?;
    if supported.sample_format() != cpal::SampleFormat::F32 {
        return Err(format!(
            "the output device uses {:?} samples; jam mode needs f32",
            supported.sample_format()
        ));
    }
    let config: cpal::StreamConfig = supported.into();
    let channels = usize::from(config.channels);
    if channels == 0 {
        return Err("the output device has no channels".to_string());
    }
    Ok((device, config, channels))
}

/// Hand each phrase that arrived on stdin to the audio thread.
fn take_lines(
    lines: &std::sync::mpsc::Receiver<Result<Box<Phrase>, String>>,
    phrase_tx: &mut HeapProd<Box<Phrase>>,
) {
    loop {
        match lines.try_recv() {
            Ok(Ok(phrase)) => {
                let text = phrase.text.clone();
                let bars = phrase.length() / BEATS_PER_BAR;
                match phrase_tx.try_push(phrase) {
                    Ok(()) => eprintln!("kazoo-play: next ({bars} bars): {text}"),
                    Err(_) => {
                        eprintln!("kazoo-play: too many phrases waiting; skipped: {text}");
                    }
                }
            }
            Ok(Err(message)) => eprintln!("kazoo-play: {message}"),
            // Nothing waiting, or stdin closed: when it closes the last
            // phrase keeps looping until the player is stopped.
            Err(
                std::sync::mpsc::TryRecvError::Empty | std::sync::mpsc::TryRecvError::Disconnected,
            ) => {
                return;
            }
        }
    }
}

/// Read phrases from stdin, one per line, and hand each over parsed.
fn read_phrases(out: &std::sync::mpsc::Sender<Result<Box<Phrase>, String>>, velocity: u8) {
    for line in io::stdin().lock().lines() {
        let message = match line {
            Ok(line) if line.trim().is_empty() => continue,
            Ok(line) => Phrase::parse(&line, velocity)
                .map(Box::new)
                .map_err(|err| format!("{err}: {}", line.trim())),
            Err(err) => Err(format!("reading stdin failed: {err}")),
        };
        // The receiver only goes away when the player is shutting down.
        if out.send(message).is_err() {
            return;
        }
    }
}

/// Tells the player what changed, once.
#[derive(Debug, Default)]
struct Reporter {
    last: Option<LinkStatus>,
    phrases: u64,
    notes_lost: u64,
    syncs_lost: u64,
    stranded: u64,
}

impl Reporter {
    fn report(&mut self, link: &HubLink, stats: &JamStats) {
        let now_linked = link.status();
        let changed = self.last.as_ref().is_none_or(|last| {
            last.connected != now_linked.connected
                || last.strip != now_linked.strip
                || last.last_refusal != now_linked.last_refusal
        });
        if changed {
            match (
                now_linked.connected,
                now_linked.strip,
                &now_linked.last_refusal,
            ) {
                (true, Some(strip), _) => {
                    eprintln!("kazoo-play: on the desk, strip {}", u16::from(strip) + 1);
                }
                (_, _, Some(why)) => eprintln!("kazoo-play: playing on my own ({why})"),
                _ => eprintln!("kazoo-play: playing on my own (no desk yet)"),
            }
            self.last = Some(now_linked);
        }
        let phrases = stats.phrases_started.load(Ordering::Relaxed);
        if phrases != self.phrases {
            eprintln!("kazoo-play: phrase {phrases} playing");
            self.phrases = phrases;
        }
        for (label, counter, seen) in [
            (
                "notes the desk would not take",
                &stats.notes_lost,
                &mut self.notes_lost,
            ),
            (
                "desk transport changes missed",
                &stats.syncs_lost,
                &mut self.syncs_lost,
            ),
            (
                "phrases leaked",
                &stats.phrases_stranded,
                &mut self.stranded,
            ),
        ] {
            let now = counter.load(Ordering::Relaxed);
            if now != *seen {
                eprintln!("kazoo-play: {now} {label}");
                *seen = now;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phrases_round_up_to_whole_bars() {
        let phrase = Phrase::parse("c4/4 e4/4 g4/4", 100).unwrap();
        assert!((phrase.length() - 4.0).abs() < f64::EPSILON);
        let phrase = Phrase::parse("c4/1 e4/4", 100).unwrap();
        assert!((phrase.length() - 8.0).abs() < f64::EPSILON);
        assert_eq!(Phrase::parse("r/4", 100), Err(PhraseError::Empty));
        assert!(matches!(
            Phrase::parse("c4/4 zz", 100),
            Err(PhraseError::Notation(_))
        ));
    }

    #[test]
    fn windows_catch_notes_across_the_loop_end() {
        assert!(crosses(0.0, 3.99, 4.01, 4.0));
        assert!(crosses(2.0, 1.99, 2.01, 4.0));
        assert!(!crosses(2.0, 2.01, 2.03, 4.0));
    }
}
