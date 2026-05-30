//! kazoo-play — pipe text notation into timestamped musical events.
//!
//! First slice: parse compact notation from stdin or argv and emit a stable
//! line-oriented event stream. This is intentionally useful before any socket
//! bridge exists: agents can generate notation, shell tools can transform it,
//! and future Kazoo clients can consume the same lines.

use std::env;
use std::io::{self, Read};
use std::process::ExitCode;

use kazoo_core::notation::{NotationError, parse_note_events};
use kazoo_core::protocol::{NoteEvent, NoteEventKind};

const DEFAULT_BPM: f64 = 120.0;
const DEFAULT_SAMPLE_RATE: u32 = 48_000;
const DEFAULT_CHANNEL: u8 = 0;

#[derive(Debug, Clone, PartialEq)]
struct Options {
    bpm: f64,
    sample_rate: u32,
    channel: u8,
    format: OutputFormat,
    notation: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            bpm: DEFAULT_BPM,
            sample_rate: DEFAULT_SAMPLE_RATE,
            channel: DEFAULT_CHANNEL,
            format: OutputFormat::Lines,
            notation: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Lines,
    Tsv,
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
    let notation = match options.notation {
        Some(notation) => notation,
        None => read_stdin()?,
    };
    let events = parse_note_events(&notation, options.bpm, options.sample_rate, options.channel)
        .map_err(format_notation_error)?;

    Ok(format_events(&events, options.format))
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut options = Options::default();
    let mut notation_parts = Vec::new();
    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => return Err(help()),
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
    "usage: kazoo-play [--bpm N] [--sample-rate HZ] [--channel 0-15] [--format lines|tsv] [NOTATION...]\n\nexamples:\n  echo 'c4/8 d4/8 e4/8 [g4 b4 d5]/4 r/8' | kazoo-play\n  kazoo-play --bpm 96 --channel 2 'c3/4 [g3 bb3]/4 r/8 c4/8'\n"
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
}
