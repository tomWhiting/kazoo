//! kazoo-play — pipe text notation into timestamped musical events or sound.
//!
//! `kazoo-play` is the first terminal-native musical primitive: humans and
//! agents can write compact notation, pipe it around, print timestamped events,
//! or play it immediately through the default audio device.

use std::env;
use std::f32::consts::TAU;
use std::fmt;
use std::io::{self, Read, Write};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use kazoo_core::notation::{NotationError, parse_note_events};
use kazoo_core::protocol::{NoteEvent, NoteEventKind};

mod jam;

const DEFAULT_BPM: f64 = 120.0;
const DEFAULT_SAMPLE_RATE: u32 = 48_000;
const DEFAULT_CHANNEL: u8 = 0;
const RELEASE_FRAMES: u64 = 12_000;

#[derive(Debug, Clone, PartialEq)]
struct Options {
    bpm: f64,
    sample_rate: u32,
    channel: u8,
    format: OutputFormat,
    mode: Mode,
    notation: Option<String>,
    name: String,
    velocity: u8,
    drive: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            bpm: DEFAULT_BPM,
            sample_rate: DEFAULT_SAMPLE_RATE,
            channel: DEFAULT_CHANNEL,
            format: OutputFormat::Lines,
            mode: Mode::Events,
            notation: None,
            name: "kazoo-play".to_string(),
            velocity: 100,
            drive: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Lines,
    Tsv,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Events,
    Play,
    Jam,
}

fn main() -> ExitCode {
    match run(env::args().skip(1)) {
        Ok(output) => match write_output(&mut io::stdout().lock(), &output) {
            Ok(()) => ExitCode::SUCCESS,
            // The reader went away (for example `| head`): nothing is left to
            // tell, but the run did not deliver everything, so it is not a success.
            Err(err) if err.kind() == io::ErrorKind::BrokenPipe => ExitCode::from(1),
            Err(err) => {
                eprintln!("kazoo-play: failed to write output: {err}");
                ExitCode::from(1)
            }
        },
        Err(err) => {
            eprintln!("kazoo-play: {err}");
            ExitCode::from(2)
        }
    }
}

/// Write the whole output and flush it, so a write failure is reported
/// instead of panicking the way `print!` does.
fn write_output(out: &mut impl Write, output: &str) -> io::Result<()> {
    out.write_all(output.as_bytes())?;
    out.flush()
}

fn run(args: impl IntoIterator<Item = String>) -> Result<String, String> {
    let options = parse_args(args)?;
    if options.mode == Mode::Jam {
        jam::run_jam(&jam::JamOptions {
            name: options.name.clone(),
            bpm: options.bpm,
            channel: options.channel,
            velocity: options.velocity,
            drive: options.drive,
            phrase: options.notation,
        })?;
        return Ok(String::new());
    }
    let notation = match options.notation.as_ref() {
        Some(notation) => notation.clone(),
        None => read_stdin()?,
    };
    let events = parse_note_events(&notation, options.bpm, options.sample_rate, options.channel)
        .map_err(|err| format_notation_error(&err))?;

    match options.mode {
        Mode::Events => Ok(format_events(&events, options.format)),
        Mode::Play => {
            play_events(&events, options.sample_rate)?;
            Ok(String::new())
        }
        Mode::Jam => Err("jam mode is handled above".to_string()),
    }
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut options = Options::default();
    let mut notation_parts = Vec::new();
    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => return Err(help()),
            "--play" => options.mode = Mode::Play,
            "--events" => options.mode = Mode::Events,
            "--jam" => options.mode = Mode::Jam,
            "--drive" => options.drive = true,
            "--name" => {
                options.name = iter
                    .next()
                    .ok_or_else(|| "missing value after --name".to_string())?;
            }
            "--velocity" => {
                let velocity: u8 = parse_value(&mut iter, "--velocity")?;
                if velocity > 127 {
                    return Err("--velocity must be 0-127".to_string());
                }
                options.velocity = velocity;
            }
            "--bpm" => {
                options.bpm = parse_value(&mut iter, "--bpm")?;
                if !options.bpm.is_finite() || options.bpm <= 0.0 {
                    return Err("--bpm must be positive".to_string());
                }
            }
            "--sample-rate" | "--rate" => {
                options.sample_rate = parse_value(&mut iter, "--sample-rate")?;
                if options.sample_rate == 0 {
                    return Err("--sample-rate must be non-zero".to_string());
                }
            }
            "--channel" | "-c" => {
                options.channel = parse_value::<u8>(&mut iter, "--channel")?.min(15);
            }
            "--format" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "missing value after --format".to_string())?;
                options.format = match value.as_str() {
                    "lines" => OutputFormat::Lines,
                    "tsv" => OutputFormat::Tsv,
                    _ => return Err("--format must be 'lines' or 'tsv'".to_string()),
                };
            }
            "--" => {
                notation_parts.extend(iter.by_ref());
                break;
            }
            other if other.starts_with('-') => return Err(format!("unknown option: {other}")),
            _ => notation_parts.push(arg),
        }
    }

    if !notation_parts.is_empty() {
        options.notation = Some(notation_parts.join(" "));
    }

    Ok(options)
}

