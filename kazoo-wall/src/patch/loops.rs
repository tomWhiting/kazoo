//! Saved tape geometry and the undoable choices for each loop.
use super::{Patch, error, printable, valid_id};
use crate::catalogue::Kind;
use crate::daemon::loops::MAX_LOOPS;
use crate::dsp::looper::{MAX_LOOP_BARS, MAX_LOOP_FRAMES};
use crate::protocol::{ErrorCode, LoopOp, WallError, What, valid_from};
use serde::{Deserialize, Serialize};

/// Removed loops kept for undo, oldest removal first.
pub const MAX_RETIRED_LOOPS: usize = 64;
/// The tape and desk source kept with a loop module.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LoopRecord {
    /// Absent for blank tape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tape: Option<TapeRecord>,
    /// A desk strip's label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}
/// Sample identity and beat geometry of a stored stereo reel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TapeRecord {
    /// Content hash of the reel.
    pub reel: String,
    /// Recorded frames per second.
    pub rate: u32,
    /// Frames before the seam tail.
    pub frames: u64,
    /// Continuation frames at the seam.
    pub tail: u64,
    /// Loop duration in song beats.
    pub beats: f64,
    /// Song beat of the first frame.
    pub anchor: f64,
    /// Last recorded pass for each bar.
    pub bars: Vec<Option<i64>>,
}
/// A removed module's tape and source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetiredLoop {
    /// Removed module id.
    pub module: String,
    /// Its saved tape and source.
    pub record: LoopRecord,
}
impl TapeRecord {
    /// Validate geometry before any worker allocation or installation.
    ///
    /// # Errors
    /// Returns the unreadable part of a saved record.
    pub fn check(&self) -> Result<(), String> {
        self.reel
            .parse::<kazoo_core::reels::ReelId>()
            .map_err(|error| error.to_string())?;
        if self.frames == 0 || self.frames > MAX_LOOP_FRAMES as u64 {
            return Err("frames outside tape capacity".into());
        }
        if self.tail > 4096 {
            return Err("tail exceeds 4096 frames".into());
        }
        if !self.beats.is_finite() || !(1.0..=260.0).contains(&self.beats) {
            return Err("beats outside 1 to 260".into());
        }
        if !self.anchor.is_finite() {
            return Err("anchor is not finite".into());
        }
        if self.bars.len() != (self.beats / 4.0).ceil() as usize || self.bars.len() > MAX_LOOP_BARS
        {
            return Err("bar count disagrees with beats".into());
        }
        if !(8_000..=384_000).contains(&self.rate) {
            return Err("rate outside 8000 to 384000".into());
        }
        Ok(())
    }
}
impl LoopRecord {
    fn check(&self) -> Result<(), String> {
        if self.from.as_deref().is_some_and(|from| !valid_from(from)) {
            return Err("invalid desk strip label".into());
        }
        if let Some(tape) = &self.tape {
            tape.check()?;
        }
        Ok(())
    }
}
impl Patch {
    /// The saved tape and source of one loop.
    #[must_use]
    pub fn loop_record(&self, id: &str) -> Option<&LoopRecord> {
        self.loops.get(id)
    }
    /// Keep `record` as loop `id`'s tape and source.
    ///
    /// # Errors
    /// `id` is not a loop on the wall, or the record does not read as the
    /// file would have to hold it (see [`TapeRecord::check`]).
    pub fn set_loop(&mut self, id: &str, record: LoopRecord) -> Result<(), WallError> {
        if self.find(id)?.kind != Kind::LOOP {
            return Err(error(ErrorCode::NotAllowed, format!("{id} is not a loop")));
        }
        record.check().map_err(|why| {
            error(
                ErrorCode::BadRequest,
                format!("{id}'s tape does not read: {why}"),
            )
        })?;
        self.keep_loop(id, record);
        Ok(())
    }
    /// Keep `record` for `id`, holding no record for a blank loop that
    /// records what is patched into it, so the file names only loops with
    /// something to keep.
    fn keep_loop(&mut self, id: &str, record: LoopRecord) {
        if record == LoopRecord::default() {
            self.loops.remove(id);
        } else {
            self.loops.insert(id.to_string(), record);
        }
    }
    /// Every active loop's saved record.
    #[must_use]
    pub const fn loops(&self) -> &std::collections::BTreeMap<String, LoopRecord> {
        &self.loops
    }
    /// Whether the wall has room for another loop.
    ///
    /// # Errors
    /// `full` when it holds [`MAX_LOOPS`] already.
    pub fn loop_room(&self) -> Result<(), WallError> {
        if self
            .modules
            .iter()
            .filter(|module| module.kind == Kind::LOOP)
            .count()
            >= MAX_LOOPS
        {
            Err(error(
                ErrorCode::Full,
                "a wall holds eight loops, like the eight-track; remove one, or erase one and use it again",
            ))
        } else {
            Ok(())
        }
    }
    /// Change tape or source, preserving the prior value for undo.
    ///
    /// # Errors
    /// Refuses unknown modules, other module kinds and invalid strip labels.
    pub fn loop_op(&mut self, module: &str, op: &LoopOp) -> Result<What, WallError> {
        if self.find(module)?.kind != Kind::LOOP {
            return Err(error(
                ErrorCode::NotAllowed,
                format!("{module} is not a loop"),
            ));
        }
        let mut record = self.loops.get(module).cloned().unwrap_or_default();
        match op {
            LoopOp::Erase => {
                let was = record.tape.take();
                self.keep_loop(module, record);
                Ok(What::Erase {
                    module: module.into(),
                    was,
                    tape: None,
                })
            }
            LoopOp::From { from } => {
                if from.as_deref().is_some_and(|name| !valid_from(name)) {
                    return Err(error(
                        ErrorCode::BadName,
                        "a desk strip label is 1 to 24 bytes of letters, digits, /, _, . or -",
                    ));
                }
                let was = std::mem::replace(&mut record.from, from.clone());
                self.keep_loop(module, record);
                Ok(What::LoopFrom {
                    module: module.into(),
                    from: from.clone(),
                    was,
                })
            }
        }
    }
    /// Undo an erase, or the undo of one: the loop holds `was` again.
    pub(super) fn undo_erase(
        &mut self,
        module: &str,
        was: Option<&TapeRecord>,
    ) -> Result<What, WallError> {
        if self.find(module)?.kind != Kind::LOOP {
            return Err(error(
                ErrorCode::NotAllowed,
                format!("{module} is not a loop"),
            ));
        }
        let mut record = self.loops.get(module).cloned().unwrap_or_default();
        let prior = std::mem::replace(&mut record.tape, was.cloned());
        self.keep_loop(module, record);
        Ok(What::Erase {
            module: module.to_string(),
            was: prior,
            tape: was.cloned(),
        })
    }
    pub(super) fn load_loops(&mut self, file: &super::PatchFile, notes: &mut Vec<String>) {
        for (id, record) in &file.loops {
            if self
                .module(id)
                .is_none_or(|module| module.kind != Kind::LOOP)
            {
                notes.push(format!(
                    "loop record for '{}', which is not a loop module, dropped",
                    printable(id)
                ));
            } else if let Err(error) = record.check() {
                notes.push(format!(
                    "{}'s tape was unreadable in the file ({error}); it is blank",
                    printable(id)
                ));
            } else {
                self.keep_loop(id, record.clone());
            }
        }
        for retired in &file.retired_loops {
            if !valid_id(&retired.module) || self.module(&retired.module).is_some() {
                notes.push(format!(
                    "removed loop record for '{}' dropped",
                    printable(&retired.module)
                ));
            } else if let Err(error) = retired.record.check() {
                notes.push(format!(
                    "{}'s tape was unreadable in the file ({error}); it is blank",
                    printable(&retired.module)
                ));
            } else {
                self.retired_loops.push_back(retired.clone());
            }
        }
        while self.retired_loops.len() > MAX_RETIRED_LOOPS {
            self.retired_loops.pop_front();
        }
    }
}
