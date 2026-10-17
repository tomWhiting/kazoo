use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, Parser, Subcommand, ValueEnum};
use kazoo_wall::daemon::{AudioMode, DaemonConfig, DeskMode, ServeCommand};

/// The kazoo studio: the desk, the wall, the seat and every instrument, in one command.
#[derive(Debug, Parser)]
#[command(name = "kazoo", version, about, long_about = None, propagate_version = true)]
pub struct Cli {
    /// Desk host:port, or a socket path containing /.
    #[arg(long, global = true, value_name = "ADDRESS", value_parser = desk_address)]
    pub desk: Option<String>,
    /// The studio command to run.
    #[command(subcommand)]
    pub command: Command,
}

fn desk_address(text: &str) -> Result<String, String> {
    kazoo_core::ipc::link::HubAddress::parse(text)
        .map(|_| text.to_owned())
        .map_err(|error| error.to_string())
}

/// The desk, wall, seat and instruments available in the studio.
#[derive(Debug, Subcommand, PartialEq)]
pub enum Command {
    /// The desk: every instrument plugs into a strip and follows its tempo and play/stop.
    Mix(MixArgs),
    /// Start or stop the desk and instruments named in a studio session file.
    Studio {
        /// The operation on the named studio session.
        #[command(subcommand)]
        command: StudioCommand,
    },
    /// The wall: the shared modular synth that never stops (the console, unless told otherwise).
    Wall {
        /// The wall operation; none opens its console.
        #[command(subcommand)]
        command: Option<WallCommand>,
    },
    /// The wall for a Claude seat: an MCP server on stdio.
    Mcp(McpArgs),
    /// TR-808 drum machine.
    #[command(name = "808")]
    EightOhEight,
    /// TB-303 acid bass.
    #[command(name = "303")]
    ThreeOhThree,
    /// Yamaha CS-80 pad synth.
    Cs80,
    /// Minimoog bass and lead.
    Mini,
    /// Juno chorus polysynth.
    Juno,
    /// Prophet-5 polysynth.
    Prophet,
    /// Four-operator FM synth.
    Dx(PhraseArgs),
    /// Karplus-Strong plucked string.
    String(PhraseArgs),
    /// Modal bell and glass synth.
    Glass(GlassArgs),
    /// Jupiter-8 style arpeggiator.
    Arp(ArpArgs),
    /// Text notation to events or sound.
    Play(PlayArgs),
    /// The mouth: sing or whistle into a synth, and drive every other instrument with your voice.
    Mouth(MouthArgs),
    /// The voice tracker DAW (kazoo-tui).
    Tui,
    /// The wall's tree: every moment it has been, grown as a tree you can walk.
    Tree(TreeArgs),
}

/// How to show the tree.
#[derive(Debug, Args, Default, PartialEq, Eq)]
pub struct TreeArgs {
    /// Print the whole tree as braille text and exit, instead of walking it.
    #[arg(long)]
    pub print: bool,
    /// Columns for --print.
    #[arg(long, requires = "print", default_value_t = 100, value_parser = clap::value_parser!(u16).range(10..=1000))]
    pub cols: u16,
    /// Rows for --print.
    #[arg(long, requires = "print", default_value_t = 40, value_parser = clap::value_parser!(u16).range(5..=1000))]
    pub rows: u16,
}

/// Microphone device selection and enumeration.
#[derive(Debug, Args, Default, PartialEq)]
pub struct MouthArgs {
    /// Input device name; omit to follow the system default.
    #[arg(long)]
    pub input: Option<String>,
    /// Input channel, 1-based; omit to average all channels.
    #[arg(long, value_parser = clap::value_parser!(u16).range(1..=64))]
    pub input_channel: Option<u16>,
    /// List input devices without opening a stream.
    #[arg(long)]
    pub list_inputs: bool,
    /// Output device name; omit to use the system default.
    #[arg(long)]
    pub output: Option<String>,
    /// Snap pitch to scale:root, for example major:D or pentatonic-minor:C#.
    #[arg(long, value_parser = kazoo_mouth::parse_quantise)]
    pub quantise: Option<kazoo_core::voice::quantise::QuantiseSettings>,
    /// Publish voice frames without making sound.
    #[arg(long)]
    pub silent: bool,
    /// Score recorded WAV takes and exit without opening audio.
    #[arg(long, num_args = 1.., conflicts_with = "list_inputs")]
    pub score: Vec<PathBuf>,
}

