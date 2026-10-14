use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use super::model::{describe, differences, same};
use crate::migrate;
use crate::patch::{Patch, PatchFile, hash_bytes};
use crate::protocol::{Change, What, valid_save_name};
use crate::store::MAX_WALL_FILE;
use crate::tree::TreeError;

/// A wall file that holds the whole wall after change `after`: where it is
/// and what it proves. Only this is held through the walk; the file itself
/// is read again, bounded, each time the walk needs its wall.
#[derive(Debug, Clone)]
pub(super) struct Evidence {
    pub path: PathBuf,
    pub after: u64,
    /// The file has a `seed` key (one written before seeds has none).
    pub seeded: bool,
    pub version: u32,
    /// Which wall the file says it is.
    pub wall: u64,
    rank: u8,
    /// A load's verified backup: the fingerprint its bytes must still hold.
    fingerprint: Option<String>,
}

/// A file read whole (at most [`MAX_WALL_FILE`] bytes): its wall as
/// `migrate::read` gives it, unmigrated, whether it has a seed, and the
/// hash of its bytes.
struct Decoded {
    file: PatchFile,
    seeded: bool,
    hash: String,
}

impl Decoded {
    /// The wall, with seed 0 when the file was written before seeds.
    fn wall(self) -> PatchFile {
        let mut file = self.file;
        if !self.seeded {
            file.seed = 0;
        }
        file
    }
}

fn decode(path: &Path) -> Result<Decoded, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take(MAX_WALL_FILE + 1).read_to_end(&mut bytes))
        .map_err(|e| format!("it could not be read: {e}"))?;
    if !u64::try_from(bytes.len()).is_ok_and(|n| n <= MAX_WALL_FILE) {
        return Err(format!(
            "it is too large to be a wall (more than {MAX_WALL_FILE} bytes)"
        ));
    }
    let seeded = serde_json::from_slice::<serde_json::Value>(&bytes)
        .map_err(|e| format!("it is not JSON: {e}"))?
        .get("seed")
        .is_some();
    let (file, _) = migrate::read(&bytes)?;
    Ok(Decoded {
        file,
        seeded,
        hash: hash_bytes(&bytes),
    })
}

impl Evidence {
    pub const fn is_backup(&self) -> bool {
        self.rank == 0
    }

    fn decoded(&self) -> Result<Decoded, TreeError> {
        let failed = |why: String| {
            TreeError::NotOneWall(format!(
                "{} could not be read again: {why}",
                self.path.display()
            ))
        };
        let decoded = decode(&self.path).map_err(failed)?;
        if self
            .fingerprint
            .as_ref()
            .is_some_and(|fingerprint| *fingerprint != decoded.hash)
        {
            return Err(failed(
                "its bytes no longer hold the fingerprint its load logged".into(),
            ));
        }
        Ok(decoded)
    }

    /// The wall the file holds, as read and unmigrated; a file written
    /// before seeds has seed 0, as the wall had then.
    pub fn file(&self) -> Result<PatchFile, TreeError> {
        self.decoded().map(Decoded::wall)
    }

    /// The file exactly as `migrate::read` gives it to the daemon (a
    /// seedless file's seed taken from its bytes).
    pub fn as_read(&self) -> Result<PatchFile, TreeError> {
        Ok(self.decoded()?.file)
    }

    /// The file's name, for the commits and the report.
    pub fn label(&self) -> String {
        self.path.file_name().map_or_else(
            || self.path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        )
    }
}

/// Every wall file in the state directory and what it proves.
pub(super) struct Proofs {
    pub files: Vec<Evidence>,
    pub patch: Option<Evidence>,
    pub unreadable: Vec<String>,
    pub bad_backups: Vec<String>,
    pub backups: usize,
    pub disagreements: Vec<String>,
    pub count: usize,
}

