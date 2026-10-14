use super::{Command, EngineControl};

#[derive(Debug)]
pub(super) struct CommandBatch {
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

    pub(crate) fn finish_batch(&mut self) {
        if let Some(batch) = self.batch.take() {
            for command in batch.commands {
                self.push(command);
            }
        }
    }

    pub(crate) fn abort_batch(&mut self) {
        if let Some(batch) = self.batch.take() {
            self.slots = batch.slots;
            self.knobs.truncate(batch.knobs);
            self.costs.truncate(batch.costs);
        }
    }
}
