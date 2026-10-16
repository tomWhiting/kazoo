//! The wall follows the desk's rate (KZ-D21).
//!
//! The desk takes only instruments at its own rate. The wall opens its
//! device at whatever rate the device is set to, and keeps it for as long
//! as it plays. So a wall started on a 48 kHz device never plugged into a
//! desk started later on a 44.1 kHz one (Tom, 16 Oct 2026: "the mixer and
//! wall aren't connecting"). Now, while a desk the wall joins says a rate
//! other than the wall's, the wall opens the audio again: on the default
//! device as it is set (never switching its rate, which is the owner's),
//! and when that still differs, on the headless loop at the desk's rate,
//! which the desk plays. When the desk has gone (two looks in a row, a
//! retry apart), the wall goes back to its device. The music carries on
//! from the bar it was at.
//!
//! The desk is looked for at most once per [`DEVICE_RETRY`], and not at
//! all while the wall is plugged into it: a look reads the desk's record
//! and knocks on its socket.

use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::Instant;

use kazoo_core::ipc::discovery;
use kazoo_core::ipc::link::HubLink;

use super::super::DeskMode;
use super::super::audio::{AudioMode, Device};
use super::{Control, DEVICE_RETRY, play_on, start_headless};

/// Looks in a row that find no desk before the wall, following one, goes
/// back to its device: one look can land while the desk is starting again.
const GONE_AFTER: u8 = 2;

/// The rate a desk the wall joins runs at, when one is up and says; why
/// not, when the desk's record or socket cannot be read.
fn desk_rate(desk: &DeskMode) -> Result<Option<u32>, String> {
    let record = match discovery::read_pid_file() {
        Ok(Some(record)) => record,
        Ok(None) => return Ok(None),
        Err(err) => return Err(format!("the desk's record does not read: {err}")),
    };
    let ours = match desk {
        DeskMode::Off => false,
        DeskMode::Discover => true,
        DeskMode::Socket(path) => same_socket(path, &record.socket),
    };
    if !ours {
        return Ok(None);
    }
    match discovery::hub_listening(&record.socket) {
        Ok(true) => Ok(record.sample_rate),
        Ok(false) => Ok(None),
        Err(err) => Err(format!("the desk's socket cannot be tried: {err}")),
    }
}

/// Whether `a` and `b` name the same socket: the same path, or the same
/// file reached by two (`/tmp` and `/private/tmp` on macOS, a link, a
/// relative path). A path that names nothing is no desk's socket.
fn same_socket(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        (Err(_), _) | (_, Err(_)) => false,
    }
}

/// What following the desk's rate means for the audio now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Follow {
    /// Nothing to do.
    Stay,
    /// Open the audio again to match a desk at this rate.
    Match(u32),
    /// The desk the wall followed has gone: back to the device.
    Device,
}

/// What to do, given the desk's rate (`desk`), the wall's (`wall`), the
/// rate the wall is playing at for a desk on the headless loop
/// (`following`), the desk and wall rates it last opened the audio again
/// for (`tried`, so a device that cannot match is not opened again and
/// again), and how many looks in a row found no desk (`missed`).
fn follow(
    desk: Option<u32>,
    wall: u32,
    following: Option<u32>,
    tried: Option<(u32, u32)>,
    missed: u8,
) -> Follow {
    match desk {
        Some(desk) if desk != wall && tried != Some((desk, wall)) => Follow::Match(desk),
        None if following.is_some() && missed >= GONE_AFTER => Follow::Device,
        Some(_) | None => Follow::Stay,
    }
}

/// Following the rate of a desk the wall joins.
#[derive(Debug, Default)]
pub(super) struct Watch {
    /// The desk's rate the wall renders at on the headless loop, when its
    /// device plays at another.
    following: Option<u32>,
    /// The desk's and the wall's rates the audio was last opened again
    /// for.
    tried: Option<(u32, u32)>,
    /// When the desk is next looked for.
    due: Option<Instant>,
    /// Looks in a row that found no desk.
    missed: u8,
    /// Why the last look could not be made, said once until it changes.
    said: Option<String>,
}

impl Watch {
    /// The desk's rate the wall renders at on the headless loop, if it
    /// does.
    pub(super) const fn following(&self) -> Option<u32> {
        self.following
    }

    /// Forget the rates the audio was last opened again for: it is opening
    /// again for another reason, and may come back at any rate.
    pub(super) const fn forget_tried(&mut self) {
        self.tried = None;
    }

