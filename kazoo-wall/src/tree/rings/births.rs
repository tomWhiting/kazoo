use super::Walk;
use super::model::Found;
use crate::patch::Patch;
use crate::protocol::Change;
use crate::tree::TreeError;

impl Walk<'_> {
    pub(super) fn ending(&mut self, last: &Change) -> Result<(), TreeError> {
        let Some(evidence) = self
            .log
            .proofs
            .patch
            .as_ref()
            .filter(|e| e.after == last.seq && self.model.known)
        else {
            return Ok(());
        };
        // The next start finds identities in the saved file, independently
        // of the identities reconstructed while replaying old log lines.
        let mut file = evidence.as_read()?;
        crate::migrate::migrate(&mut file).map_err(TreeError::Git)?;
        let saved = Patch::load(&file).map_err(TreeError::Git)?.0.to_file();
        let mut file = self.model.file.clone();
        for module in &mut file.modules {
            if let Some(original) = saved.modules.iter().find(|m| m.id == module.id) {
                module.birth.clone_from(&original.birth);
            }
        }
        self.model.adopt(&file, last.seq, Found::File)?;
        if self.model.files(None)? != self.previous {
            self.own(
                "settle",
                (last.seq, &last.at),
                ("the module births as patch.json keeps them at the wall's next start",
                 "The next start reads patch.json; modules without a birth in that file receive its found marks."),
                None,
            )?;
        }
        Ok(())
    }
}