fn wall_paths(state: &Path) -> Result<Vec<(PathBuf, u8)>, std::io::Error> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(state)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let rank = if name.starts_with("patch.before-") && name.ends_with(".json") {
            1
        } else if name == "patch.json" {
            2
        } else if name.starts_with("patch.") && name.ends_with(".json") {
            3
        } else {
            continue;
        };
        if entry.file_type()?.is_file() {
            paths.push((entry.path(), rank));
        }
    }
    match fs::read_dir(state.join("saves")) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                let name = entry.file_name();
                if name
                    .to_string_lossy()
                    .strip_suffix(".json")
                    .is_some_and(valid_save_name)
                    && entry.file_type()?.is_file()
                {
                    paths.push((entry.path(), 4));
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            paths.sort();
            return Ok(paths);
        }
        Err(e) => return Err(e),
    }
    paths.sort();
    Ok(paths)
}

impl Proofs {
    /// Index every wall file, each read once here (one at a time) and
    /// again only when the walk reaches it.
    pub fn read(state: &Path, changes: &[Change]) -> Result<Self, std::io::Error> {
        let mut proofs = Self {
            files: Vec::new(),
            patch: None,
            unreadable: Vec::new(),
            bad_backups: Vec::new(),
            backups: 0,
            disagreements: Vec::new(),
            count: 0,
        };
        let mut hashes = BTreeMap::new();
        for (path, rank) in wall_paths(state)? {
            match decode(&path) {
                Ok(decoded) => {
                    hashes.insert(path.clone(), decoded.hash);
                    let evidence = Evidence {
                        path,
                        after: decoded.file.last_seq,
                        seeded: decoded.seeded,
                        version: decoded.file.version,
                        wall: decoded.file.wall,
                        rank,
                        fingerprint: None,
                    };
                    if evidence.path == state.join("patch.json") {
                        proofs.patch = Some(evidence.clone());
                    }
                    proofs.files.push(evidence);
                    proofs.count += 1;
                }
                Err(e) => proofs.unreadable.push(format!("{}: {e}", path.display())),
            }
        }
        for change in changes {
            let (What::Load {
                backup,
                fingerprint,
                ..
            }
            | What::Go {
                backup,
                fingerprint,
                ..
            }) = &change.what
            else {
                continue;
            };
            let path = state.join("saves").join(format!("{backup}.json"));
            let held = proofs.files.iter().find(|e| e.path == path);
            match held {
                Some(e) if hashes.get(&path) == Some(fingerprint) => {
                    let mut e = e.clone();
                    e.rank = 0;
                    e.after = change.seq.saturating_sub(1);
                    e.fingerprint = Some(fingerprint.clone());
                    proofs.files.push(e);
                    proofs.backups += 1;
                }
                Some(_) | None => proofs.bad_backups.push(backup.clone()),
            }
        }
        proofs
            .files
            .sort_by(|a, b| (a.after, a.rank, &a.path).cmp(&(b.after, b.rank, &b.path)));
        let mut files: Vec<Evidence> = Vec::new();
        for evidence in std::mem::take(&mut proofs.files) {
            let Some(first) = files.last().filter(|f| f.after == evidence.after) else {
                files.push(evidence);
                continue;
            };
            // Two files at a time: the one that decides, and this one.
            let said = match (first.file(), evidence.file()) {
                (Ok(a), Ok(b)) => describe(&differences(&a, &b, evidence.seeded)),
                (Err(e), _) | (_, Err(e)) => e.to_string(),
            };
            if !said.is_empty() {
                proofs.disagreements.push(format!(
                    "{} and {} disagree after change {}: {said}",
                    first.label(),
                    evidence.label(),
                    evidence.after,
                ));
            }
        }
        proofs.files = files;
        Ok(proofs)
    }

    pub fn at(&self, seq: u64) -> Option<&Evidence> {
        self.files.iter().find(|e| e.after == seq)
    }

