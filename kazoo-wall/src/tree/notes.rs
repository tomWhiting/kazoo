use std::path::Path;

use crate::protocol::Change;

pub fn append(state: &Path, change: &Change, files: &mut super::thread::FileDiff) {
    for note in &change.notes {
        if !crate::protocol::notes::valid_name(&note.name) {
            eprintln!(
                "kazoo-wall: change {} has invalid note name {:?}",
                change.seq, note.name
            );
            continue;
        }
        match crate::store::notes::read(state, note) {
            Ok(text) => files.push((
                format!("notes/{}/{}", change.seq, note.name),
                Some(text.into_bytes()),
            )),
            Err(err) => eprintln!(
                "kazoo-wall: change {} note {} is unavailable: {err}",
                change.seq, note.name
            ),
        }
    }
}

pub fn valid_path(path: &str) -> bool {
    path.strip_prefix("notes/")
        .and_then(|rest| rest.split_once('/'))
        .is_some_and(|(seq, name)| {
            !seq.is_empty()
                && seq.bytes().all(|c| c.is_ascii_digit())
                && crate::protocol::notes::valid_name(name)
        })
}

pub(super) fn body(
    change: &Change,
    files: &super::thread::FileDiff,
) -> Result<String, super::TreeError> {
    let path = format!("notes/{}/description.md", change.seq);
    let text = files
        .iter()
        .find(|(name, _)| name == &path)
        .and_then(|(_, bytes)| bytes.as_deref())
        .map(std::str::from_utf8)
        .transpose()
        .map_err(|err| super::TreeError::Git(format!("description UTF-8: {err}")))?;
    Ok(text.map_or_else(String::new, |text| {
        let mut body: String = text.chars().take(2000).collect();
        if text.chars().count() > 2000 {
            body.push('…');
        }
        format!("{body}\n\n")
    }))
}

/// What the daemon's log says of each change in `healed` that has notes:
/// healing folds it into a whole moment, so it never gets a commit of its
/// own, and a batch's notes go only on its own commit (never under another
/// change's). Its notes stay in the store, read by `notes` (`wall_notes`).
pub fn healed<'a>(healed: impl Iterator<Item = &'a Change>) -> Vec<String> {
    healed
        .filter(|change| !change.notes.is_empty())
        .map(|change| {
            let names = change
                .notes
                .iter()
                .map(|note| note.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "kazoo-wall: change {seq} is healed into a whole moment with no commit of its own, so its notes ({names}) are not on the tree; wall_notes {seq} reads them from the store",
                seq = change.seq
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_a_healed_batch_is_named_with_its_notes() {
        let change = |seq: u64, notes: serde_json::Value| -> Change {
            serde_json::from_value(serde_json::json!({"seq":seq,"at":"2026-10-17T00:00:00Z","seat":"Tom","summary":"s","notes":notes,"what":{"op":"tempo","from":120.0,"to":130.0,"desk":false}})).unwrap()
        };
        let changes = [
            change(4, serde_json::json!([])),
            change(
                5,
                serde_json::json!([{"name":"description.md","bytes":3,"sha256":"x"},{"name":"proof.md","bytes":3,"sha256":"y"}]),
            ),
        ];
        assert_eq!(
            healed(changes.iter()),
            vec![
                "kazoo-wall: change 5 is healed into a whole moment with no commit of its own, so its notes (description.md, proof.md) are not on the tree; wall_notes 5 reads them from the store"
                    .to_string()
            ]
        );
    }
}
