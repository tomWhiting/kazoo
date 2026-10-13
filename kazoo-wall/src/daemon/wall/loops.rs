//! Loop requests use the wall's ordinary durable change and rollback path.
use super::{Outcome, Wall};
use crate::protocol::{LoopOp, WallError};
impl Wall {
    pub(super) fn loop_change(
        &mut self,
        seat: &str,
        module: &str,
        op: &LoopOp,
    ) -> Result<Outcome, WallError> {
        let id = self.patch.find(module)?.id.clone();
        let what = self.patch.loop_op(&id, op)?;
        self.changed(seat, what, None, Some(id), None)
    }
}
