//! Commands held on this side while a batch is made, so the engine hears
//! all of a batch that was logged and none of one that was refused.

use super::{Command, EngineControl};

/// Commands held while a batch is made, and the room as it was before it.
#[derive(Debug)]
pub(super) struct CommandBatch {
    /// The batch's commands, in order.
    pub commands: Vec<Command>,
    slots: usize,
    knobs: usize,
    costs: usize,
}

impl EngineControl {
    #[cfg(test)]
    pub(crate) fn queued_commands(&self) -> usize {
        use ringbuf::traits::Observer;
        self.commands.occupied_len() + self.backlog.len()
    }

    /// Hold every command from now until [`Self::finish_batch`] or
    /// [`Self::abort_batch`].
    ///
    /// # Errors
    ///
    /// When a batch is held already.
    pub(crate) fn begin_batch(&mut self) -> Result<(), &'static str> {
        if self.batch.is_some() {
            return Err("engine command batch is already open");
        }
        self.batch = Some(CommandBatch {
            commands: Vec::new(),
            slots: self.slots,
            knobs: self.knobs.len(),
            costs: self.costs.len(),
        });
        Ok(())
    }

    /// Send the held commands, in order.
    pub(crate) fn finish_batch(&mut self) {
        if let Some(batch) = self.batch.take() {
            for command in batch.commands {
                self.push(command);
            }
        }
    }

    /// Drop the held commands, and the room reserved for them: the engine
    /// hears none of them.
    pub(crate) fn abort_batch(&mut self) {
        if let Some(batch) = self.batch.take() {
            self.slots = batch.slots;
            self.knobs.truncate(batch.knobs);
            self.costs.truncate(batch.costs);
        }
    }
}
