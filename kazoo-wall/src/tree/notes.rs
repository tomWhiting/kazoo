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

pub fn whole<'a>(
    state: &Path,
    history: impl Iterator<Item = &'a Change>,
    files: &mut super::Files,
) {
    for change in history {
        let mut added = Vec::new();
        append(state, change, &mut added);
        for (path, bytes) in added {
            if let Some(bytes) = bytes {
                files.insert(path, bytes);
            }
        }
    }
}

pub fn wall_only(files: &super::Files) -> super::Files {
    files
        .iter()
        .filter(|(path, _)| !valid_path(path))
        .map(|(path, bytes)| (path.clone(), bytes.clone()))
        .collect()
}
