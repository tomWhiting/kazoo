use super::{
    COMMAND_BACKLOG, Command, Engine, EngineControl, Ordering, Retired, SUB_BLOCK,
    SWAP_FADE_SECONDS,
};
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LandingKind {
    Cut,
    Cross,
}

/// Beats in a bar, as the console counts them.
const BEATS_PER_BAR: f64 = 4.0;

/// How long before its bar a go must be asked for to land on it: the swap
/// fade, so the wall going out fades out whole, and a tenth of a second
/// for the go to cross to the audio thread (a device buffer is a few
/// hundredths). A go asked later lands on the bar after.
const LANDING_LEAD_SECONDS: f64 = SWAP_FADE_SECONDS as f64 + 0.1;

/// One complete wall, prepared before it enters the render command ring.
#[derive(Debug)]
pub struct TimedSwap {
    kind: LandingKind,
    prior: Option<Box<Self>>,
    edits: VecDeque<Command>,
    beat: f64,
    serial: u64,
    /// Whether the wall going out has begun to fade towards `beat`.
    fading: bool,
    commands: VecDeque<Command>,
}

impl EngineControl {
    #[cfg(test)]
    pub(crate) const fn retired_modules(&self) -> usize {
        self.retired_modules
    }

    pub(crate) const fn landing_beat(&self) -> f64 {
        self.swap_beat
    }

    /// Collect a replacement wall for the next absolute four-beat boundary.
    pub fn begin_swap(&mut self) {
        self.swap_commands = Some(VecDeque::new());
    }

    /// Queue the collected wall and return its absolute landing beat: the
    /// first bar at least [`LANDING_LEAD_SECONDS`] ahead, or the bar a go
    /// not yet landed waits for (it replaces that one).
    pub fn end_swap(&mut self) -> f64 {
        self.end_landing(LandingKind::Cut)
    }

    /// Hold a prepared crossing for the same bar mechanism as a cut.
    pub(crate) fn end_cross(&mut self) -> f64 {
        self.end_landing(LandingKind::Cross)
    }

    pub(crate) fn next_landing_serial(&self) -> u64 {
        self.swap_serial
            .max(self.shared.unhushed())
            .saturating_add(1)
    }

    pub(crate) fn landing_pending(&self) -> bool {
        self.shared.unhushed() < self.swap_serial
    }

    fn end_landing(&mut self, kind: LandingKind) -> f64 {
        let beat = if self.shared.unhushed() < self.swap_serial {
            self.swap_beat
        } else {
            let lead =
                kazoo_core::ipc::follow::beats_in(LANDING_LEAD_SECONDS, self.shared.bpm(), 1.0);
            bar_from(self.shared.beat() + lead)
        };
        self.swap_beat = beat;
        if let Some(commands) = self.swap_commands.take() {
            self.swap_serial = self
                .swap_serial
                .max(self.shared.unhushed())
                .saturating_add(1);
            self.push(Command::Swap(Box::new(TimedSwap {
                kind,
                prior: None,
                edits: VecDeque::with_capacity(COMMAND_BACKLOG),
                beat,
                serial: self.swap_serial,
                fading: false,
                commands,
            })));
        }
        beat
    }
}

/// The first bar at or after `beat`.
fn bar_from(beat: f64) -> f64 {
    (beat / BEATS_PER_BAR).ceil() * BEATS_PER_BAR
}

impl Engine {
    pub(super) fn queue_swap(&mut self, mut swap: Box<TimedSwap>) {
        if let Some(mut old) = self.pending_swap.take() {
            // A go replacing one that is fading out towards the same bar
            // carries the fade on, rather than starting it again.
            swap.fading = old.fading && old.beat.to_bits() == swap.beat.to_bits();
            // Probe freshness counts sent tables, including superseded ones.
            if swap.kind == LandingKind::Cross {
                // The control rack already includes these placements. Keep
                // their prepared commands, and intervening edits, in order.
                std::mem::swap(&mut old.edits, &mut self.after_swap);
                swap.prior = Some(old);
            } else {
                self.tables = self.tables.wrapping_add(
                    old.commands
                        .iter()
                        .filter(|command| matches!(command, Command::Cables(_)))
                        .count() as u64,
                );
                self.retire(Retired::Swap(old));
            }
        }
        self.pending_swap = Some(swap);
        while let Some(command) = self.after_swap.pop_front() {
            if matches!(command, Command::Cables(_)) {
                self.tables = self.tables.wrapping_add(1);
            }
            self.retire(Retired::Command(command));
        }
        // A go that arrives too late to fade the wall out whole for its bar
        // waits for the next.
        let step = super::Tick::new(self.sample_rate, self.bpm, self.beat).beat_step;
        self.aim_swap(step, true);
    }

