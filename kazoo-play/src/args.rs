use std::fmt;
use std::io::{self, Write};
use std::process::ExitCode;

const DEFAULT_BPM: f64 = 120.0;
const DEFAULT_SAMPLE_RATE: u32 = 48_000;
const DEFAULT_CHANNEL: u8 = 0;

/// What the command line gave.
#[derive(Debug, Clone, PartialEq)]
pub struct Args {
    /// Tempo in beats per minute.
    pub bpm: f64,
    /// Sample rate used to timestamp events.
    pub sample_rate: u32,
    /// MIDI channel, from zero to fifteen.
    pub channel: u8,
    /// Format of the event listing.
    pub format: OutputFormat,
    /// Whether to list events, play once, or jam.
    pub mode: Mode,
    /// Notation supplied as arguments; otherwise read stdin.
    pub notation: Option<String>,
    /// Strip name used when joining the desk.
    pub name: String,
    /// Jam note velocity, from zero to 127.
    pub velocity: u8,
    /// Send jam notes to the other instruments.
    pub drive: bool,
}

impl Default for Args {
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

/// Format of the timestamped event listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// One readable event per line.
    Lines,
    /// Tab-separated event fields.
    Tsv,
}

/// How to deliver the notation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Return a timestamped event listing.
    Events,
    /// Sound the notation once on the default output device.
    Play,
    /// Loop a phrase in time with the desk.
    Jam,
}

/// What `--help` prints.
pub const USAGE: &str = "kazoo-play: pipe text notation into timestamped musical events or sound\n\nusage: kazoo play [--play|--events|--jam] [--bpm N] [--sample-rate HZ] [--channel 0-15] [--format lines|tsv] [--name NAME] [--velocity 0-127] [--drive] [NOTATION...]\n       kazoo-play [--play|--events|--jam] [--bpm N] [--sample-rate HZ] [--channel 0-15] [--format lines|tsv] [--name NAME] [--velocity 0-127] [--drive] [NOTATION...]\n\n--jam plugs into the kazoo-mix desk and loops the phrase on the song, locked to the desk's play/stop and tempo; each line on stdin is a new phrase, taking over at the next loop. --drive sends the notes to the other instruments instead of sounding.\n\nexamples:\n  kazoo-play --jam --name bass 'c2/8 c2/8 r/4 eb2/8 g2/8 r/4'\n  echo 'c4/8 d4/8 e4/8 [g4 b4 d5]/4 r/8' | kazoo-play --play\n  kazoo-play --bpm 96 --channel 2 'c3/4 [g3 bb3]/4 r/8 c4/8'\n";

/// Why a run did not deliver, and the exit code a shell should see.
#[derive(Debug)]
pub enum Error {
    /// Bad notation, no device, the desk link, or stdin: exit 2.
    Run(String),
    /// Output could not be written: exit 1, silent when the reader went away.
    Output(io::Error),
}

impl Error {
    /// Exit code for this failure: run errors are 2, output errors are 1.
    #[must_use]
    pub const fn exit_code(&self) -> u8 {
        match self {
            Self::Run(_) => 2,
            Self::Output(_) => 1,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Run(why) => f.write_str(why),
            Self::Output(err) => write!(f, "failed to write output: {err}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Run(_) => None,
            Self::Output(err) => Some(err),
        }
    }
}

/// The standalone binary's entry: parse, print help, and run.
pub fn cli(args: impl IntoIterator<Item = String>) -> ExitCode {
    let result = match parse_args(args) {
        Ok(None) => {
            let mut stdout = io::stdout().lock();
            stdout
                .write_all(USAGE.as_bytes())
                .and_then(|()| stdout.flush())
                .map_err(Error::Output)
        }
        Ok(Some(args)) => crate::run::run(args),
        Err(why) => Err(Error::Run(why)),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(Error::Output(err)) if err.kind() == io::ErrorKind::BrokenPipe => ExitCode::from(1),
        Err(err) => {
            eprintln!("kazoo-play: {err}");
            ExitCode::from(err.exit_code())
        }
    }
}

/// Read arguments after the program name.
///
/// `Ok(None)` requests help, wherever `-h` or `--help` stands before `--`,
/// even after a bad argument or where a flag's value would be; an error is
/// one sentence describing the invalid argument.
pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<Args>, String> {
    let args: Vec<String> = args.into_iter().collect();
    if args
        .iter()
        .take_while(|arg| arg.as_str() != "--")
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        return Ok(None);
    }
    let mut options = Args::default();
    let mut notation_parts = Vec::new();
    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
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

    Ok(Some(options))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_is_not_an_error() {
        for flag in ["--help", "-h"] {
            assert_eq!(parse_args([flag.to_string()]), Ok(None));
        }
    }

    #[test]
    fn defaults_are_the_documented_ones() {
        let expected = Args {
            bpm: 120.0,
            sample_rate: 48_000,
            channel: 0,
            format: OutputFormat::Lines,
            mode: Mode::Events,
            notation: None,
            name: "kazoo-play".to_string(),
            velocity: 100,
            drive: false,
        };
        assert_eq!(Args::default(), expected);
        assert_eq!(parse_args([]), Ok(Some(expected)));
    }

    #[test]
    fn every_flag_lands() {
        let args = [
            "--jam",
            "--drive",
            "--name",
            "bass",
            "--velocity",
            "90",
            "--bpm",
            "96",
            "--rate",
            "44100",
            "-c",
            "2",
            "--format",
            "tsv",
            "--",
            "c4/4",
            "e4/4",
        ];
        assert_eq!(
            parse_args(args.map(str::to_string)),
            Ok(Some(Args {
                bpm: 96.0,
                sample_rate: 44_100,
                channel: 2,
                format: OutputFormat::Tsv,
                mode: Mode::Jam,
                notation: Some("c4/4 e4/4".to_string()),
                name: "bass".to_string(),
                velocity: 90,
                drive: true,
            }))
        );
    }

    #[test]
    fn exit_codes_follow_the_error_kind() {
        assert_eq!(Error::Run("bad notation".to_string()).exit_code(), 2);
        assert_eq!(
            Error::Output(std::io::ErrorKind::BrokenPipe.into()).exit_code(),
            1
        );
    }

    #[test]
    fn help_wins_wherever_it_stands() {
        let parse = |args: &[&str]| parse_args(args.iter().map(|arg| (*arg).to_string()));
        assert_eq!(parse(&["--bogus", "--help"]), Ok(None));
        assert_eq!(parse(&["--name", "-h"]), Ok(None));
        assert_eq!(parse(&["--bpm", "--help"]), Ok(None));
        assert_eq!(parse(&["c4/4", "-h"]), Ok(None));
        assert_eq!(
            parse(&["--", "-h"]),
            Ok(Some(Args {
                notation: Some("-h".to_string()),
                ..Args::default()
            }))
        );
    }

    #[test]
    fn usage_opens_with_the_name_and_description() {
        assert!(USAGE.starts_with(
            "kazoo-play: pipe text notation into timestamped musical events or sound\n\n"
        ));
    }

    #[test]
    fn usage_names_both_ways_to_start_it() {
        assert!(USAGE.contains("usage: kazoo play "));
        assert!(USAGE.contains("\n       kazoo-play "));
    }

    #[test]
    fn parses_play_mode() {
        let options = parse_args(["--play".to_string(), "c4/4".to_string()])
            .unwrap()
            .unwrap();

        assert_eq!(options.mode, Mode::Play);
        assert_eq!(options.notation, Some("c4/4".to_string()));
    }
}
