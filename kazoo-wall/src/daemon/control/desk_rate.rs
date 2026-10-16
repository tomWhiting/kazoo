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
//! which the desk plays. When the desk goes, the wall goes back to its
//! device. The music carries on from the bar it was at.

use std::time::Instant;

use kazoo_core::ipc::discovery;
use kazoo_core::ipc::link::HubLink;

use super::super::DeskMode;
use super::super::audio::AudioMode;
use super::{Control, start_audio, start_headless};

/// The rate a desk the wall joins runs at, when one is up and says.
pub(super) fn desk_rate(desk: &DeskMode) -> Option<u32> {
    let record = match discovery::read_pid_file() {
        Ok(Some(record)) => record,
        Ok(None) => return None,
        Err(err) => {
            eprintln!("kazoo-wall: the desk's record does not read: {err}");
            return None;
        }
    };
    let ours = match desk {
        DeskMode::Off => false,
        DeskMode::Discover => true,
        DeskMode::Socket(path) => record.socket == *path,
    };
    if !ours {
        return None;
    }
    match discovery::hub_listening(&record.socket) {
        Ok(true) => record.sample_rate,
        Ok(false) => None,
        Err(err) => {
            eprintln!("kazoo-wall: the desk's socket cannot be tried: {err}");
            None
        }
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

/// What to do, given the desk's rate (`desk`), the wall's (`wall`), whether
/// the wall is plugged into the desk, the rate the wall is playing at for a
/// desk on the headless loop (`following`), and the desk and wall rates it
/// last opened the audio again for (`tried`, so a device that cannot match
/// is not opened again and again).
pub(super) fn follow(
    desk: Option<u32>,
    wall: u32,
    plugged_in: bool,
    following: Option<u32>,
    tried: Option<(u32, u32)>,
) -> Follow {
    match desk {
        Some(desk) if desk != wall && !plugged_in && tried != Some((desk, wall)) => {
            Follow::Match(desk)
        }
        None if following.is_some() => Follow::Device,
        Some(_) | None => Follow::Stay,
    }
}

impl Control {
    /// Follow the desk's rate when it differs from the wall's (see the
    /// module's words). Only a wall on the device at the rate the device is
    /// set to follows: a rate or the headless loop asked for is kept.
    pub(super) fn follow_desk_rate(&mut self, now: Instant) {
        if self.audio != (AudioMode::Device { sample_rate: None }) {
            return;
        }
        let desk = desk_rate(&self.desk);
        let wall = self.wall.engine_mut().sample_rate();
        let plugged_in = self.wall.desk_link().is_some_and(HubLink::is_connected);
        match follow(
            desk,
            wall,
            plugged_in,
            self.following_desk,
            self.desk_rate_tried,
        ) {
            Follow::Stay => {}
            Follow::Match(rate) => {
                self.desk_rate_tried = Some((rate, wall));
                eprintln!(
                    "kazoo-wall: the desk runs at {rate} Hz and the wall at {wall} Hz; opening the audio again to match"
                );
                self.match_desk(rate, now);
            }
            Follow::Device => {
                self.following_desk = None;
                self.desk_rate_tried = None;
                eprintln!("kazoo-wall: the desk has gone; back to the audio device");
                self.rebuild(now);
            }
        }
    }

    /// Open the audio again for a desk at `rate`: the device as it is set,
    /// or, when that is at another rate, the headless loop at `rate`.
    fn match_desk(&mut self, rate: u32, now: Instant) {
        self.replace_audio(now, |control, at| {
            let (backend, engine, link) = start_audio(
                control.audio,
                &control.desk,
                at,
                control.heard,
                &control.listen_tx,
                None,
            );
            if engine.sample_rate() == rate {
                control.following_desk = None;
                return (backend, engine, link);
            }
            eprintln!(
                "kazoo-wall: the device plays at {} Hz; the wall renders at {rate} Hz for the desk to play",
                engine.sample_rate()
            );
            drop((backend, engine, link));
            control.following_desk = Some(rate);
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

    #[test]
    fn a_desk_at_another_rate_is_matched_once_and_a_gone_desk_gives_the_device_back() {
        // The wall at 48 kHz, unplugged, a desk at 44.1 kHz: match it.
        assert_eq!(
            follow(Some(44_100), 48_000, false, None, None),
            Follow::Match(44_100)
        );
        // Tried already for these two rates (the device could not): stay.
        assert_eq!(
            follow(Some(44_100), 48_000, false, None, Some((44_100, 48_000))),
            Follow::Stay
        );
        // The same rate, or plugged in: nothing to do.
        assert_eq!(
            follow(Some(48_000), 48_000, false, None, None),
            Follow::Stay
        );
        assert_eq!(follow(Some(44_100), 48_000, true, None, None), Follow::Stay);
        // Following a desk on the headless loop at its rate: stay while it
        // is up, back to the device when it goes.
        assert_eq!(
            follow(
                Some(44_100),
                44_100,
                true,
                Some(44_100),
                Some((44_100, 48_000))
            ),
            Follow::Stay
        );
        assert_eq!(
            follow(None, 44_100, false, Some(44_100), None),
            Follow::Device
        );
        // No desk and not following one: nothing.
        assert_eq!(follow(None, 48_000, false, None, None), Follow::Stay);
        // A desk that comes back at yet another rate is matched again.
        assert_eq!(
            follow(Some(96_000), 44_100, false, None, Some((44_100, 48_000))),
            Follow::Match(96_000)
        );
    }
}