/// Desk startup options.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct MixArgs {
    /// Put a built-in 808 pattern on strip 1.
    #[arg(long)]
    pub demo: bool,
}

/// Operations on a named studio session.
#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum StudioCommand {
    /// Start the desk and instruments in tmux, then attach to their panes.
    Up {
        /// The session file name in ~/.kazoo/studio, without .toml.
        name: String,
    },
    /// Stop the desk before stopping the instrument panes.
    Down {
        /// The tmux studio session name, without the kazoo- prefix.
        name: String,
    },
}

/// Operations on the shared wall.
#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum WallCommand {
    /// Plant the tree and grow its trunk from the wall's change log (once, with the wall stopped).
    Tree {
        #[command(subcommand)]
        command: TreeCommand,
    },
    /// Run the wall until stopped (Ctrl-C, SIGTERM, or a console).
    Serve(ServeArgs),
    /// Ask the running wall to stop.
    Stop,
    /// Open the console, starting the wall if it is not playing (the default).
    Console,
}

/// Operations on the wall's lasting history.
#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum TreeCommand {
    /// Plant the tree and grow its trunk from the wall's change log (once, with the wall stopped).
    Import(ImportArgs),
}

/// Options for growing the first rings.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct ImportArgs {
    /// Grow beside the tree, report the history, and remove the scratch tree.
    #[arg(long)]
    pub dry_run: bool,
}

/// Wall rendering and desk options.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct ServeArgs {
    /// Render on a timer with no audio device (the desk still hears the wall).
    #[arg(long)]
    pub no_audio: bool,
    /// Never join the kazoo-mix desk.
    #[arg(long)]
    pub no_desk: bool,
    /// Sample rate: with a device, switch it to this; with --no-audio, render at this rate (default 48000).
    #[arg(long, value_parser = clap::value_parser!(u32).range(8_000..=384_000))]
    pub rate: Option<u32>,
}

/// Settings for a seat's stdio MCP server.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct McpArgs {
    /// Who this seat is on the wall: 1 to 24 of A-Z a-z 0-9 space _ . -.
    #[arg(long, env = "KAZOO_SEAT", value_parser = seat_name)]
    pub seat: String,
    /// News of other seats' changes comes at most once per this many seconds.
    #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u64).range(1..=3_600))]
    pub notify_every: u64,
    /// The wall's control socket (default: kazoo-wall.sock in kazoo's runtime directory).
    #[arg(long)]
    pub socket: Option<PathBuf>,
}

/// Notation for a looping instrument.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct PhraseArgs {
    /// A looped kazoo-play phrase, as notation.
    #[arg(long)]
    pub phrase: Option<String>,
}

/// Glass preview and phrase options.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct GlassArgs {
    /// Render the preview to this WAV and exit.
    #[arg(long)]
    pub demo: Option<PathBuf>,
    /// A looped kazoo-play phrase, as notation.
    #[arg(long)]
    pub phrase: Option<String>,
}

/// Arpeggiator destination and audition options.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct ArpArgs {
    /// Drive every instrument (all) or one registered instrument UUID.
    #[arg(long, default_value = "all", value_parser = kazoo_arp::parse_target)]
    pub target: [u8; 16],
    /// Mute the audition voice while still driving the desk.
    #[arg(long)]
    pub silent: bool,
}

/// Mutually exclusive event listing, playback and jam modes.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct PlaybackArgs {
    /// Play the notation once.
    #[arg(long, group = "mode")]
    pub play: bool,
    /// List timestamped events (the default).
    #[arg(long, group = "mode")]
    pub events: bool,
    /// Loop the notation with the desk.
    #[arg(long, group = "mode")]
    pub jam: bool,
}

/// Event listing, playback and jam options.
#[derive(Debug, Args, PartialEq)]
pub struct PlayArgs {
    /// How to deliver the notation.
    #[command(flatten)]
    pub mode: PlaybackArgs,
    /// Send jam notes to the other instruments.
    #[arg(long)]
    pub drive: bool,
    /// Strip name when joining the desk.
    #[arg(long, default_value = "kazoo-play")]
    pub name: String,
    /// Jam note velocity.
    #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u8).range(0..=127))]
    pub velocity: u8,
    /// Tempo in beats per minute; finite and positive.
    #[arg(long, default_value_t = 120.0, value_parser = positive_bpm)]
    pub bpm: f64,
    /// Sample rate used to timestamp events.
    #[arg(long = "sample-rate", visible_alias = "rate", default_value_t = 48_000,
          value_parser = clap::value_parser!(u32).range(1..))]
    pub sample_rate: u32,
    /// MIDI channel, from zero to fifteen.
    #[arg(short = 'c', long, default_value_t = 0, value_parser = clap::value_parser!(u8).range(0..=15))]
    pub channel: u8,
    /// Format of the event listing.
    #[arg(long, value_enum, default_value_t = Format::Lines)]
    pub format: Format,
    /// Notation; with none, stdin is read.
    #[arg(trailing_var_arg = true)]
    pub notation: Vec<String>,
}