    /// One round at `now` for a wall at `wall` Hz, plugged into the desk or
    /// not: `look` finds the desk's rate when a look is due. What to do,
    /// and a line to log when a look could not be made for a new reason.
    fn round(
        &mut self,
        now: Instant,
        wall: u32,
        plugged_in: bool,
        look: impl FnOnce() -> Result<Option<u32>, String>,
    ) -> (Follow, Option<String>) {
        if plugged_in {
            // Plugged in, the desk is up and at the wall's rate.
            self.missed = 0;
            return (Follow::Stay, None);
        }
        if self.due.is_some_and(|due| now < due) {
            return (Follow::Stay, None);
        }
        self.due = Some(now + DEVICE_RETRY);
        let (desk, say) = match look() {
            Ok(desk) => {
                self.said = None;
                (desk, None)
            }
            Err(why) => {
                let say = (self.said.as_ref() != Some(&why)).then(|| why.clone());
                self.said = Some(why);
                (None, say)
            }
        };
        self.missed = if desk.is_some() {
            0
        } else {
            self.missed.saturating_add(1)
        };
        let next = follow(desk, wall, self.following, self.tried, self.missed);
        match next {
            Follow::Stay => {}
            Follow::Match(rate) => self.tried = Some((rate, wall)),
            Follow::Device => {
                self.following = None;
                self.tried = None;
                self.missed = 0;
            }
        }
        (next, say)
    }
}

impl Control {
    /// Follow the desk's rate when it differs from the wall's (see the
    /// module's words). Only a wall on the device at the rate the device is
    /// set to, joining a desk, follows: a rate or the headless loop asked
    /// for is kept.
    pub(super) fn follow_desk_rate(&mut self, now: Instant) {
        if self.audio != (AudioMode::Device { sample_rate: None }) || self.desk == DeskMode::Off {
            return;
        }
        let wall = self.wall.engine_mut().sample_rate();
        let plugged_in = self.wall.desk_link().is_some_and(HubLink::is_connected);
        let desk = &self.desk;
        let (next, say) = self
            .desk_rate
            .round(now, wall, plugged_in, || desk_rate(desk));
        if let Some(why) = say {
            eprintln!("kazoo-wall: {why}");
        }
        match next {
            Follow::Stay => {}
            Follow::Match(rate) => {
                eprintln!(
                    "kazoo-wall: the desk runs at {rate} Hz and the wall at {wall} Hz; opening the audio again to match"
                );
                self.match_desk(rate, now);
            }
            Follow::Device => {
                eprintln!("kazoo-wall: the desk has gone; back to the audio device");
                self.rebuild(now);
            }
        }
    }

