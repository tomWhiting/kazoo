use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::DirBuilderExt;

use super::{MAX_WALL_FILE, SaveError, Store, full_flush, read_wall_file, write_durably};
use crate::patch::Chain;
use crate::protocol::valid_save_name;

impl Store {
    pub fn list_chains(&self) -> io::Result<Vec<String>> {
        let dir = self.dir.join("chains");
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err),
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            if let Some(name) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_suffix(".json"))
                && valid_save_name(name)
            {
                names.push(name.to_string());
            }
        }
        names.sort();
        Ok(names)
    }

    pub fn write_chain(&self, chain: &Chain, overwrite: bool) -> Result<(), SaveError> {
        chain
            .validate()
            .map_err(|err| io::Error::other(err.message))?;
        let found = self
            .list_chains()?
            .into_iter()
            .find(|name| name.eq_ignore_ascii_case(&chain.name));
        if found.is_some() && !overwrite {
            return Err(SaveError::Exists(
                found.unwrap_or_else(|| chain.name.clone()),
            ));
        }
        let name = found.unwrap_or_else(|| chain.name.clone());
        let dir = self.dir.join("chains");
        if !dir.is_dir() {
            DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
            full_flush(&self.dir)?;
        }
        let mut saved = chain.clone();
        saved.name.clone_from(&name);
        let mut bytes = serde_json::to_vec_pretty(&saved).map_err(io::Error::other)?;
        bytes.push(b'\n');
        if bytes.len() as u64 > MAX_WALL_FILE {
            return Err(SaveError::TooLarge(bytes.len()));
        }
        if let Some(err) = write_durably(&dir, &format!("{name}.json"), &bytes, None, || Ok(()))? {
            return Err(err.into());
        }
        Ok(())
    }

    pub fn read_chain(&self, name: &str) -> Result<Chain, SaveError> {
        if !valid_save_name(name) {
            return Err(io::Error::other("invalid chain name").into());
        }
        let found = self
            .list_chains()?
            .into_iter()
            .find(|saved| saved.eq_ignore_ascii_case(name))
            .ok_or_else(|| SaveError::Unknown(name.to_string()))?;
        let broken = |reason: String| SaveError::Broken {
            name: found.clone(),
            reason,
        };
        let bytes = read_wall_file(&self.dir.join("chains").join(format!("{found}.json")))?
            .map_err(&broken)?;
        let chain: Chain = serde_json::from_slice(&bytes).map_err(|err| broken(err.to_string()))?;
        chain.validate().map_err(|err| broken(err.message))?;
        if !chain.name.eq_ignore_ascii_case(&found) {
            return Err(broken(
                "the chain's name does not match its file".to_string(),
            ));
        }
        Ok(chain)
    }
}
