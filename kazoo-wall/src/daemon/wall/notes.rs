use super::{Wall, abandoned_changes, error};
use crate::protocol::{Change, ErrorCode, WallError, What};

impl Wall {
    pub(super) fn notes(&self, seq: u64) -> Result<crate::protocol::NotesResult, WallError> {
        use crate::protocol::{Attachment, NotesResult};
        let change = self.note_change(seq)?;
        if change.notes.is_empty() {
            return Err(error(
                ErrorCode::NotFound,
                format!("change {seq} exists but has no notes"),
            ));
        }
        let mut result = NotesResult {
            seq,
            note: match &change.what {
                What::Batch { note, .. } => note.clone(),
                _ => change.summary.clone(),
            },
            description: None,
            attachments: Vec::new(),
            missing: Vec::new(),
        };
        for note in &change.notes {
            let read = self
                .store
                .as_ref()
                .ok_or_else(|| std::io::Error::other("no state store"))
                .and_then(|store| crate::store::notes::read(store.dir(), note));
            match read {
                Ok(text) if note.name == "description.md" => result.description = Some(text),
                Ok(text) => result.attachments.push(Attachment {
                    name: note.name.clone(),
                    text,
                }),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    result.missing.push(note.name.clone());
                }
                Err(err) => {
                    return Err(error(
                        ErrorCode::Internal,
                        format!("read note {}: {err}", note.name),
                    ));
                }
            }
        }
        Ok(result)
    }
}

impl Wall {
    fn note_change(&self, seq: u64) -> Result<Change, WallError> {
        let not_made = || error(ErrorCode::NotFound, format!("change {seq} was not made"));
        if abandoned_changes(self.log.iter()).contains(&seq) {
            return Err(not_made());
        }
        let change =
            if let Some(change) = self.log.iter().find(|change| change.seq == seq) {
                Some(change.clone())
            } else if let Some(store) = self.store.as_ref().filter(|_| {
                seq <= self.made && self.log.front().is_none_or(|first| seq < first.seq)
            }) {
                let history = store.load_log().map_err(|err| {
                    error(ErrorCode::Internal, format!("read notes history: {err}"))
                })?;
                if abandoned_changes(history.changes.iter()).contains(&seq) {
                    return Err(not_made());
                }
                history.changes.into_iter().find(|change| change.seq == seq)
            } else {
                None
            };
        change.ok_or_else(|| {
            error(
                ErrorCode::NotFound,
                format!("change {seq} does not exist in the retained log"),
            )
        })
    }
}