fn parse_value<T>(iter: &mut impl Iterator<Item = String>, flag: &str) -> Result<T, String>
where
    T: std::str::FromStr,
{
    let value = iter
        .next()
        .ok_or_else(|| format!("missing value after {flag}"))?;
    value
        .parse::<T>()
        .map_err(|_| format!("invalid value for {flag}: {value}"))
}

fn read_stdin() -> Result<String, String> {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|err| format!("failed to read stdin: {err}"))?;
    if input.trim().is_empty() {
        return Err(help());
    }
    Ok(input)
}

fn format_notation_error(err: &NotationError) -> String {
    format!("token {}: {}", err.token_index + 1, err.message)
}

fn play_events(events: &[NoteEvent], requested_sample_rate: u32) -> Result<(), String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "no default output audio device found".to_string())?;
    let supported = device
        .default_output_config()
        .map_err(|err| format!("failed to query default output config: {err}"))?;
    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let output_rate = config.sample_rate;
    let render = Arc::new(RenderState::new(events, requested_sample_rate, output_rate));
    let done = Arc::clone(&render.done);
    let failed = Arc::new(AtomicBool::new(false));

    let stream = match sample_format {
        cpal::SampleFormat::F32 => {
            build_stream::<f32>(&device, &config, Arc::clone(&render), Arc::clone(&failed))?
        }
        cpal::SampleFormat::I16 => {
            build_stream::<i16>(&device, &config, Arc::clone(&render), Arc::clone(&failed))?
        }
        cpal::SampleFormat::U16 => {
            build_stream::<u16>(&device, &config, Arc::clone(&render), Arc::clone(&failed))?
        }
        other => return Err(format!("unsupported output sample format: {other:?}")),
    };
    stream
        .play()
        .map_err(|err| format!("failed to start output stream: {err}"))?;

    // Without this check a stream that dies (device unplugged) would never
    // reach the end of the phrase, and the process would wait forever.
    while !done.load(Ordering::Acquire) {
        if failed.load(Ordering::Acquire) {
            return Err("output stream stopped before the phrase finished".to_string());
        }
        thread::sleep(Duration::from_millis(10));
    }
    drop(stream);
    Ok(())
}

/// Whether a stream error means no more audio will be rendered.
const fn is_fatal(err: &cpal::StreamError) -> bool {
    matches!(
        err,
        cpal::StreamError::DeviceNotAvailable | cpal::StreamError::StreamInvalidated
    )
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    render: Arc<RenderState>,
    failed: Arc<AtomicBool>,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = usize::from(config.channels.max(1));
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                for frame in data.chunks_mut(channels) {
                    let sample = render.next_sample();
                    for target in frame {
                        *target = T::from_sample(sample);
                    }
                }
            },
            move |err| {
                eprintln!("kazoo-play stream error: {err}");
                if is_fatal(&err) {
                    failed.store(true, Ordering::Release);
                }
            },
            None,
        )
        .map_err(|err| format!("failed to build output stream: {err}"))
}

#[derive(Debug)]
struct RenderState {
    events: Vec<RenderEvent>,
    cursor: AtomicUsize,
    done: Arc<AtomicBool>,
    total_frames: usize,
    sample_rate: f32,
}

