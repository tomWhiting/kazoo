use super::{Command, Engine, EngineControl, Ordering, Retired, SUB_BLOCK};
use std::collections::VecDeque;

/// One complete wall, prepared before it enters the render command ring.
#[derive(Debug)]
pub struct TimedSwap {
    beat: f64,
    serial: u64,
    commands: VecDeque<Command>,
}

impl EngineControl {
    pub(crate) const fn landing_beat(&self) -> f64 {
        self.swap_beat
    }

    /// Collect a replacement wall for the next absolute four-beat boundary.
    pub fn begin_swap(&mut self) {
        self.swap_commands = Some(VecDeque::new());
    }

    /// Queue the collected wall and return its absolute landing beat.
    pub fn end_swap(&mut self) -> f64 {
        let beat = if self.shared.unhushed() < self.swap_serial {
            self.swap_beat
        } else {
            next_bar(self.shared.beat())
        };
        self.swap_beat = beat;
        if let Some(commands) = self.swap_commands.take() {
            self.swap_serial = self
                .swap_serial
                .max(self.shared.unhushed())
                .saturating_add(1);
            self.push(Command::Swap(Box::new(TimedSwap {
                beat,
                serial: self.swap_serial,
                commands,
            })));
        }
        beat
    }
}

fn next_bar(beat: f64) -> f64 {
    (beat / 4.0).floor().mul_add(4.0, 4.0)
}

impl Engine {
    pub(super) fn queue_swap(&mut self, swap: Box<TimedSwap>) {
        if let Some(old) = self.pending_swap.replace(swap) {
            // Probe freshness counts sent tables, including superseded ones.
            self.tables = self.tables.wrapping_add(
                old.commands
                    .iter()
                    .filter(|command| matches!(command, Command::Cables(_)))
                    .count() as u64,
            );
            self.retire(Retired::Swap(old));
        }
        while let Some(command) = self.after_swap.pop_front() {
            if matches!(command, Command::Cables(_)) {
                self.tables = self.tables.wrapping_add(1);
            }
            self.retire(Retired::Command(command));
        }
    }

    pub(super) fn land_swap(&mut self) {
        let step = super::Tick::new(self.sample_rate, self.bpm, self.beat).beat_step;
        if self
            .pending_swap
            .as_ref()
            .is_none_or(|swap| step.mul_add(1e-6, self.beat) < swap.beat)
        {
            return;
        }
        if let Some(mut swap) = self.pending_swap.take() {
            self.origin = self.beat;
            self.wall_beat = Some(0.0);
            self.master.reset();
            self.hush_gain = 0.0;
            while let Some(command) = swap.commands.pop_front() {
                self.apply_command(command);
            }
            self.shared.unhushed.store(swap.serial, Ordering::Release);
            self.retire(Retired::Swap(swap));
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
        &self,
        left: &mut super::Block,
        right: &mut super::Block,
        beat_step: f64,
    ) {
        if let Some(swap) = &self.pending_swap {
            let remaining = (swap.beat - self.beat) / beat_step;
            let fade = f64::from(self.sample_rate * super::SWAP_FADE_SECONDS);
            for (frame, (left, right)) in left.iter_mut().zip(right).enumerate() {
                let gain = ((remaining - frame as f64) / fade).clamp(0.0, 1.0) as f32;
                *left *= gain;
                *right *= gain;
            }
        }
    }
}
