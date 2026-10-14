use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use color_eyre::Result;
use color_eyre::eyre::{WrapErr, eyre};
use crossbeam_channel::Sender;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use kazoo_core::analysis::PitchDetectorConfig;
use kazoo_core::host::audio::{DeviceChoice, OpenedOutput, OutputRequest};
use kazoo_core::host::link::{LinkSetup, connect_or_direct};
use kazoo_core::io::mic::{Mic, MicRequest, MicTarget};
use kazoo_core::ipc::link::HubLink;
use kazoo_core::voice::VoiceFrame;
use kazoo_core::voice::judge::{self, Judge, OnsetConfig};
use kazoo_core::voice::quantise::{NOTE_NAMES, QuantiseSettings, SCALE_NAMES};
use ratatui::widgets::Paragraph;
use ringbuf::traits::{Consumer, Split};
use ringbuf::{HeapCons, HeapRb};

use crate::engine::{Command, EngineSetup, MouthEngine, Stats};

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Args {
    pub input: Option<String>,
    pub input_channel: Option<u16>,
    pub list_inputs: bool,
    pub output: Option<String>,
    pub quantise: QuantiseSettings,
    pub silent: bool,
    pub score: Vec<PathBuf>,
}

pub const USAGE: &str =
    "kazoo-mouth: sing or whistle into a synth, and drive every other instrument with your voice

usage: kazoo mouth [options]
       kazoo-mouth [options]

  --input <name>          Choose the microphone.
  --input-channel <n>     Choose channel 1 to 64; otherwise average all.
  --list-inputs           List inputs and exit without opening a stream.
  --output <name>         Choose the output device.
  --quantise <scale:root> Snap pitch, for example major:D.
  --silent               Publish voice without making sound.
  --score <wav>...        Score recorded takes and exit without audio.

Space toggles quantise. Esc and Ctrl-C quit.
";

pub fn parse_quantise(text: &str) -> std::result::Result<QuantiseSettings, String> {
    let choices = || {
        format!(
            "use scale:root; scales: {}; roots: {}",
            SCALE_NAMES.map(|name| name.replace(' ', "-")).join(", "),
            NOTE_NAMES.join(", ")
        )
    };
    let (scale, root) = text.split_once(':').ok_or_else(choices)?;
    let scale = SCALE_NAMES
        .iter()
        .position(|name| name.replace(' ', "-").eq_ignore_ascii_case(scale))
        .ok_or_else(choices)?;
    let root = NOTE_NAMES
        .iter()
        .position(|name| name.eq_ignore_ascii_case(root))
        .ok_or_else(choices)?;
    Ok(QuantiseSettings {
        scale,
        root: root as u8,
        blend: 1.0,
        ..QuantiseSettings::default()
    })
}

pub fn parse_args(
    words: impl IntoIterator<Item = String>,
) -> std::result::Result<Option<Args>, String> {
    let words: Vec<_> = words.into_iter().collect();
    if words
        .iter()
        .any(|word| matches!(word.as_str(), "-h" | "--help"))
    {
        return Ok(None);
    }
    let mut words = words.into_iter().peekable();
    let mut args = Args::default();
    while let Some(word) = words.next() {
        match word.as_str() {
            "--input" | "--output" | "--input-channel" | "--quantise" => {
                let value = words
                    .next()
                    .filter(|value| !value.starts_with("--") && !value.is_empty())
                    .ok_or_else(|| format!("{word} needs a value"))?;
                match word.as_str() {
                    "--input" => args.input = Some(value),
                    "--output" => args.output = Some(value),
                    "--quantise" => args.quantise = parse_quantise(&value)?,
                    _ => {
                        let channel = value
                            .parse::<u16>()
                            .map_err(|_| "--input-channel needs a number from 1 to 64")?;
                        if !(1..=64).contains(&channel) {
                            return Err("--input-channel needs a number from 1 to 64".into());
                        }
                        args.input_channel = Some(channel);
                    }
                }
            }
            "--list-inputs" => args.list_inputs = true,
            "--silent" => args.silent = true,
            "--score" => {
                while words.peek().is_some_and(|word| !word.starts_with('-')) {
                    if let Some(path) = words.next() {
                        args.score.push(PathBuf::from(path));
                    }
                }
                if args.score.is_empty() {
                    return Err("--score needs at least one WAV path".into());
                }
            }
            _ => return Err(format!("unknown argument '{word}'\n{USAGE}")),
        }
    }
    Ok(Some(args))
}