impl RenderState {
    fn new(events: &[NoteEvent], source_sample_rate: u32, output_sample_rate: u32) -> Self {
        let rate_ratio =
            f64::from(output_sample_rate.max(1)) / f64::from(source_sample_rate.max(1));
        let mut render_events = Vec::new();
        let mut last_frame = 0_u64;

        for (idx, event) in events.iter().enumerate() {
            let NoteEventKind::NoteOn { note, velocity } = event.kind else {
                continue;
            };
            let frame = scale_frame(event.frame, rate_ratio);
            let end_frame = find_note_off(events, idx + 1, event.channel, note).map_or_else(
                || frame + scale_frame(RELEASE_FRAMES, rate_ratio),
                |off| scale_frame(off, rate_ratio),
            );
            last_frame = last_frame.max(end_frame);
            render_events.push(RenderEvent {
                frame,
                end_frame,
                note,
                velocity,
            });
        }

        render_events.sort_by_key(|event| event.frame);
        let release = (RELEASE_FRAMES as f64 * rate_ratio).round() as usize;

        Self {
            events: render_events,
            cursor: AtomicUsize::new(0),
            done: Arc::new(AtomicBool::new(false)),
            total_frames: last_frame as usize + release + output_sample_rate as usize / 4,
            sample_rate: output_sample_rate as f32,
        }
    }

    fn next_sample(&self) -> f32 {
        let frame = self.cursor.fetch_add(1, Ordering::Relaxed);
        if frame >= self.total_frames {
            self.done.store(true, Ordering::Release);
            return 0.0;
        }

        let mut sample = 0.0_f32;
        for event in &self.events {
            if frame < event.frame as usize || frame >= event.end_frame as usize {
                continue;
            }
            sample += voice_sample(event, frame as u64, self.sample_rate);
        }
        // Non-finite input renders as silence, never as noise.
        kazoo_core::sanitize_sample((sample * 0.25).tanh())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct RenderEvent {
    frame: u64,
    end_frame: u64,
    note: u8,
    velocity: f32,
}

fn find_note_off(events: &[NoteEvent], start_idx: usize, channel: u8, note: u8) -> Option<u64> {
    events
        .iter()
        .skip(start_idx)
        .find_map(|event| match event.kind {
            NoteEventKind::NoteOff { note: off_note, .. }
                if off_note == note && event.channel == channel =>
            {
                Some(event.frame)
            }
            _ => None,
        })
}

fn scale_frame(frame: u64, rate_ratio: f64) -> u64 {
    (frame as f64 * rate_ratio).round() as u64
}

fn voice_sample(event: &RenderEvent, frame: u64, sample_rate: f32) -> f32 {
    let age = frame.saturating_sub(event.frame) as f32;
    let duration = event.end_frame.saturating_sub(event.frame).max(1) as f32;
    let release_start = duration * 0.82;
    let amp = if age < 256.0 {
        age / 256.0
    } else if age > release_start {
        (1.0 - (age - release_start) / (duration - release_start).max(1.0)).max(0.0)
    } else {
        1.0
    };
    let freq = midi_frequency(event.note);
    let phase = TAU * freq * age / sample_rate;
    let sine = phase.sin();
    let soft_saw = (phase / TAU).fract().mul_add(2.0, -1.0).tanh();
    sine.mul_add(0.75, soft_saw * 0.25) * amp * event.velocity
}

fn midi_frequency(note: u8) -> f32 {
    440.0 * ((f32::from(note) - 69.0) / 12.0).exp2()
}

fn format_events(events: &[NoteEvent], format: OutputFormat) -> String {
    EventListing { events, format }.to_string()
}

/// Every event, one per line, in the chosen format.
struct EventListing<'a> {
    events: &'a [NoteEvent],
    format: OutputFormat,
}

impl fmt::Display for EventListing<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for event in self.events {
            match self.format {
                OutputFormat::Lines => write_event_line(f, event)?,
                OutputFormat::Tsv => write_event_tsv(f, event)?,
            }
        }
        Ok(())
    }
}

fn write_event_line(f: &mut fmt::Formatter<'_>, event: &NoteEvent) -> fmt::Result {
    let (frame, channel) = (event.frame, event.channel);
    match event.kind {
        NoteEventKind::NoteOn { note, velocity } => {
            writeln!(f, "@{frame} ch{channel} note_on {note} {velocity:.3}")
        }
        NoteEventKind::NoteOff { note, velocity } => {
            writeln!(f, "@{frame} ch{channel} note_off {note} {velocity:.3}")
        }
        NoteEventKind::ControlChange { controller, value } => {
            writeln!(f, "@{frame} ch{channel} cc {controller} {value:.3}")
        }
        NoteEventKind::PitchBend(value) => writeln!(f, "@{frame} ch{channel} bend {value:.3}"),
    }
}

