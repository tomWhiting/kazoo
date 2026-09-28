//! kazoo-mix — terminal studio mixing desk.
//!
//! This binary owns the local audio device and wires the pieces from the
//! library together: the [`AudioCallback`] that runs the engine inside the
//! device callback, the instrument [`Hub`] that plugs kazoo's instruments into
//! the desk's strips, the lock-free [`SharedState`] they share with the desk,
//! and the desk UI itself.

use std::sync::Arc;
use std::time::{Duration, Instant};

use color_eyre::Result;
use color_eyre::eyre::eyre;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossterm::event::{self, Event, MouseEventKind};

use kazoo_core::ipc::link::LinkStatus;
use kazoo_mix::callback::AudioCallback;
use kazoo_mix::desk::Desk;
use kazoo_mix::engine::MixerEngine;
use kazoo_mix::hub::{Hub, HubConfig, HubNotice, HubStopped, LeaveReason, PATCH_CAPACITY};
use kazoo_mix::patchbay::{PatchbayCallback, PatchbayHub, patchbay};
use kazoo_mix::shared::{DESK_CHANNELS, SharedState};
use kazoo_mix::source::DemoEightOhEight;
use kazoo_mix::terminal::{MixTerminal, TerminalGuard};
use kazoo_mix::ui::{self, StatusInfo};

/// Desk redraw interval (~30 fps).
const UI_TICK: Duration = Duration::from_millis(33);

/// How long hub news stays in the header.
const NEWS_SECONDS: u64 = 8;

const USAGE: &str = "\
kazoo-mix — the kazoo studio desk

Usage: kazoo-mix [--demo]

Start kazoo-mix, then start instruments (kazoo-808, kazoo-mini, kazoo-cs80,
kazoo-dx, kazoo-arp) in other terminals: each plugs into the next free strip
and follows the desk's tempo and play/stop.

Options:
  --demo     put a built-in 808 pattern on strip 1
  -h, --help show this help
";

/// Command-line choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Options {
    demo: bool,
}

/// Parse arguments; `None` means help was asked for.
fn parse_args(args: impl Iterator<Item = String>) -> Result<Option<Options>> {
    let mut options = Options { demo: false };
    for arg in args {
        match arg.as_str() {
            "--demo" => options.demo = true,
            "-h" | "--help" => return Ok(None),
            other => return Err(eyre!("unknown option `{other}`\n\n{USAGE}")),
        }
    }
    Ok(Some(options))
}

fn main() -> Result<()> {
    color_eyre::install()?;
    let Some(options) = parse_args(std::env::args().skip(1))? else {
        print!("{USAGE}");
        return Ok(());
    };

    // Open audio and the hub before taking over the terminal, so a failure
    // prints a readable error instead of vanishing with the alternate screen.
    let (audio, patchbay) = MixerAudio::start()?;
    let hub = Hub::start(
        HubConfig::standard(audio.sample_rate, [false; DESK_CHANNELS]),
        Arc::clone(&audio.shared),
        patchbay,
    )?;
    // The demo plugs into the hub like any instrument.
    let demo = if options.demo {
        Some(DemoEightOhEight::start(
            hub.socket().to_path_buf(),
            audio.sample_rate,
            Arc::clone(&audio.shared),
        )?)
    } else {
        None
    };
    let mut terminal_guard = TerminalGuard::enter();
    let mouse = terminal_guard.mouse_enabled();

    let result = terminal_guard.terminal_mut().map_or_else(
        || Err(eyre!("terminal was restored before the desk started")),
        |terminal| run_desk(terminal, &audio, &hub, demo.as_ref(), mouse),
    );

    drop(demo);
    drop(hub);
    drop(audio);
    terminal_guard.restore()?;
    result
}

fn run_desk(
    terminal: &mut MixTerminal,
    audio: &MixerAudio,
    hub: &Hub,
    demo: Option<&DemoEightOhEight>,
    mouse: bool,
) -> Result<()> {
    let mut desk = Desk::new();
    let mut news = HubNews::default();
    let mut last_tick = Instant::now();
    let mut redraw = true;

    while !desk.should_quit {
        let now = Instant::now();
        let since_tick = now.duration_since(last_tick);
        if since_tick >= UI_TICK {
            desk.tick(&audio.shared, since_tick.as_secs_f32());
            news.collect(hub);
            last_tick = now;
            redraw = true;
        }

        if redraw {
            let (note, note_alert) = news.line(hub, demo);
            let status = StatusInfo {
                sample_rate: audio.sample_rate,
                channels: audio.channels,
                buffer_frames: audio.shared.callback_frames(),
                note,
                note_alert,
                stream_errors: audio.shared.stream_errors(),
                engine_faults: audio.shared.engine_faults(),
                mouse,
            };
            terminal.draw(|frame| ui::draw(frame, &mut desk, &audio.shared, &status))?;
            redraw = false;
        }

        // Wait for input until the next meter tick. Bare pointer movement is
        // reported constantly while mouse capture is on; it changes nothing,
        // so it never forces a redraw.
        let wait = UI_TICK.saturating_sub(last_tick.elapsed());
        if event::poll(wait)? {
            match event::read()? {
                Event::Key(key) => {
                    desk.handle_key(key, &audio.shared);
                    redraw = true;
                }
                Event::Mouse(mouse) if mouse.kind != MouseEventKind::Moved => {
                    desk.handle_mouse(mouse, &audio.shared);
                    redraw = true;
                }
                Event::Resize(..) => redraw = true,
                // Bare pointer movement, focus and paste events change
                // nothing on the desk.
                Event::Mouse(_) | Event::FocusGained | Event::FocusLost | Event::Paste(_) => {}
            }
        }
    }

    Ok(())
}