/// Format of a timestamped event listing.
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum Format {
    /// One readable event per line.
    Lines,
    /// Tab-separated event fields.
    Tsv,
}

fn positive_bpm(text: &str) -> Result<f64, String> {
    let bpm: f64 = text.parse().map_err(|_| "--bpm must be positive")?;
    if bpm.is_finite() && bpm > 0.0 {
        Ok(bpm)
    } else {
        Err("--bpm must be positive".to_owned())
    }
}

fn seat_name(text: &str) -> Result<String, String> {
    if kazoo_wall::protocol::valid_name(text) {
        Ok(text.to_owned())
    } else {
        Err(format!(
            "'{}' is not a valid seat name: use 1 to 24 of A-Z a-z 0-9 space _ . -",
            text.escape_default()
        ))
    }
}

impl From<MouthArgs> for kazoo_mouth::Args {
    fn from(args: MouthArgs) -> Self {
        Self {
            input: args.input,
            input_channel: args.input_channel,
            list_inputs: args.list_inputs,
            output: args.output,
            quantise: args.quantise.unwrap_or_default(),
            silent: args.silent,
            score: args.score,
        }
    }
}

impl From<MixArgs> for kazoo_mix::Args {
    fn from(args: MixArgs) -> Self {
        Self { demo: args.demo }
    }
}

impl From<PhraseArgs> for kazoo_dx::Args {
    fn from(args: PhraseArgs) -> Self {
        Self {
            phrase: args.phrase,
        }
    }
}

impl From<PhraseArgs> for kazoo_string::Args {
    fn from(args: PhraseArgs) -> Self {
        Self {
            phrase: args.phrase,
        }
    }
}

impl From<GlassArgs> for kazoo_glass::Args {
    fn from(args: GlassArgs) -> Self {
        Self {
            demo: args.demo,
            phrase: args.phrase,
        }
    }
}

impl From<ArpArgs> for kazoo_arp::Args {
    fn from(args: ArpArgs) -> Self {
        Self {
            target: args.target,
            silent: args.silent,
        }
    }
}

impl From<PlayArgs> for kazoo_play::Args {
    fn from(args: PlayArgs) -> Self {
        Self {
            mode: if args.mode.jam {
                kazoo_play::Mode::Jam
            } else if args.mode.play {
                kazoo_play::Mode::Play
            } else {
                kazoo_play::Mode::Events
            },
            notation: if args.notation.is_empty() {
                None
            } else {
                Some(args.notation.join(" "))
            },
            bpm: args.bpm,
            sample_rate: args.sample_rate,
            channel: args.channel,
            format: match args.format {
                Format::Lines => kazoo_play::OutputFormat::Lines,
                Format::Tsv => kazoo_play::OutputFormat::Tsv,
            },
            name: args.name,
            velocity: args.velocity,
            drive: args.drive,
        }
    }
}

impl From<McpArgs> for kazoo_mcp::Args {
    fn from(args: McpArgs) -> Self {
        Self {
            seat: args.seat,
            notify_every: Some(Duration::from_secs(args.notify_every)),
            socket: Some(args.socket.unwrap_or_else(kazoo_wall::paths::socket_path)),
        }
    }
}

impl TryFrom<ServeArgs> for DaemonConfig {
    type Error = std::io::Error;

    fn try_from(args: ServeArgs) -> Result<Self, Self::Error> {
        let mut config = Self::standard()?;
        if args.no_desk {
            config.desk = DeskMode::Off;
        }
        config.audio = if args.no_audio {
            AudioMode::Headless {
                sample_rate: args.rate.unwrap_or(48_000),
            }
        } else {
            AudioMode::Device {
                sample_rate: args.rate,
            }
        };
        Ok(config)
    }
}

/// Build the console's daemon launch command for the unified executable.
#[must_use]
pub fn console_serve(program: PathBuf) -> ServeCommand {
    ServeCommand {
        program,
        args: vec!["wall".into(), "serve".into()],
    }
}

#[cfg(test)]
mod tests;