pub fn cli(words: impl IntoIterator<Item = String>) -> Result<()> {
    match parse_args(words).map_err(|error| eyre!(error))? {
        Some(args) => run(args),
        None => {
            write!(io::stdout().lock(), "{USAGE}")?;
            Ok(())
        }
    }
}

pub fn run(args: Args) -> Result<()> {
    if !args.score.is_empty() {
        return crate::score::print_takes(
            &args.score,
            &mut io::stdout().lock(),
            &mut io::stderr().lock(),
        );
    }
    if args.list_inputs {
        let mut out = io::stdout().lock();
        for device in kazoo_core::io::device::enumerate_input_devices()? {
            writeln!(
                out,
                "{}{}: {} input channels; rates {:?}",
                device.name,
                if device.is_default { " (default)" } else { "" },
                device.max_input_channels,
                device.supported_sample_rates
            )?;
        }
        return Ok(());
    }
    if args
        .input_channel
        .is_some_and(|channel| !(1..=64).contains(&channel))
    {
        return Err(eyre!("--input-channel must be between 1 and 64"));
    }
    let quantise = args.quantise.held();
    let silent = args.silent;
    let mut session = Session::start(args)?;
    let mut terminal = kazoo_host::terminal::enter()?;
    let result = event_loop(&mut session, &mut terminal, quantise, silent);
    let restored = terminal.leave().wrap_err("restoring the mouth terminal");
    let stopped = session.stop();
    combine(combine(result, restored), stopped)
}

struct Session {
    output: Option<OpenedOutput>,
    mic: Option<Mic>,
    hub: Option<HubLink>,
    judge: Option<Judge>,
    commands: Sender<Command>,
    display: HeapCons<VoiceFrame>,
    stats: Arc<Stats>,
}

impl Session {
    fn start(args: Args) -> Result<Self> {
        let prepared = OutputRequest {
            device: device(args.output),
            ..OutputRequest::default()
        }
        .prepare_output()?;
        let info = prepared.info();
        let (mic_tx, mic_rx) = HeapRb::new((info.rate / 10).max(1) as usize).split();
        let (display_tx, display) = HeapRb::new(1024).split();
        let (commands, command_rx) = crossbeam_channel::bounded(64);
        let (hub, desk) = connect_or_direct(&LinkSetup {
            name: "kazoo-mouth",
            channels: 2,
            rate: info.rate,
            block: info.frames,
        })?;
        let (feed, results, judge) = judge::spawn(
            PitchDetectorConfig::for_rate(info.rate),
            info.frames,
            OnsetConfig::default(),
        )?;
        let stats = Arc::new(Stats::default());
        let mut session = Self {
            output: None,
            mic: None,
            hub: Some(hub),
            judge: Some(judge),
            commands,
            display,
            stats: Arc::clone(&stats),
        };
        let mut engine = MouthEngine::new(EngineSetup {
            rate: info.rate,
            block: info.frames,
            mic: mic_rx,
            commands: command_rx,
            display: display_tx,
            desk,
            judge: (feed, results),
            stats: Arc::clone(&stats),
        })?;
        session
            .commands
            .try_send(Command::SetQuantise(args.quantise))?;
        session.commands.try_send(Command::SetSilent(args.silent))?;
        session.output = Some(prepared.open(move |out, info| {
            engine.render(out, usize::from(info.channels));
        })?);
        session.mic = Some(kazoo_core::io::mic::open_mic(
            MicRequest {
                device: device(args.input),
                channel: args.input_channel,
            },
            MicTarget {
                rate: info.rate,
                frames: info.frames,
            },
            mic_tx,
            Arc::new(move |offered, accepted| {
                stats
                    .mic_dropped
                    .fetch_add((offered - accepted) as u64, Ordering::Relaxed);
            }),
        )?);
        Ok(session)
    }

    fn stop(&mut self) -> Result<()> {
        if self.output.is_none() && self.mic.is_none() && self.hub.is_none() && self.judge.is_none()
        {
            return Ok(());
        }
        drop(self.output.take());
        drop(self.mic.take());
        let joined = self.judge.take().map_or(Ok(()), |judge| {
            judge.join().map_err(|panic| {
                drop(panic);
                eyre!("mouth pitch judge panicked")
            })
        });
        drop(self.hub.take());
        let reported = match kazoo_core::host::link::exit_report() {
            Some(report) => writeln!(io::stderr().lock(), "{report}").map_err(Into::into),
            None => Ok(()),
        };
        combine(joined, reported)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            eprintln!("kazoo-mouth: stopping audio failed: {error}");
        }
    }
}

