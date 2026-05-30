//! kazoo-play — pipe text notation into timestamped musical events or sound.
//!
//! `kazoo-play` is the first terminal-native musical primitive: humans and
//! agents can write compact notation, pipe it around, print timestamped events,
//! or play it immediately through the default audio device.

use std::env;
use std::f32::consts::TAU;
use std::io::{self, Read};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use kazoo_core::notation::{NotationError, parse_note_events};
use kazoo_core::protocol::{NoteEvent, NoteEventKind};

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
}

fn main() -> ExitCode {
    match run(env::args().skip(1)) {
        Ok(output) => {
            print!("{output}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("kazoo-play: {err}");
            ExitCode::from(2)
        }
    }
}

fn run(args: impl IntoIterator<Item = String>) -> Result<String, String> {
    let options = parse_args(args)?;
    let notation = match options.notation.as_ref() {
        Some(notation) => notation.clone(),
        None => read_stdin()?,
    };
    let events = parse_note_events(&notation, options.bpm, options.sample_rate, options.channel)
        .map_err(format_notation_error)?;

    match options.mode {
        Mode::Events => Ok(format_events(&events, options.format)),
        Mode::Play => {
            play_events(&events, options.sample_rate)?;
            Ok(String::new())
        }
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

fn format_notation_error(err: NotationError) -> String {
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

    let stream = match sample_format {
        cpal::SampleFormat::F32 => build_stream::<f32>(&device, &config, Arc::clone(&render))?,
        cpal::SampleFormat::I16 => build_stream::<i16>(&device, &config, Arc::clone(&render))?,
        cpal::SampleFormat::U16 => build_stream::<u16>(&device, &config, Arc::clone(&render))?,
        other => return Err(format!("unsupported output sample format: {other:?}")),
    };
    stream
        .play()
        .map_err(|err| format!("failed to start output stream: {err}"))?;

    while !done.load(Ordering::Acquire) {
        thread::sleep(Duration::from_millis(10));
    }
    drop(stream);
    Ok(())
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    render: Arc<RenderState>,
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
            move |err| eprintln!("kazoo-play stream error: {err}"),
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
            let end_frame = find_note_off(events, idx + 1, event.channel, note)
                .map_or(frame + scale_frame(RELEASE_FRAMES, rate_ratio), |off| {
                    scale_frame(off, rate_ratio)
                });
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
        (sample * 0.25).tanh()
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
    (sine * 0.75 + soft_saw * 0.25) * amp * event.velocity
}

fn midi_frequency(note: u8) -> f32 {
    440.0 * 2.0_f32.powf((f32::from(note) - 69.0) / 12.0)
}

fn format_events(events: &[NoteEvent], format: OutputFormat) -> String {
    let mut out = String::new();
    for event in events {
        match format {
            OutputFormat::Lines => format_event_line(&mut out, event),
            OutputFormat::Tsv => format_event_tsv(&mut out, event),
        }
    }
    out
}

fn format_event_line(out: &mut String, event: &NoteEvent) {
    match event.kind {
        NoteEventKind::NoteOn { note, velocity } => {
            out.push_str(&format!(
                "@{} ch{} note_on {} {:.3}\n",
                event.frame, event.channel, note, velocity
            ));
        }
        NoteEventKind::NoteOff { note, velocity } => {
            out.push_str(&format!(
                "@{} ch{} note_off {} {:.3}\n",
                event.frame, event.channel, note, velocity
            ));
        }
        NoteEventKind::ControlChange { controller, value } => {
            out.push_str(&format!(
                "@{} ch{} cc {} {:.3}\n",
                event.frame, event.channel, controller, value
            ));
        }
        NoteEventKind::PitchBend(value) => {
            out.push_str(&format!(
                "@{} ch{} bend {:.3}\n",
                event.frame, event.channel, value
            ));
        }
    }
}

fn format_event_tsv(out: &mut String, event: &NoteEvent) {
    match event.kind {
        NoteEventKind::NoteOn { note, velocity } => {
            out.push_str(&format!(
                "{}\t{}\tnote_on\t{}\t{:.3}\n",
                event.frame, event.channel, note, velocity
            ));
        }
        NoteEventKind::NoteOff { note, velocity } => {
            out.push_str(&format!(
                "{}\t{}\tnote_off\t{}\t{:.3}\n",
                event.frame, event.channel, note, velocity
            ));
        }
        NoteEventKind::ControlChange { controller, value } => {
            out.push_str(&format!(
                "{}\t{}\tcc\t{}\t{:.3}\n",
                event.frame, event.channel, controller, value
            ));
        }
        NoteEventKind::PitchBend(value) => {
            out.push_str(&format!(
                "{}\t{}\tbend\t\t{:.3}\n",
                event.frame, event.channel, value
            ));
        }
    }
}

fn help() -> String {
    "usage: kazoo-play [--play|--events] [--bpm N] [--sample-rate HZ] [--channel 0-15] [--format lines|tsv] [NOTATION...]\n\nexamples:\n  echo 'c4/8 d4/8 e4/8 [g4 b4 d5]/4 r/8' | kazoo-play --play\n  kazoo-play --bpm 96 --channel 2 'c3/4 [g3 bb3]/4 r/8 c4/8'\n"
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