    /// Move a held go whose bar the song position no longer leads to: one
    /// the song has gone past (the go reached the audio thread late, or the
    /// desk moved the song on), or one more than a bar ahead (the desk moved
    /// the song back); and, when the go has just arrived or the desk has
    /// just moved the song (`arrived`), one the wall going out can no
    /// longer fade out whole for. It goes to the first bar the wall can
    /// fade out whole for (a frame short is whole: the fade may begin on
    /// the first frame of a sub-block). Between those, the song only moves
    /// on towards the bar, and the fade begins on its own frame.
    fn aim_swap(&mut self, step: f64, arrived: bool) {
        let fade = step * f64::from(self.sample_rate * SWAP_FADE_SECONDS);
        let beat = self.beat;
        if let Some(swap) = self.pending_swap.as_mut() {
            let aim = bar_from(beat + fade);
            let passed = swap.beat < beat - step;
            let short = arrived && !swap.fading && swap.beat - beat < fade - step;
            let back = swap.beat > aim + BEATS_PER_BAR;
            if passed || short || back {
                swap.beat = aim;
                swap.fading = false;
            }
        }
    }

    /// Take in the held go when the song reaches its bar; `desk_moved` when
    /// the desk set the song position or tempo for this sub-block.
    pub(super) fn land_swap(&mut self, desk_moved: bool) {
        let step = super::Tick::new(self.sample_rate, self.bpm, self.beat).beat_step;
        self.aim_swap(step, desk_moved);
        if self
            .pending_swap
            .as_ref()
            .is_none_or(|swap| step.mul_add(1e-6, self.beat) < swap.beat)
        {
            return;
        }
        if let Some(mut swap) = self.pending_swap.take() {
            // The bar itself, not the frame's beat (within a millionth of a
            // frame of it): the origin is a whole bar of the song.
            self.origin = swap.beat;
            self.shared
                .origin
                .store(self.origin.to_bits(), Ordering::Relaxed);
            self.wall_beat = Some(0.0);
            let serial = swap.serial;
            let cross = swap.kind == LandingKind::Cross;
            if !cross {
                self.master.reset();
                self.hush_gain = 0.0;
            }
            // Reverse the held chain without allocating, then take each
            // prepared wall and its edits in their original order.
            let mut ordered = None;
            loop {
                let prior = swap.prior.take();
                swap.prior = ordered;
                ordered = Some(swap);
                if let Some(previous) = prior {
                    swap = previous;
                } else {
                    break;
                }
            }
            while let Some(mut held) = ordered.take() {
                ordered = held.prior.take();
                while let Some(command) = held.commands.pop_front() {
                    if !cross || !matches!(command, Command::Hush | Command::Unhush) {
                        self.apply_command(command);
                    }
                }
                while let Some(command) = held.edits.pop_front() {
                    self.apply_command(command);
                }
                self.retire(Retired::Swap(held));
            }
            self.shared
                .landing_frame
                .store(self.rendered, Ordering::Relaxed);
            self.shared.unhushed.store(serial, Ordering::Release);
            while let Some(command) = self.after_swap.pop_front() {
                self.apply_command(command);
            }
        }
    }

    pub(super) fn frames_to_swap(&self, beat_step: f64) -> usize {
        self.pending_swap.as_ref().map_or(SUB_BLOCK, |swap| {
            ((swap.beat - self.beat) / beat_step - 1e-6)
                .ceil()
                .clamp(1.0, SUB_BLOCK as f64) as usize
        })
    }

    pub(super) fn fade_to_swap(
        &mut self,
        left: &mut super::Block,
        right: &mut super::Block,
        beat_step: f64,
    ) {
        let beat = self.beat;
        let fade = f64::from(self.sample_rate * SWAP_FADE_SECONDS);
        if let Some(swap) = self.pending_swap.as_mut() {
            if swap.kind == LandingKind::Cross {
                return;
            }
            let remaining = (swap.beat - beat) / beat_step;
            for (frame, (left, right)) in left.iter_mut().zip(right).enumerate() {
                let gain = ((remaining - frame as f64) / fade).clamp(0.0, 1.0);
                swap.fading |= gain < 1.0;
                // In 0..1: single precision is plenty.
                let gain = gain as f32;
                *left *= gain;
                *right *= gain;
            }
        }
    }
}
