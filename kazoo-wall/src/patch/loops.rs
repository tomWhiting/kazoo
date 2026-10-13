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
    /// Keep a loop's record. Callers validate the module and record first.
    pub fn set_loop(&mut self, id: &str, record: LoopRecord) {
        self.loops.insert(id.into(), record);
    }
    /// Every active loop's saved record.
    #[must_use]
    pub const fn loops(&self) -> &std::collections::BTreeMap<String, LoopRecord> {
        &self.loops
    }
    pub(super) fn loop_room(&self) -> Result<(), WallError> {
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
        match op {
            LoopOp::Erase => {
                let record = self.loops.entry(module.into()).or_default();
                Ok(What::Erase {
                    module: module.into(),
                    was: record.tape.take(),
                })
            }
            LoopOp::From { from } => {
                if from.as_deref().is_some_and(|name| !valid_from(name)) {
                    return Err(error(
                        ErrorCode::BadName,
                        "a desk strip label is 1 to 24 bytes of letters, digits, /, _, . or -",
                    ));
                }
                let record = self.loops.entry(module.into()).or_default();
                let was = std::mem::replace(&mut record.from, from.clone());
                Ok(What::LoopFrom {
                    module: module.into(),
                    from: from.clone(),
                    was,
                })
            }
        }
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
                self.loops.insert(id.clone(), record.clone());
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