    /// The wall the load `change` put up, or why it is not known.
    pub fn incoming(
        &self,
        state: &Path,
        change: &Change,
        changes: &[Change],
    ) -> Result<(PatchFile, Option<String>), String> {
        let (save, modules, cables, tempo) = match &change.what {
            What::Load {
                save,
                modules,
                cables,
                tempo,
                ..
            } => (save.as_deref(), modules, cables, tempo),
            What::Go {
                modules,
                cables,
                tempo,
                ..
            } => {
                if let Some(undone) = change.undoes {
                    return self.backup(undone);
                }
                (None, modules, cables, tempo)
            }
            _ => return Err(format!("change {} is not a load or go", change.seq)),
        };
        let next = changes
            .iter()
            .find(|c| {
                c.seq > change.seq
                    && !matches!(
                        c.what,
                        What::Record { .. } | What::Speak { .. } | What::Abandoned { .. }
                    )
            })
            .map_or(u64::MAX, |c| c.seq);
        if let Some(e) = self
            .files
            .iter()
            .find(|e| e.after >= change.seq && e.after < next)
        {
            let mut file = e.file().map_err(|e| e.to_string())?;
            if let Some(name) = save.filter(|_| file.modules.iter().any(|m| m.birth.is_none())) {
                let source = saved_before(state, name, change, *modules, *cables, *tempo).and_then(
                    |source| {
                        let mismatches = differences(&source, &file, false);
                        if mismatches.is_empty() {
                            Ok(source)
                        } else {
                            Err(format!(
                                "it differs from the snapshot: {}",
                                describe(&mismatches)
                            ))
                        }
                    },
                );
                match source {
                    Ok(source) => {
                        // A post-load snapshot has the new wall number; the
                        // saved modules' identities belong to the source wall.
                        file.wall = source.wall;
                        for module in &mut file.modules {
                            if let Some(original) =
                                source.modules.iter().find(|m| m.id == module.id)
                            {
                                module.birth.clone_from(&original.birth);
                            }
                        }
                    }
                    Err(reason) => {
                        return Ok((
                            file,
                            Some(format!(
                                "the snapshot holds the wall, but the save \"{name}\" does not prove its earlier births: {reason}"
                            )),
                        ));
                    }
                }
            }
            return Ok((file, None));
        }
        if save.is_none() && change.undoes.is_none() && matches!(change.what, What::Load { .. }) {
            let mut file = Patch::empty(*tempo).to_file();
            if let Some(e) = self.files.iter().find(|e| e.wall == change.seq && e.seeded) {
                file.seed = e.file().map_err(|e| e.to_string())?.seed;
            }
            return Ok((file, None));
        }
        if let Some(undone) = change.undoes {
            return self.backup(undone);
        }
        if matches!(change.what, What::Go { .. }) {
            return Err("no kept file proves the wall after the go".into());
        }
        let name = save.unwrap_or_default();
        saved_before(state, name, change, *modules, *cables, *tempo).map(|file| (file, None))
    }

    fn backup(&self, undone: u64) -> Result<(PatchFile, Option<String>), String> {
        self.files.iter()
                .find(|e| e.rank == 0 && e.after == undone.saturating_sub(1))
                .map_or_else(
                    || Err(format!(
                        "the backup of the wall load {undone} replaced does not hold its fingerprint"
                    )),
                    |e| e.file().map(|file| (file, None)).map_err(|e| e.to_string()),
                )
    }
}

fn saved_before(
    state: &Path,
    name: &str,
    change: &Change,
    modules: u32,
    cables: u32,
    tempo: f64,
) -> Result<PatchFile, String> {
    if !valid_save_name(name) {
        return Err(format!(
            "the save name at change {} is not valid",
            change.seq
        ));
    }
    let path = state.join("saves").join(format!("{name}.json"));
    let file = decode(&path)?.wall();
    let at = chrono::NaiveDateTime::parse_from_str(&change.at, "%Y-%m-%dT%H:%M:%SZ")
        .map_err(|e| format!("its time {} does not read: {e}", change.at))?
        .and_utc();
    let modified = fs::metadata(&path)
        .and_then(|m| m.modified())
        .map_err(|e| format!("when it was last written is not known: {e}"))?;
    if chrono::DateTime::<chrono::Utc>::from(modified) >= at {
        Err("it was written again after the load".into())
    } else if u32::try_from(file.modules.len()) != Ok(modules)
        || u32::try_from(file.cables.len()) != Ok(cables)
        || !same(file.tempo, tempo)
    {
        Err("its modules, cables or tempo are not those the load put up".into())
    } else {
        Ok(file)
    }
}
