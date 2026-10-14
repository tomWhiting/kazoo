use std::io::{self, BufWriter, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use color_eyre::Result;
use color_eyre::eyre::{WrapErr, eyre};
use crossbeam_channel::Sender;
use crossterm::event::{self, Event};
use kazoo_core::analysis::PitchDetectorConfig;
use kazoo_core::host::audio::{DeviceChoice, InputNotices, OpenedOutput, OutputRequest};
use kazoo_core::host::link::{LinkSetup, connect_or_direct};
use kazoo_core::io::mic::{Mic, MicRequest, MicTarget};
use kazoo_core::ipc::link::HubLink;
use kazoo_core::voice::VoiceFrame;
use kazoo_core::voice::judge::{self, Judge, OnsetConfig};
use kazoo_core::voice::quantise::{NOTE_NAMES, QuantiseSettings, SCALE_NAMES};
use ringbuf::traits::{Consumer, Split};
use ringbuf::{HeapCons, HeapRb};

use crate::controls::{Action, App, settings};
use crate::engine::looper::{LoopFeed, LoopOut};
use crate::engine::{Command, EngineSetup, MouthEngine, Stats};
use crate::ui::{self, Display};

mod terminal;

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

Up/Down choose a control. Left/Right change it; Shift makes a big step.
Space toggles the focused switch. w steps waveform. v toggles vibrato. s toggles silent. ? shows help.
Tab/Shift-Tab walks panels. 1..5 select FX. [/] choose slot; b bypasses; {/} move; d resets a knob.
,/. change own tempo by 1 BPM; </> by 10. The desk owns synced tempo.
r records or punches; u undoes; C clears; l chooses length; o stops/starts.
-/= change loop volume; _/+ make a big step; w saves ~/.kazoo/mouth/loops/.
Esc closes help or quits. Ctrl-C quits.
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
    if let Some(args) = parse_args(words).map_err(|error| eyre!(error))? {
        run(args)
    } else {
        write!(io::stdout().lock(), "{USAGE}")?;
        Ok(())
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
    let path = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| eyre!("HOME is missing: cannot locate mouth settings"))?
        .join(".kazoo/mouth/settings.json");
    let mut loaded = settings::load(&path);
    if args.quantise != QuantiseSettings::default() {
        loaded.settings.quantise = args.quantise.held();
        loaded.settings.last_blend = loaded.settings.quantise.blend;
    }
    loaded.settings.silent = args.silent;
    let mut session = Session::start(args, &loaded.settings)?;
    let mut terminal = kazoo_host::terminal::enter_with(terminal_options())?;
    let mut app = App::loaded(loaded, path, Instant::now());
    app.input_label.clone_from(&session.input_label);
    app.loop_sample_rate = session.rate;
    app.loop_feed = session.loop_feed.take();
    if let Some(reason) = terminal.mouse_refused() {
        app.message(format!("mouse capture refused: {reason}"), Instant::now());
    }
    let result = event_loop(&mut session, &mut terminal, &mut app);
    app.sync_stats(&session.stats, Instant::now());
    app.save_if_due(Instant::now(), true);
    let stopped = session.stop();
    terminal::finish(result, terminal, stopped)
}

struct Session {
    input_label: String,
    rate: u32,
    output: Option<OpenedOutput>,
    mic: Option<Mic>,
    hub: Option<HubLink>,
    judge: Option<Judge>,
    commands: Sender<Command>,
    display: HeapCons<VoiceFrame>,
    stats: Arc<Stats>,
    loop_feed: Option<LoopFeed>,
    loop_directory: PathBuf,
    writers: Vec<std::thread::JoinHandle<()>>,
    saved: crossbeam_channel::Receiver<String>,
    saved_tx: Sender<String>,
}

impl Session {
    fn start(args: Args, settings: &settings::Settings) -> Result<Self> {
        let prepared = OutputRequest {
            device: device(args.output),
            ..OutputRequest::default()
        }
        .prepare_output()?;
        let notices = InputNotices::default();
        let input = prepared.input_pick(args.input.as_deref(), &notices)?;
        let input_label = input.why.as_ref().map_or_else(
            || input.name.clone(),
            |why| format!("{} ({why})", input.name),
        );
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
        let loop_directory = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| eyre!("HOME is missing: cannot locate saved loops"))?
            .join(".kazoo/mouth/loops");
        let (saved_tx, saved) = crossbeam_channel::bounded(8);
        let mut session = Self {
            input_label,
            rate: info.rate,
            output: None,
            mic: None,
            hub: Some(hub),
            judge: Some(judge),
            commands,
            display,
            stats: Arc::clone(&stats),
            loop_feed: None,
            loop_directory,
            writers: Vec::new(),
            saved,
            saved_tx,
        };
        let mut engine = MouthEngine::with_settings(
            EngineSetup {
                rate: info.rate,
                block: info.frames,
                mic: mic_rx,
                commands: command_rx,
                display: display_tx,
                desk,
                judge: (feed, results),
                stats: Arc::clone(&stats),
            },
            settings,
        )?;
        session.loop_feed = engine.take_loop_feed();
        session.output = Some(prepared.open(move |out, info| {
            engine.render(out, usize::from(info.channels));
        })?);
        session.mic = Some(kazoo_core::io::mic::open_mic_with_notices(
            MicRequest {
                device: DeviceChoice::Named(input.name),
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
            notices,
        )?);
        Ok(session)
    }