fn write_event_tsv(f: &mut fmt::Formatter<'_>, event: &NoteEvent) -> fmt::Result {
    let (frame, channel) = (event.frame, event.channel);
    match event.kind {
        NoteEventKind::NoteOn { note, velocity } => {
            writeln!(f, "{frame}\t{channel}\tnote_on\t{note}\t{velocity:.3}")
        }
        NoteEventKind::NoteOff { note, velocity } => {
            writeln!(f, "{frame}\t{channel}\tnote_off\t{note}\t{velocity:.3}")
        }
        NoteEventKind::ControlChange { controller, value } => {
            writeln!(f, "{frame}\t{channel}\tcc\t{controller}\t{value:.3}")
        }
        NoteEventKind::PitchBend(value) => {
            writeln!(f, "{frame}\t{channel}\tbend\t\t{value:.3}")
        }
    }
}

fn help() -> String {
    "usage: kazoo-play [--play|--events|--jam] [--bpm N] [--sample-rate HZ] [--channel 0-15] [--format lines|tsv] [--name NAME] [--velocity 0-127] [--drive] [NOTATION...]\n\n--jam plugs into the kazoo-mix desk and loops the phrase on the song, locked to the desk's play/stop and tempo; each line on stdin is a new phrase, taking over at the next loop. --drive sends the notes to the other instruments instead of sounding.\n\nexamples:\n  kazoo-play --jam --name bass 'c2/8 c2/8 r/4 eb2/8 g2/8 r/4'\n  echo 'c4/8 d4/8 e4/8 [g4 b4 d5]/4 r/8' | kazoo-play --play\n  kazoo-play --bpm 96 --channel 2 'c3/4 [g3 bb3]/4 r/8 c4/8'\n"
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_args_and_outputs_lines() {
        let output = run([
            "--bpm".to_string(),
            "120".to_string(),
            "--sample-rate".to_string(),
            "48000".to_string(),
            "c4/4".to_string(),
        ])
        .unwrap();

        assert_eq!(
            output,
            "@0 ch0 note_on 60 0.800\n@24000 ch0 note_off 60 0.000\n"
        );
    }

    #[test]
    fn parses_play_mode() {
        let options = parse_args(["--play".to_string(), "c4/4".to_string()]).unwrap();

        assert_eq!(options.mode, Mode::Play);
        assert_eq!(options.notation, Some("c4/4".to_string()));
    }

    #[test]
    fn supports_tsv_and_channel() {
        let output = run([
            "--format".to_string(),
            "tsv".to_string(),
            "--channel".to_string(),
            "2".to_string(),
            "c4/4".to_string(),
        ])
        .unwrap();

        assert_eq!(
            output,
            "0\t2\tnote_on\t60\t0.800\n24000\t2\tnote_off\t60\t0.000\n"
        );
    }

    #[test]
    fn reports_parse_errors() {
        let err = run(["nope/4".to_string()]).unwrap_err();

        assert!(err.contains("token 1"));
    }

    #[test]
    fn formats_every_event_kind() {
        let event = |frame, kind| NoteEvent {
            frame,
            source: None,
            destination: None,
            channel: 1,
            kind,
        };
        let events = [
            event(
                0,
                NoteEventKind::ControlChange {
                    controller: 7,
                    value: 0.5,
                },
            ),
            event(5, NoteEventKind::PitchBend(-0.25)),
        ];
        assert_eq!(
            format_events(&events, OutputFormat::Lines),
            "@0 ch1 cc 7 0.500\n@5 ch1 bend -0.250\n"
        );
        assert_eq!(
            format_events(&events, OutputFormat::Tsv),
            "0\t1\tcc\t7\t0.500\n5\t1\tbend\t\t-0.250\n"
        );
    }

    #[test]
    fn output_write_errors_are_returned() {
        struct Closed;
        impl Write for Closed {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let err = write_output(&mut Closed, "@0 ch0 note_on 60 0.800\n").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
        let mut buffer = Vec::new();
        write_output(&mut buffer, "abc").unwrap();
        assert_eq!(buffer, b"abc");
    }

    #[test]
    fn only_lost_devices_are_fatal() {
        assert!(is_fatal(&cpal::StreamError::DeviceNotAvailable));
        assert!(is_fatal(&cpal::StreamError::StreamInvalidated));
        assert!(!is_fatal(&cpal::StreamError::BufferUnderrun));
    }

    #[test]
    fn synth_helpers_are_bounded() {
        let event = RenderEvent {
            frame: 0,
            end_frame: 48_000,
            note: 60,
            velocity: 0.8,
        };

        let sample = voice_sample(&event, 1_000, 48_000.0);
        assert!(sample.is_finite());
        assert!(sample.abs() <= 1.0);
    }
}