/// The latest thing the hub had to say, for the header.
#[derive(Debug, Default)]
struct HubNews {
    latest: Option<(HubNotice, Instant)>,
    stopped: bool,
}

impl HubNews {
    fn collect(&mut self, hub: &Hub) {
        loop {
            match hub.next_notice() {
                Ok(Some(notice)) => self.latest = Some((notice, Instant::now())),
                Ok(None) => return,
                Err(HubStopped) => {
                    self.stopped = true;
                    return;
                }
            }
        }
    }

    /// The header line and whether it is a warning.
    fn line(&self, hub: &Hub, demo: Option<&DemoEightOhEight>) -> (String, bool) {
        if self.stopped || !hub.is_running() {
            return (
                "the instrument hub stopped: restart kazoo-mix to plug instruments in".to_string(),
                true,
            );
        }
        if let Some((notice, at)) = &self.latest {
            if at.elapsed() < Duration::from_secs(NEWS_SECONDS) {
                let alert = matches!(
                    notice,
                    HubNotice::Refused { .. }
                        | HubNotice::Fault(_)
                        | HubNotice::Left {
                            reason: LeaveReason::Disconnected(_)
                                | LeaveReason::Protocol(_)
                                | LeaveReason::Stuck
                                | LeaveReason::Engine(_),
                            ..
                        }
                );
                return (notice.to_string(), alert);
            }
        }
        let (demo, demo_connected) =
            demo.map_or_else(|| (String::new(), false), |demo| demo_line(&demo.status()));
        let others = hub
            .snapshot()
            .instruments
            .saturating_sub(u64::from(demo_connected));
        let line = match others {
            0 => format!(
                "{demo}no instruments yet: run kazoo-tui, kazoo-808, kazoo-mini, kazoo-cs80, \
                 kazoo-dx or another kazoo instrument in another terminal"
            ),
            1 => format!("{demo}1 instrument plugged in"),
            n => format!("{demo}{n} instruments plugged in"),
        };
        (line, false)
    }
}

/// What the header says about the built-in demo, and whether it is on the
/// desk.
fn demo_line(status: &LinkStatus) -> (String, bool) {
    if let Some(strip) = status.strip.filter(|_| status.connected) {
        return (
            format!("808 demo on strip {} · ", u16::from(strip) + 1),
            true,
        );
    }
    let line = status.last_refusal.as_deref().map_or_else(
        || "808 demo plugging in · ".to_string(),
        |why| format!("808 demo not plugged in: {why} · "),
    );
    (line, false)
}

/// The running audio device and shared desk state.
struct MixerAudio {
    _stream: cpal::Stream,
    shared: Arc<SharedState>,
    sample_rate: u32,
    channels: u16,
}

impl MixerAudio {
    /// Open the default output device and start the desk's callback. Returns
    /// the hub's end of the patchbay the callback listens on.
    fn start() -> Result<(Self, PatchbayHub)> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| eyre!("no default output audio device found"))?;
        let supported = device.default_output_config()?;
        let sample_format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();

        let shared = Arc::new(SharedState::new());
        let engine = MixerEngine::new(DESK_CHANNELS, config.sample_rate)
            .map_err(|err| eyre!("failed to build the mixer engine: {err:?}"))?;
        let (patchbay_hub, patchbay_callback) = patchbay(PATCH_CAPACITY);
        let stream = match sample_format {
            cpal::SampleFormat::F32 => {
                build_output_stream::<f32>(&device, &config, &shared, engine, patchbay_callback)?
            }
            cpal::SampleFormat::I16 => {
                build_output_stream::<i16>(&device, &config, &shared, engine, patchbay_callback)?
            }
            cpal::SampleFormat::U16 => {
                build_output_stream::<u16>(&device, &config, &shared, engine, patchbay_callback)?
            }
            other => return Err(eyre!("unsupported output sample format: {other:?}")),
        };
        stream.play()?;

        Ok((
            Self {
                _stream: stream,
                shared,
                sample_rate: config.sample_rate,
                channels: config.channels,
            },
            patchbay_hub,
        ))
    }
}

fn build_output_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    shared: &Arc<SharedState>,
    engine: MixerEngine,
    patchbay: PatchbayCallback,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    // Everything the callback touches is allocated here, before the stream
    // starts.
    let mut callback =
        AudioCallback::new(engine, Arc::clone(shared), config.channels).with_patchbay(patchbay);
    let error_shared = Arc::clone(shared);

    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| callback.process(data),
        move |_err| {
            // Printing here would corrupt the desk; the header shows the count.
            error_shared.note_stream_error();
        },
        None,
    )?;

    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Option<Options>> {
        parse_args(args.iter().map(ToString::to_string))
    }

    #[test]
    fn arguments_choose_the_demo_or_help() {
        assert_eq!(parse(&[]).unwrap(), Some(Options { demo: false }));
        assert_eq!(parse(&["--demo"]).unwrap(), Some(Options { demo: true }));
        assert_eq!(parse(&["--help"]).unwrap(), None);
        let err = parse(&["--nope"]).unwrap_err().to_string();
        assert!(err.contains("unknown option `--nope`"), "{err}");
    }
}