    fn stop(&mut self) -> Result<()> {
        let writers = self.join_writers();
        if self.output.is_none() && self.mic.is_none() && self.hub.is_none() && self.judge.is_none()
        {
            return writers;
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
        let reported = kazoo_core::host::link::exit_report().map_or_else(
            || Ok(()),
            |report| writeln!(io::stderr().lock(), "{report}").map_err(Into::into),
        );
        combine(writers, combine(joined, reported))
    }

    fn join_writers(&mut self) -> Result<()> {
        let mut result = Ok(());
        for handle in self.writers.drain(..) {
            // Draining reports before joining leaves room for every worker to finish.
            for message in self.saved.try_iter() {
                eprintln!("kazoo-mouth: {message}");
            }
            if let Err(panic) = handle.join() {
                drop(panic);
                result = combine(result, Err(eyre!("mouth loop WAV writer panicked")));
            }
        }
        for message in self.saved.try_iter() {
            eprintln!("kazoo-mouth: {message}");
        }
        result
    }

    fn loop_returns(&mut self, app: &mut App, now: Instant) {
        for message in self.saved.try_iter() {
            app.message(message, now);
        }
        let mut index = 0;
        while index < self.writers.len() {
            if self.writers[index].is_finished() {
                let handle = self.writers.swap_remove(index);
                if let Err(panic) = handle.join() {
                    drop(panic);
                    app.message("loop not saved: WAV writer panicked".into(), now);
                }
            } else {
                index += 1;
            }
        }
        // Each live writer can report once without filling the report ring at shutdown.
        while self.writers.len() < 8 {
            let Some(item) = app.loop_feed.as_mut().and_then(|feed| feed.out.try_pop()) else {
                break;
            };
            app.save_returned();
            match item {
                LoopOut::Refused(samples) => {
                    drop(samples);
                    app.message("save cancelled: the loop changed".into(), now);
                }
                LoopOut::Bounced {
                    samples,
                    rate,
                    layers,
                } => {
                    let directory = self.loop_directory.clone();
                    let report = self.saved_tx.clone();
                    let worker = std::thread::Builder::new()
                        .name("mouth-loop-wav".into())
                        .spawn(move || {
                            let seconds = samples.len() as f64 / f64::from(rate);
                            let result = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .map_err(io::Error::other)
                                .and_then(|time| {
                                    save_loop(&directory, &samples, rate, time.as_secs())
                                });
                            let message = match result {
                                Ok(path) => path.file_name().map_or_else(
                                    || {
                                        format!(
                                            "loop not saved: saved path has no filename: {}",
                                            path.display()
                                        )
                                    },
                                    |name| {
                                        format!(
                                            "saved ~/.kazoo/mouth/loops/{} ({seconds:.2} s)",
                                            name.to_string_lossy()
                                        )
                                    },
                                ),
                                Err(error) => format!("loop not saved: {error} ({layers} layers)"),
                            };
                            if let Err(error) = report.send(message) {
                                eprintln!("kazoo-mouth: {}", error.0);
                            }
                        });
                    match worker {
                        Ok(handle) => self.writers.push(handle),
                        Err(error) => app.message(format!("loop not saved: {error}"), now),
                    }
                }
            }
        }
    }
}

fn reserve_loop(directory: &Path, seconds: u64) -> io::Result<(PathBuf, PathBuf)> {
    let mut suffix = 1_u64;
    loop {
        let name = if suffix == 1 {
            format!("mouth-loop-{seconds}.wav")
        } else {
            format!("mouth-loop-{seconds}-{suffix}.wav")
        };
        let path = directory.join(name);
        let reservation = directory.join(format!(".mouth-loop-{seconds}-{suffix}.reserve"));
        if !path.try_exists()? {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&reservation)
            {
                Ok(file) => {
                    drop(file);
                    // A completed writer may have released the name since the first lookup.
                    match path.try_exists() {
                        Ok(false) => return Ok((path, reservation)),
                        Ok(true) => std::fs::remove_file(&reservation)?,
                        Err(error) => {
                            std::fs::remove_file(&reservation).map_err(|cleanup| {
                                io::Error::other(format!(
                                    "{error}; reservation cleanup failed: {cleanup}"
                                ))
                            })?;
                            return Err(error);
                        }
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    suffix = suffix
                        .checked_add(1)
                        .ok_or_else(|| io::Error::other("loop filename suffix exhausted"))?;
                    continue;
                }
                Err(error) => return Err(error),
            }
        }
        suffix = suffix
            .checked_add(1)
            .ok_or_else(|| io::Error::other("loop filename suffix exhausted"))?;
    }
}

fn save_loop(directory: &Path, samples: &[f32], rate: u32, seconds: u64) -> io::Result<PathBuf> {
    if rate == 0 || samples.is_empty() || samples.iter().any(|sample| !sample.is_finite()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "loop needs finite samples and a positive rate",
        ));
    }
    let mut builder = std::fs::DirBuilder::new();
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700)
        .recursive(true)
        .create(directory)?;
    // A hidden reservation keeps concurrent saves separate without exposing an empty WAV.
    let (path, reservation) = reserve_loop(directory, seconds)?;
    let result = settings::atomic_write(&path, |file| {
        let mut writer = hound::WavWriter::new(
            BufWriter::new(file),
            hound::WavSpec {
                channels: 1,
                sample_rate: rate,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .map_err(io::Error::other)?;
        for sample in samples {
            writer.write_sample(*sample).map_err(io::Error::other)?;
        }
        writer.finalize().map_err(io::Error::other)
    });
    match (result, std::fs::remove_file(&reservation)) {
        (Ok(()), Ok(())) => Ok(path),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(cleanup)) => Err(io::Error::other(format!(
            "{error}; reservation cleanup failed: {cleanup}"
        ))),
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
    app: &mut App,
) -> Result<()> {
    let mut display = Display::new(session.rate);
    let period = Duration::from_secs_f64(1.0 / 30.0);
    let mut draw_at = Instant::now();
    let mut quitting = false;
    loop {
        if let Some(output) = &session.output {
            output.check_error()?;
        }
        if session.stats.shutdown.load(Ordering::Acquire) {
            quitting = true;
        }
        if quitting && !app.loop_save_pending() {
            return Ok(());
        }
        if session.stats.synth_fault.load(Ordering::Relaxed) {
            return Err(eyre!(
                "mouth synth refused its parameter table; output muted"
            ));
        }
        if Instant::now() >= draw_at {
            for voice in session.display.pop_iter().take(1024) {
                display.push(voice);
            }
            display.recentre();
            let link = link_state(session);
            let now = Instant::now();
            app.sync_stats(&session.stats, now);
            app.sync_loop(session.rate, now);
            session.loop_returns(app, now);
            app.save_if_due(now, false);
            if let Some(mic) = &session.mic {
                show_input_notice(app, mic.input_notices(), now);
            }
            terminal.draw(|frame| ui::draw(frame, app, &display, &link, now))?;
            draw_at = Instant::now() + period;
        }
        if event::poll(draw_at.saturating_duration_since(Instant::now()))? {
            let action = match event::read()? {
                Event::Key(key) if !quitting => app
                    .handle_key(key, &session.commands, Instant::now())
                    .wrap_err("mouth command channel disconnected")?,
                Event::Mouse(mouse) if !quitting => app
                    .handle_mouse(mouse, &session.commands, Instant::now())
                    .wrap_err("mouth command channel disconnected")?,
                _ => Action::Continue,
            };
            if action == Action::Quit {
                session
                    .commands
                    .try_send(Command::Shutdown)
                    .wrap_err("mouth shutdown command not sent; closing output")?;
                quitting = true;
                if app.loop_save_pending() {
                    app.message("finishing loop save".into(), Instant::now());
                }
            }
        }
    }
}

fn show_input_notice(app: &mut App, notices: &InputNotices, now: Instant) {
    if let Some(message) = notices.take() {
        app.message(message, now);
    }
}

fn link_state(session: &Session) -> String {
    let Some(hub) = &session.hub else {
        return "playing direct: no desk".into();
    };
    desk_status(&hub.status())
}

fn desk_status(state: &kazoo_core::ipc::link::LinkStatus) -> String {
    if !state.connected {
        state
            .desk_refusal()
            .or(state.last_refusal.as_deref())
            .map_or_else(
                || "playing direct: no desk".into(),
                |why| format!("not on the desk: {why}"),
            )
    } else if state.voice.desk_carries {
        state.strip.map_or_else(
            || "on the desk, awaiting strip".into(),
            |strip| format!("on the desk, strip {}", u16::from(strip) + 1),
        )
    } else {
        "the desk is too old to carry the voice".into()
    }
}

const fn terminal_options() -> kazoo_host::terminal::Options {
    kazoo_host::terminal::Options {
        mouse: true,
        key_release_events: false,
    }
}
