//! A batch's steps as moments of their own (KZ-T10c).
//!
//! A batch stays one change in the log: one seq, one undo, one notice to
//! the seats and one block of sound. The tree grows a moment for each of
//! its steps, so a batch of fifty-five changes grows the tree as fifty-five
//! changes would (Tom, 16 Oct 2026: "each one forms individual moments from
//! the individual changes inside"). The wall after each step but the last
//! is made here, from the wall before the batch, by the model `kazoo wall
//! tree import` replays the log with, so a live tree and an imported one
//! hold the same walls. The last step's moment is the batch's own commit,
//! holding the wall as the wall has it.

use super::model::Model;
use super::{Files, TreeError};
use crate::patch::PatchFile;
use crate::protocol::{Change, What};

/// The steps of `change` when it is a batch made (not an undo of one) of
/// more than one step; otherwise `None`, and the change is one moment.
#[must_use]
pub fn steps(change: &Change) -> Option<&[What]> {
    match (&change.what, change.undoes) {
        (What::Batch { changes, .. }, None) if changes.len() > 1 => Some(changes),
        _ => None,
    }
}

/// The tree's files after each step of batch `change` but its last, from
/// `before`, the wall as it was just before the batch.
///
/// # Errors
///
/// `TreeError::Git` when `change` is not a batch of more than one step, or
/// a step does not apply to the wall before it as the step says (a
/// before-value disagrees, or what it names is not there).
pub fn batch_parts(before: &PatchFile, change: &Change) -> Result<Vec<Files>, TreeError> {
    let steps = steps(change)
        .ok_or_else(|| TreeError::Git(format!("change {} is not a batch of steps", change.seq)))?;
    let mut model = Model::new(before.clone(), true)?;
    let mut parts = Vec::with_capacity(steps.len() - 1);
    for (index, step) in steps[..steps.len() - 1].iter().enumerate() {
        // The step as a change of its own, by the batch's seat, at its
        // time: never a copy of the whole batch for each step.
        let part = Change {
            seq: change.seq,
            at: change.at.clone(),
            seat: change.seat.clone(),
            what: step.clone(),
            summary: crate::change::describe(step),
            undoes: None,
            clock: change.clock.clone(),
            comment: None,
        };
        let applied = model.apply(&part);
        if let Some(why) = applied.unknown.or(applied.doubt) {
            return Err(TreeError::Git(format!(
                "step {} of change {} does not apply to the wall before it: {why}",
                index + 1,
                change.seq
            )));
        }
        model.stamp(&part, 0)?;
        parts.push(model.files(Some(&part))?);
    }
    Ok(parts)
}