    /// Open the audio again for a desk at `rate`: on the device as it is
    /// set, when that is `rate`; otherwise on the headless loop at `rate`.
    /// The device is opened first and its rate read, so the engine and its
    /// desk link are made once, at the rate they keep.
    fn match_desk(&mut self, rate: u32, now: Instant) {
        self.replace_audio(now, |control, at| {
            match Device::open(None) {
                Ok(device) if device.sample_rate() == rate => {
                    match play_on(&device, &control.desk, at, control.heard, &control.listen_tx) {
                        Ok(started) => {
                            control.desk_rate.following = None;
                            return started;
                        }
                        Err(why) => eprintln!(
                            "kazoo-wall: {why}; the wall renders at {rate} Hz for the desk to play"
                        ),
                    }
                }
                Ok(device) => eprintln!(
                    "kazoo-wall: {} plays at {} Hz; the wall renders at {rate} Hz for the desk to play",
                    device.name(),
                    device.sample_rate()
                ),
                Err(why) => eprintln!(
                    "kazoo-wall: {why}; the wall renders at {rate} Hz for the desk to play"
                ),
            }
            control.desk_rate.following = Some(rate);
            start_headless(
                &control.desk,
                rate,
                at,
                control.heard,
                &control.listen_tx,
                None,
            )
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_desk_at_another_rate_is_matched_once_and_a_gone_desk_gives_the_device_back() {
        // The wall at 48 kHz, a desk at 44.1 kHz: match it.
        assert_eq!(
            follow(Some(44_100), 48_000, None, None, 0),
            Follow::Match(44_100)
        );
        // Tried already for these two rates (the device could not): stay.
        assert_eq!(
            follow(Some(44_100), 48_000, None, Some((44_100, 48_000)), 0),
            Follow::Stay
        );
        // The same rate: nothing to do.
        assert_eq!(follow(Some(48_000), 48_000, None, None, 0), Follow::Stay);
        // Following a desk on the headless loop at its rate: stay while it
        // is up, and after one look that misses it; back to the device
        // after two.
        assert_eq!(
            follow(
                Some(44_100),
                44_100,
                Some(44_100),
                Some((44_100, 48_000)),
                0
            ),
            Follow::Stay
        );
        assert_eq!(follow(None, 44_100, Some(44_100), None, 1), Follow::Stay);
        assert_eq!(follow(None, 44_100, Some(44_100), None, 2), Follow::Device);
        // No desk and not following one: nothing.
        assert_eq!(follow(None, 48_000, None, None, 5), Follow::Stay);
        // A desk that comes back at yet another rate is matched again.
        assert_eq!(
            follow(Some(96_000), 44_100, None, Some((44_100, 48_000)), 0),
            Follow::Match(96_000)
        );
    }

    /// A look that must not be made.
    fn no_look() -> Result<Option<u32>, String> {
        panic!("the desk was looked for when no look was due")
    }

    #[test]
    fn the_desk_is_looked_for_once_a_retry_and_never_while_plugged_in() {
        let start = Instant::now();
        let mut watch = Watch::default();
        let mut looks = 0;
        let mut look = || {
            looks += 1;
            Ok(Some(48_000))
        };
        assert_eq!(
            watch.round(start, 48_000, false, &mut look),
            (Follow::Stay, None)
        );
        // Every 20 ms tick until the retry is up: no look.
        for tick in 1..100 {
            let now = start + Duration::from_millis(20) * tick;
            assert_eq!(
                watch.round(now, 48_000, false, no_look),
                (Follow::Stay, None)
            );
        }
        watch.round(start + DEVICE_RETRY, 48_000, false, &mut look);
        // Plugged in: never looked for, however long.
        for tick in 0..10 {
            let now = start + DEVICE_RETRY * (2 + tick);
            assert_eq!(
                watch.round(now, 48_000, true, no_look),
                (Follow::Stay, None)
            );
        }
        assert_eq!(looks, 2);
    }

    #[test]
    fn why_a_look_failed_is_said_once_until_it_changes() {
        let start = Instant::now();
        let mut watch = Watch::default();
        let mut at = start;
        let mut round = |watch: &mut Watch, result: Result<Option<u32>, String>| {
            let said = watch.round(at, 48_000, false, || result).1;
            at += DEVICE_RETRY;
            said
        };
        let denied =
            || Err::<Option<u32>, _>("the desk's record does not read: denied".to_string());
        assert_eq!(
            round(&mut watch, denied()).as_deref(),
            Some("the desk's record does not read: denied")
        );
        assert_eq!(round(&mut watch, denied()), None);
        assert_eq!(round(&mut watch, denied()), None);
        assert_eq!(
            round(
                &mut watch,
                Err("the desk's socket cannot be tried: refused".to_string())
            )
            .as_deref(),
            Some("the desk's socket cannot be tried: refused")
        );
        // A look that works, then the first failure again: said again.
        assert_eq!(round(&mut watch, Ok(None)), None);
        assert!(round(&mut watch, denied()).is_some());
    }

    #[test]
    fn a_followed_desk_is_gone_only_after_two_looks_a_retry_apart() {
        let start = Instant::now();
        let mut watch = Watch {
            following: Some(44_100),
            tried: Some((44_100, 48_000)),
            ..Watch::default()
        };
        // Up and plugged in, then unplugged: the first look misses it.
        watch.round(start, 44_100, true, no_look);
        assert_eq!(
            watch.round(start, 44_100, false, || Ok(None)),
            (Follow::Stay, None)
        );
        // Ticks before the retry: no look, still following.
        assert_eq!(
            watch.round(start + DEVICE_RETRY / 2, 44_100, false, no_look),
            (Follow::Stay, None)
        );
        // The desk came back in between: the count starts again.
        assert_eq!(
            watch.round(start + DEVICE_RETRY, 44_100, false, || Ok(Some(44_100))),
            (Follow::Stay, None)
        );
        assert_eq!(
            watch.round(start + DEVICE_RETRY * 2, 44_100, false, || Ok(None)),
            (Follow::Stay, None)
        );
        assert_eq!(watch.following(), Some(44_100));
        // A second look in a row, a retry later, misses it: back to the
        // device, and everything about the desk is forgotten.
        assert_eq!(
            watch.round(start + DEVICE_RETRY * 3, 44_100, false, || Ok(None)),
            (Follow::Device, None)
        );
        assert_eq!(watch.following(), None);
        assert_eq!(watch.tried, None);
        assert_eq!(watch.missed, 0);
    }

    /// A control thread's state with no backend yet, its audio `audio`,
    /// joining `desk`, the wall's engine at 48 kHz.
    fn control(audio: AudioMode, desk: DeskMode) -> Control {
        use super::super::{AudioCounts, link_diagnostics};
        use crate::engine::{EngineConfig, engine};
        use std::collections::HashMap;
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;
        let (_engine, engine_control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
        let wall = super::super::Wall::new(
            crate::seed::seed().unwrap(),
            engine_control,
            None,
            Vec::new(),
            None,
        );
        let (listen_tx, listen_rx) = std::sync::mpsc::channel();
        let now = Instant::now();
        Control {
            wall,
            conns: HashMap::new(),
            audio,
            desk,
            backend: None,
            listen_tx,
            listen_rx,
            stop: Arc::new(AtomicBool::new(false)),
            stopping: false,
            heard: false,
            last_tick: now,
            awaiting: HashMap::new(),
            counts: AudioCounts::new(now),
            told_link_refused: false,
            desk_refusals: link_diagnostics::DeskRefusals::default(),
            reported_link_shutdowns: link_diagnostics::LinkShutdowns::default(),
            desk_rate: Watch::default(),
        }
    }

    fn renders_headless_at(control: &mut Control, rate: u32) {
        assert_eq!(control.wall.engine_mut().sample_rate(), rate);
        assert!(matches!(
            control.backend.as_ref().map(|backend| &backend.running),
            Some(super::super::Running::Headless(_))
        ));
    }

    #[test]
    fn a_render_loop_rendering_for_a_desk_starts_again_at_the_desks_rate() {
        let mut control = control(AudioMode::Device { sample_rate: None }, DeskMode::Off);
        control.desk_rate.following = Some(44_100);
        control.desk_rate.tried = Some((44_100, 48_000));
        control.rebuild(Instant::now());
        // Never back on the device while the desk is up: the render loop
        // at the desk's rate, with nothing to retry.
        renders_headless_at(&mut control, 44_100);
        assert_eq!(
            control
                .backend
                .as_ref()
                .and_then(|backend| backend.retry_at),
            None
        );
        assert_eq!(control.desk_rate.following(), Some(44_100));
        assert_eq!(control.desk_rate.tried, Some((44_100, 48_000)));
    }

    #[test]
    fn audio_opened_again_for_another_reason_forgets_the_rates_tried() {
        let mut control = control(
            AudioMode::Headless {
                sample_rate: 48_000,
            },
            DeskMode::Off,
        );
        control.desk_rate.tried = Some((44_100, 48_000));
        control.rebuild(Instant::now());
        renders_headless_at(&mut control, 48_000);
        assert_eq!(control.desk_rate.tried, None);
    }

    #[test]
    fn only_a_wall_on_its_device_as_set_joining_a_desk_looks_for_one() {
        let now = Instant::now();
        for (audio, desk) in [
            (AudioMode::Device { sample_rate: None }, DeskMode::Off),
            (
                AudioMode::Device {
                    sample_rate: Some(48_000),
                },
                DeskMode::Discover,
            ),
            (
                AudioMode::Headless {
                    sample_rate: 48_000,
                },
                DeskMode::Discover,
            ),
        ] {
            let mut control = control(audio, desk);
            control.follow_desk_rate(now);
            assert_eq!(control.desk_rate.due, None, "{audio:?}");
            assert!(control.backend.is_none());
        }
    }

    #[test]
    fn a_socket_reached_by_another_path_is_the_same_socket() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("hub.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let link = dir.path().join("link.sock");
        std::os::unix::fs::symlink(&socket, &link).unwrap();
        let canonical = std::fs::canonicalize(&socket).unwrap();
        assert!(same_socket(&socket, &socket));
        assert!(same_socket(&link, &socket));
        assert!(same_socket(&canonical, &socket));
        let other = dir.path().join("other.sock");
        let _other = std::os::unix::net::UnixListener::bind(&other).unwrap();
        assert!(!same_socket(&other, &socket));
        assert!(!same_socket(&dir.path().join("none.sock"), &socket));
    }
}
