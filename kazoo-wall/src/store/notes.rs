use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;

use sha2::{Digest, Sha256};

use super::Store;
use crate::protocol::notes::{Attachment, NoteRef};

fn digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

impl Store {
    pub fn write_notes(
        &self,
        description: Option<&str>,
        attachments: &[Attachment],
    ) -> io::Result<Vec<NoteRef>> {
        if description.is_none() && attachments.is_empty() {
            return Ok(Vec::new());
        }
        let dir = self.dir().join("notes");
        if dir.try_exists()? {
            if !dir.metadata()?.is_dir() {
                return Err(io::Error::other("notes path is not a directory"));
            }
        } else {
            fs::DirBuilder::new().mode(0o700).create(&dir)?;
            File::open(self.dir())?.sync_all()?;
        }
        let texts = description
            .into_iter()
            .map(|text| ("description.md", text))
            .chain(
                attachments
                    .iter()
                    .map(|a| (a.name.as_str(), a.text.as_str())),
            );
        let mut refs = Vec::new();
        for (name, text) in texts {
            let sha256 = digest(text);
            let path = dir.join(&sha256);
            match fs::read(&path) {
                Ok(bytes) if bytes == text.as_bytes() => {}
                Ok(_) => {
                    return Err(io::Error::other(format!(
                        "notes store has corrupt content {sha256}"
                    )));
                }
                Err(err) if err.kind() == io::ErrorKind::NotFound => {
                    let temporary = dir.join(format!(".{sha256}.tmp"));
                    if temporary.try_exists()? {
                        fs::remove_file(&temporary)?;
                    }
                    let written = (|| {
                        let mut file = OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .mode(0o600)
                            .open(&temporary)?;
                        file.write_all(text.as_bytes())?;
                        file.sync_all()?;
                        fs::rename(&temporary, &path)
                    })();
                    if written.is_err() {
                        super::tidy_away(&temporary);
                    }
                    written?;
                }
                Err(err) => return Err(err),
            }
            refs.push(NoteRef {
                name: name.into(),
                bytes: text.len(),
                sha256,
            });
        }
        File::open(dir)?.sync_all()?;
        Ok(refs)
    }
}

pub fn read(state: &Path, note: &NoteRef) -> io::Result<String> {
    if note.sha256.len() != 64
        || !note.sha256.bytes().all(|c| c.is_ascii_hexdigit())
        || note.bytes > 64 * 1024
    {
        return Err(io::Error::other(format!(
            "invalid note reference {}",
            note.name
        )));
    }
    let file = File::open(state.join("notes").join(&note.sha256))?;
    let mut text = String::new();
    file.take(64 * 1024 + 1).read_to_string(&mut text)?;
    if text.len() != note.bytes || digest(&text) != note.sha256 {
        return Err(io::Error::other(format!(
            "note {} fails its size or SHA-256 check",
            note.name
        )));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_stale_temporary_does_not_prevent_retry() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        std::fs::create_dir(dir.path().join("notes")).unwrap();
        std::fs::write(
            dir.path()
                .join("notes")
                .join(format!(".{}.tmp", digest("QED"))),
            "partial",
        )
        .unwrap();
        let refs = store.write_notes(Some("QED"), &[]).unwrap();
        assert_eq!(read(dir.path(), &refs[0]).unwrap(), "QED");
    }
}
