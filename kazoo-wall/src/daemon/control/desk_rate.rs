//! The wall follows the desk's rate (KZ-D21).
//!
//! The desk takes only instruments at its own rate. The wall opens its
//! device at whatever rate the device is set to, and keeps it for as long
//! as it plays. So a wall started on a 48 kHz device never plugged into a
//! desk started later on a 44.1 kHz one (Tom, 16 Oct 2026: "the mixer and
//! wall aren't connecting"). Now, while the desk the wall joins refuses it
//! for running at another rate, the wall opens the audio again: on the
//! default device as it is set (never switching its rate, which is the
//! owner's), and when that still differs, on the headless loop at the
//! desk's rate, which the desk plays. When the desk has gone (two looks in
//! a row, a retry apart), the wall goes back to its device. The music
//! carries on from the bar it was at.
//!
//! The desk's rate is read from its refusal, which the desk link already
//! holds: nothing is read from disk and no socket is knocked on. A look is
//! made at most once per [`DEVICE_RETRY`], and none while plugged in.
//! While a loop records, what it recorded is kept first: each recording
//! loop is asked for a take, and once they are logged (at most [`HOLD_FOR`]
//! later; what a late take misses is said, as for any audio restart) the
//! desk is looked at again and followed as it is then. The seats are told
//! why the wall waits.

use std::time::{Duration, Instant};

use kazoo_core::ipc::link::HubLink;

use super::super::DeskMode;
use super::super::audio::{AudioMode, Device};
use super::{Control, DEVICE_RETRY, play_on, start_audio, start_headless};

/// Looks in a row that find no desk before the wall, following one, goes
/// back to its device: one look can land while the desk is starting again.
const GONE_AFTER: u8 = 2;

/// The longest a follow waits for recording loops' takes (as long as a
/// named save waits for them).
const HOLD_FOR: Duration = crate::daemon::loops::SAVE_WAIT;

/// What a look at the desk link finds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Look {
    /// The desk is up: plugged in, or refused for something other than
    /// its rate.
    Up,
    /// The desk refused the wall for running at another rate: this one.
    Refused(u32),
    /// No desk: the link cannot reach one, or there is no link.
    Gone,
}

