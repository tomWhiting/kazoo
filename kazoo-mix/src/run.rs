use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

use color_eyre::Result;
use color_eyre::eyre::eyre;
use crossterm::event::{self, Event, MouseEventKind};
use kazoo_core::host::audio::{OpenedOutput, OutputRequest};

use crate::callback::AudioCallback;
use crate::desk::Desk;
use crate::engine::MixerEngine;
use crate::hub::{Hub, HubConfig, HubNotice, HubStopped, LeaveReason, PATCH_CAPACITY};
use crate::patchbay::{PatchbayHub, patchbay};
use crate::shared::{DESK_CHANNELS, SharedState};
use crate::source::DemoEightOhEight;
use crate::terminal::{MixTerminal, TerminalGuard};
use crate::ui::{self, HubPriority, StatusInfo};
use kazoo_core::ipc::link::LinkStatus;
use kazoo_core::realtime::NotRealTime;

/// Desk redraw interval (~30 fps).
const UI_TICK: Duration = Duration::from_millis(33);

/// How long hub news stays in the header.
const NEWS_SECONDS: u64 = 8;

/// Help text for the studio desk.
pub const USAGE: &str = "\
kazoo-mix: the kazoo studio desk

Usage: kazoo mix [--demo]
       kazoo-mix [--demo]

Start kazoo mix, then start instruments (kazoo 808, kazoo mini, kazoo cs80,
kazoo dx, kazoo arp, kazoo mouth) in other terminals: each plugs into the next free strip
and follows the desk's tempo and play/stop.

Options:
  --demo     put a built-in 808 pattern on strip 1
  -h, --help show this help
";

/// Command-line choices.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Args {
    /// Put a built-in 808 pattern on strip 1.
    pub demo: bool,
}

/// Read the arguments after the program name. `Ok(None)` means help was
/// asked for, wherever it stands; `Err` says what was wrong (the caller adds
/// the usage).
pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<Args>, String> {
    let mut parsed = Args::default();
    let mut unknown = None;
    for arg in args {
        match arg.as_str() {
            "--demo" => parsed.demo = true,
            "-h" | "--help" => return Ok(None),
            _ => {
                if unknown.is_none() {
                    unknown = Some(arg);
                }
            }
        }
    }
    unknown.map_or(Ok(Some(parsed)), |other| {
        Err(format!("unknown option `{other}`"))
    })
}

/// Print help or start the desk from command-line arguments.
pub fn cli(args: impl IntoIterator<Item = String>) -> Result<()> {
    match parse_args(args) {
        Ok(None) => {
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(USAGE.as_bytes())?;
            stdout.flush()?;
            Ok(())
        }
        Ok(Some(args)) => run(args),
        Err(why) => Err(eyre!("{why}\n\n{USAGE}")),
    }
}

/// Run the studio desk until it quits: audio, instrument hub and terminal.
///
/// Error reports must already be installed by the caller with
/// `color_eyre::install`; this never installs them.
pub fn run(args: Args) -> Result<()> {
    // Open audio and the hub before taking over the terminal, so a failure
    // prints a readable error instead of vanishing with the alternate screen.
    let (audio, patchbay) = MixerAudio::start()?;
    let hub = Hub::start(
        HubConfig::standard(audio.sample_rate, [false; DESK_CHANNELS]),
        Arc::clone(&audio.shared),
        patchbay,
    )?;
    // The demo plugs into the hub like any instrument.
    let demo = if args.demo {
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
        audio.stream.check_error()?;
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
                hub_priority: hub_priority(hub.real_time()),
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
                "{demo}no instruments yet: run kazoo mouth, kazoo 808, kazoo mini, kazoo cs80, \
                 kazoo dx, kazoo arp or another kazoo instrument in another terminal"
            ),
            1 => format!("{demo}1 instrument plugged in"),
            n => format!("{demo}{n} instruments plugged in"),
        };
        (line, false)
    }
}

/// How the hub's thread is scheduled, from what it said when it asked.
const fn hub_priority(outcome: Option<&Result<(), NotRealTime>>) -> HubPriority {
    match outcome {
        None => HubPriority::Asking,
        Some(Ok(())) => HubPriority::RealTime,
        Some(Err(_)) => HubPriority::Normal,
    }
}