fn device(name: Option<String>) -> DeviceChoice {
    name.map_or(DeviceChoice::Default, DeviceChoice::Named)
}

fn combine(first: Result<()>, second: Result<()>) -> Result<()> {
    match (first, second) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(first), Err(second)) => Err(eyre!("{first}; also: {second}")),
    }
}

fn event_loop(
    session: &mut Session,
    terminal: &mut kazoo_host::terminal::Terminal,
    mut quantise: QuantiseSettings,
    silent: bool,
) -> Result<()> {
    let mut latest = None;
    let period = Duration::from_secs_f64(1.0 / 30.0);
    let mut draw_at = Instant::now();
    loop {
        if let Some(output) = &session.output {
            output.check_error()?;
        }
        if session.stats.shutdown.load(Ordering::Acquire) {
            return Ok(());
        }
        if session.stats.synth_fault.load(Ordering::Relaxed) {
            return Err(eyre!(
                "mouth synth refused its parameter table; output muted"
            ));
        }
        for frame in session.display.pop_iter().take(1024) {
            latest = Some(frame);
        }
        if Instant::now() >= draw_at {
            let status = status(session, latest, quantise, silent);
            terminal.draw(|frame| frame.render_widget(Paragraph::new(status), frame.area()))?;
            draw_at = Instant::now() + period;
        }
        if event::poll(draw_at.saturating_duration_since(Instant::now()))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Esc => return Ok(()),
                KeyCode::Char('c' | 'C') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(());
                }
                KeyCode::Char(' ') => {
                    quantise.blend = if quantise.blend > 0.0 { 0.0 } else { 1.0 };
                    session
                        .commands
                        .try_send(Command::SetQuantise(quantise))
                        .wrap_err("mouth command queue refused quantise change")?;
                }
                _ => {}
            }
        }
    }
}

fn status(
    session: &Session,
    latest: Option<VoiceFrame>,
    quantise: QuantiseSettings,
    silent: bool,
) -> String {
    let mut lines = vec!["the mouth — Space: quantise; Esc/Ctrl-C: quit".to_owned()];
    if let Some(frame) = latest {
        let semitones = frame.volts * 12.0;
        let nearest = semitones.round();
        let note = (nearest as i32 + 48).rem_euclid(12) as usize;
        let octave = (nearest as i32 + 48).div_euclid(12);
        lines.push(format!(
            "{}{} {:+.1} cents; direct {:.2} Hz; snapped {:.2} Hz",
            NOTE_NAMES[note],
            octave,
            (semitones - nearest) * 100.0,
            frame.hz,
            frame.snapped_hz
        ));
        lines.push(format!(
            "gate {}; loudness {:.3}; breath {:.3}; confidence {:.3}",
            if frame.gate { "on" } else { "off" },
            frame.loudness,
            frame.breath,
            frame.confidence
        ));
    } else {
        lines.push("waiting for a voice frame".into());
    }
    lines.push(format!(
        "quantise {}: {}:{}; sound {}",
        if quantise.blend > 0.0 { "on" } else { "off" },
        SCALE_NAMES[quantise.scale],
        NOTE_NAMES[usize::from(quantise.root)],
        if silent { "off" } else { "on" }
    ));
    if let Some(hub) = &session.hub {
        let state = hub.status();
        lines.push(if state.connected {
            if hub.desk_carries_voice() {
                state.strip.map_or_else(
                    || "on the desk, awaiting strip".into(),
                    |strip| format!("on the desk, strip {}", u16::from(strip) + 1),
                )
            } else {
                "the desk is too old to carry the voice".into()
            }
        } else {
            "playing direct: no desk".into()
        });
        let dropped = hub.voice_stats().dropped;
        if dropped > 0 {
            lines.push(format!("voice dropped: {dropped}"));
        }
    }
    if let Some(mic) = &session.mic {
        let mic = mic.status();
        lines.push(format!(
            "mic {:?}: {} at {} Hz → {} Hz; channel {:?}; reopens {}",
            mic.state,
            mic.device.as_str(),
            mic.device_rate,
            mic.engine_rate,
            mic.channel,
            mic.reopens
        ));
    }
    for (name, count) in [
        ("mic dropped", &session.stats.mic_dropped),
        ("mic skipped", &session.stats.mic_skipped),
        ("display dropped", &session.stats.display_dropped),
        ("desk audio refused", &session.stats.audio_refused),
    ] {
        let count = count.load(Ordering::Relaxed);
        if count > 0 {
            lines.push(format!("{name}: {count}"));
        }
    }
    lines.join("\n")
}