/// What the desk `link` says now.
fn look(link: Option<&HubLink>) -> Look {
    let Some(link) = link else {
        return Look::Gone;
    };
    if link.is_connected() {
        return Look::Up;
    }
    let status = link.status();
    match (status.refused_for_rate(), status.desk_refusal()) {
        (Some(rate), _) => Look::Refused(rate),
        (None, Some(_)) => Look::Up,
        (None, None) => Look::Gone,
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

/// What to do, given what a look found, the wall's rate (`wall`), the rate
/// the wall is playing at for a desk on the headless loop (`following`),
/// and how many looks in a row found no desk (`missed`).
const fn follow(look: Look, wall: u32, following: Option<u32>, missed: u8) -> Follow {
    match look {
        Look::Refused(desk) if desk != wall => Follow::Match(desk),
        Look::Gone if following.is_some() && missed >= GONE_AFTER => Follow::Device,
        Look::Refused(_) | Look::Up | Look::Gone => Follow::Stay,
    }
}

/// Following the rate of a desk the wall joins.
#[derive(Debug, Default)]
pub(super) struct Watch {
    /// The desk's rate the wall renders at on the headless loop, when its
    /// device plays at another.
    following: Option<u32>,
    /// When the desk is next looked for.
    due: Option<Instant>,
    /// Looks in a row that found no desk.
    missed: u8,
    /// A follow waiting for recording loops' takes.
    held: Option<Held>,
}

/// A follow waiting for recording loops' takes to be logged.
#[derive(Debug)]
struct Held {
    /// When the takes were asked for.
    since: Instant,
    /// The takes asked for.
    tickets: Vec<u64>,
}

/// What a follow does this round.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    /// Nothing.
    Nothing,
    /// Loops are recording: ask for their takes and hold, then look again
    /// (this is what the look said).
    Keep(Follow),
    /// Still waiting for the takes.
    Wait,
    /// Do this; with a line to tell the seats after a hold: that the audio
    /// stays as it is after all, or that takes did not land in time.
    Act(Follow, Option<String>),
}

impl Watch {
    /// The desk's rate the wall renders at on the headless loop, if it
    /// does.
    pub(super) const fn following(&self) -> Option<u32> {
        self.following
    }

    /// One round at `now` for a wall at `wall` Hz, plugged into the desk or
    /// not: `look` looks at the desk when a look is due. What to do.
    fn round(
        &mut self,
        now: Instant,
        wall: u32,
        plugged_in: bool,
        look: impl FnOnce() -> Look,
    ) -> Follow {
        if plugged_in {
            // Plugged in, the desk is up and at the wall's rate.
            self.missed = 0;
            return Follow::Stay;
        }
        if self.due.is_some_and(|due| now < due) {
            return Follow::Stay;
        }
        self.due = Some(now + DEVICE_RETRY);
        let found = look();
        self.missed = match found {
            Look::Gone => self.missed.saturating_add(1),
            Look::Up | Look::Refused(_) => 0,
        };
        follow(found, wall, self.following, self.missed)
    }

    /// What to do at `now` for a wall at `wall` Hz, plugged into the desk
    /// or not: `look` looks at the desk when a look is due, `recording`
    /// says whether any loop is recording, `settled` whether the takes
    /// held for are all logged. When a hold ends the desk is looked at
    /// again at once, and what that says is done without holding again
    /// (the takes were just kept): a desk that came back or went in the
    /// meantime is followed as it is now.
    fn decide(
        &mut self,
        now: Instant,
        wall: u32,
        plugged_in: bool,
        look: impl FnOnce() -> Look,
        recording: impl FnOnce() -> bool,
        settled: impl FnOnce(&[u64]) -> bool,
    ) -> Step {
        if let Some(held) = self.held.take() {
            let settled = settled(&held.tickets);
            if !settled && now < held.since + HOLD_FOR {
                self.held = Some(held);
                return Step::Wait;
            }
            self.due = None;
            let next = self.round(now, wall, plugged_in, look);
            // The seats were told the audio opens again: say what happens.
            let said = if next == Follow::Stay {
                Some(
                    "the desk changed while the takes were kept; the wall's audio stays as it is"
                        .to_string(),
                )
            } else if settled {
                None
            } else {
                Some(format!(
                    "the loops' takes were not all kept within {} s; what they missed since their last take is said as the audio opens again",
                    HOLD_FOR.as_secs()
                ))
            };
            return Step::Act(next, said);
        }
        let next = self.round(now, wall, plugged_in, look);
        if next == Follow::Stay {
            return Step::Nothing;
        }
        if recording() {
            return Step::Keep(next);
        }
        Step::Act(next, None)
    }

    /// Hold from `now` until `tickets`, the takes asked for, are logged.
    fn hold(&mut self, now: Instant, tickets: Vec<u64>) {
        self.held = Some(Held {
            since: now,
            tickets,
        });
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
        let rate = self.wall.engine_mut().sample_rate();
        let wall = &self.wall;
        let link = wall.desk_link();
        let plugged_in = link.is_some_and(HubLink::is_connected);
        let step = self.desk_rate.decide(
            now,
            rate,
            plugged_in,
            || look(link),
            || !wall.loops_recording().is_empty(),
            |tickets| wall.takes_settled(tickets),
        );
        self.take_step(step, now, || Device::open(None));
    }

    /// Take `step`; `open` opens the device as it is set.
    fn take_step(
        &mut self,
        step: Step,
        now: Instant,
        open: impl FnOnce() -> Result<Device, String>,
    ) {
        match step {
            Step::Nothing | Step::Wait => {}
            Step::Keep(next) => {
                let recording = self.wall.loops_recording();
                let tickets = self.wall.keep_recording_loops();
                let why = match next {
                    Follow::Match(rate) => format!("the desk runs at {rate} Hz"),
                    Follow::Device | Follow::Stay => "the desk has gone".to_string(),
                };
                self.wall.tell_seats(format!(
                    "{why}; {} {} recording, so {} kept before the wall opens its audio again",
                    recording.join(", "),
                    if recording.len() == 1 { "is" } else { "are" },
                    if recording.len() == 1 {
                        "its take is"
                    } else {
                        "their takes are"
                    },
                ));
                self.desk_rate.hold(now, tickets);
            }
            Step::Act(next, late) => {
                if let Some(line) = late {
                    self.wall.tell_seats(line);
                }
                self.follow_now(next, now, open);
            }
        }
    }

    /// Do `next` now; `open` opens the device as it is set.
    fn follow_now(
        &mut self,
        next: Follow,
        now: Instant,
        open: impl FnOnce() -> Result<Device, String>,
    ) {
        match next {
            Follow::Stay => {}
            Follow::Match(rate) => {
                let wall = self.wall.engine_mut().sample_rate();
                eprintln!(
                    "kazoo-wall: the desk runs at {rate} Hz and the wall at {wall} Hz; opening the audio again to match"
                );
                self.match_desk(rate, now, open);
            }
            Follow::Device => {
                eprintln!("kazoo-wall: the desk has gone; back to the audio device");
                self.desk_rate.following = None;
                self.desk_rate.missed = 0;
                self.replace_audio(now, |control, at| {
                    start_audio(
                        control.audio,
                        &control.desk,
                        at,
                        control.heard,
                        &control.listen_tx,
                        None,
                    )
                });
            }
        }
    }

    /// Open the audio again for a desk at `rate`: on the device as it is
    /// set (opened by `open`), when that is `rate`; otherwise on the
    /// headless loop at `rate`. The device is opened first and its rate
    /// read, so the engine and its desk link are made once, at the rate
    /// they keep.
    fn match_desk(
        &mut self,
        rate: u32,
        now: Instant,
        open: impl FnOnce() -> Result<Device, String>,
    ) {
        self.replace_audio(now, |control, at| {
            match open() {
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
mod tests;