/// What the header says about the built-in demo, and whether it is on the
/// desk.
fn demo_line(status: &LinkStatus) -> (String, bool) {
    let priority = if status.normal_priority {
        ", desk link at normal priority"
    } else {
        ""
    };
    if let Some(strip) = status.strip.filter(|_| status.connected) {
        return (
            format!("808 demo on strip {}{priority} · ", u16::from(strip) + 1),
            true,
        );
    }
    let line = status.last_refusal.as_deref().map_or_else(
        || format!("808 demo plugging in{priority} · "),
        |why| format!("808 demo not plugged in: {why}{priority} · "),
    );
    (line, false)
}

/// The running audio device and shared desk state.
struct MixerAudio {
    stream: OpenedOutput,
    shared: Arc<SharedState>,
    sample_rate: u32,
    channels: u16,
}

impl MixerAudio {
    /// Open the default output device and start the desk's callback. Returns
    /// the hub's end of the patchbay the callback listens on.
    fn start() -> Result<(Self, PatchbayHub)> {
        let request = OutputRequest::default();
        let prepared = request.prepare_output()?;
        let info = prepared.info();

        let shared = Arc::new(SharedState::new());
        let engine = MixerEngine::new(DESK_CHANNELS, info.rate)
            .map_err(|err| eyre!("failed to build the mixer engine: {err:?}"))?;
        let (patchbay_hub, patchbay_callback) = patchbay(PATCH_CAPACITY);
        let mut callback = AudioCallback::new(engine, Arc::clone(&shared), info.channels)
            .with_patchbay(patchbay_callback);
        let stream = prepared.open(move |data, _| callback.process(data))?;

        Ok((
            Self {
                stream,
                shared,
                sample_rate: info.rate,
                channels: info.channels,
            },
            patchbay_hub,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Option<Args>, String> {
        parse_args(args.iter().map(ToString::to_string))
    }

    #[test]
    fn arguments_choose_the_demo_or_help() {
        assert_eq!(parse(&[]), Ok(Some(Args { demo: false })));
        assert_eq!(parse(&["--demo"]), Ok(Some(Args { demo: true })));
        assert_eq!(parse(&["--help"]), Ok(None));
        assert_eq!(parse(&["--demo", "-h"]), Ok(None));
        let err = parse(&["--nope"]).unwrap_err();
        assert!(err.contains("unknown option `--nope`"), "{err}");
        assert!(!err.contains(USAGE));
    }

    #[test]
    fn help_wins_wherever_it_stands() {
        assert_eq!(parse(&["--bogus", "--help"]), Ok(None));
        assert_eq!(parse(&["-h", "--bogus"]), Ok(None));
    }

    #[test]
    fn usage_names_the_desk() {
        assert!(USAGE.contains("kazoo-mix"));
    }

    #[test]
    fn usage_opens_with_the_name_and_description() {
        assert!(USAGE.starts_with("kazoo-mix: the kazoo studio desk\n"));
    }
    #[test]
    fn the_header_shows_the_hubs_priority_as_it_said() {
        assert_eq!(hub_priority(None), HubPriority::Asking);
        assert_eq!(hub_priority(Some(&Ok(()))), HubPriority::RealTime);
        let refused = kazoo_core::realtime::RealTime::promote(std::time::Duration::ZERO)
            .map(drop)
            .expect_err("a zero period is refused");
        assert_eq!(hub_priority(Some(&Err(refused))), HubPriority::Normal);
    }

    #[test]
    fn the_demo_line_keeps_its_strip_at_normal_priority() {
        let status = LinkStatus {
            normal_priority: true,
            connected: true,
            strip: Some(2),
            blocks_sent: 0,
            blocks_dropped: 0,
            messages_dropped: 0,
            connections: 1,
            last_refusal: None,
        };
        assert_eq!(
            demo_line(&status),
            (
                "808 demo on strip 3, desk link at normal priority \u{b7} ".to_string(),
                true
            )
        );
    }
}
