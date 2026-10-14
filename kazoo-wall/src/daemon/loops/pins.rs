//! Complete references fail closed before any reel is reclaimed.
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::patch::PatchFile;
use crate::protocol::{Change, TakeWhy, What};
use crate::store::Store;
use kazoo_core::reels::{REELS_BUDGET_BYTES, ReelId, ReelStore, SWEEP_GRACE};

/// The patch, saves, both log generations and the permanent wall history.
#[derive(Debug, Clone, Default)]
pub struct ReelRefs {
    pub ids: BTreeSet<ReelId>,
    protected: BTreeSet<ReelId>,
    patch: BTreeSet<ReelId>,
    checkpoints: BTreeMap<u64, (String, ReelId)>,
    newest: BTreeMap<String, ReelId>,
    saves: BTreeMap<String, (SystemTime, BTreeSet<ReelId>)>,
    changes: BTreeMap<u64, What>,
}

impl ReelRefs {
    fn patch_ids(file: &PatchFile) -> Result<BTreeSet<ReelId>, String> {
        file.loops
            .values()
            .chain(file.retired_loops.iter().map(|item| &item.record))
            .filter_map(|record| record.tape.as_ref())
            .map(|tape| {
                tape.reel
                    .parse()
                    .map_err(|error| format!("unreadable reel reference: {error}"))
            })
            .collect()
    }

    /// Read each save and both log files before the first pin or after rotation.
    pub fn read(
        file: &PatchFile,
        store: Option<&mut Store>,
        changes: &[Change],
    ) -> Result<Self, String> {
        let mut refs = Self::default();
        refs.refresh(file, store, changes, true)?;
        Ok(refs)
    }

    pub fn refresh(
        &mut self,
        file: &PatchFile,
        store: Option<&mut Store>,
        changes: &[Change],
        reread_log: bool,
    ) -> Result<(), String> {
        self.patch = Self::patch_ids(file)?;
        if let Some(store) = store {
            let saves = store.list_saves().map_err(|error| error.to_string())?;
            let names: BTreeSet<_> = saves.iter().map(|save| save.name.clone()).collect();
            self.saves.retain(|name, _| names.contains(name));
            for save in saves {
                if self
                    .saves
                    .get(&save.name)
                    .is_none_or(|(at, _)| *at != save.saved_at)
                {
                    let read = store.read_save(&save.name).map_err(|error| {
                        format!("{}'s reel references cannot be read: {error}", save.name)
                    })?;
                    self.saves.insert(
                        save.name,
                        (save.saved_at, Self::patch_ids(&read.patch.to_file())?),
                    );
                }
            }
            if reread_log {
                let log = store.load_log().map_err(|error| error.to_string())?;
                if log.skipped != 0 || log.repeated != 0 {
                    return Err(
                        "the change log has unreadable or repeated lines; no reels are reclaimed"
                            .into(),
                    );
                }
                self.changes = log
                    .changes
                    .into_iter()
                    .map(|change| (change.seq, change.what))
                    .collect();
            }
        }
        for change in changes {
            self.changes.insert(change.seq, change.what.clone());
        }
        self.ids = self.patch.clone();
        self.protected = self
            .saves
            .values()
            .flat_map(|(_, ids)| ids.iter().copied())
            .collect();
        self.ids.extend(self.protected.iter().copied());
        self.checkpoints.clear();
        self.newest.clear();
        let changes: Vec<_> = self
            .changes
            .iter()
            .map(|(seq, what)| (*seq, what.clone()))
            .collect();
        for (seq, what) in changes {
            self.index(seq, &what)?;
        }
        Ok(())
    }

    pub fn set_patch(&mut self, file: &PatchFile) -> Result<(), String> {
        self.patch = Self::patch_ids(file)?;
        self.ids.extend(self.patch.iter().copied());
        Ok(())
    }

    pub fn add(&mut self, seq: u64, what: &What) -> Result<(), String> {
        self.changes.insert(seq, what.clone());
        self.index(seq, what)
    }

    fn index(&mut self, seq: u64, what: &What) -> Result<(), String> {
        match what {
            What::Take {
                module,
                tape,
                was,
                why,
                ..
            } => {
                for record in tape.iter().chain(was) {
                    self.ids.insert(
                        record
                            .reel
                            .parse()
                            .map_err(|error| format!("bad take reel: {error}"))?,
                    );
                }
                if let Some(tape) = tape {
                    let id = tape
                        .reel
                        .parse()
                        .map_err(|error| format!("bad take reel: {error}"))?;
                    self.newest.insert(module.clone(), id);
                    if *why == TakeWhy::Checkpoint {
                        self.checkpoints.insert(seq, (module.clone(), id));
                    } else {
                        self.protected.insert(id);
                    }
                }
            }
            What::Erase { was, tape, .. } => {
                for record in was.iter().chain(tape) {
                    self.ids.insert(
                        record
                            .reel
                            .parse()
                            .map_err(|error| format!("bad erase reel: {error}"))?,
                    );
                }
            }
            What::Chain { changes, .. } => {
                for change in changes {
                    self.index(seq, change)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// Worker-owned pin and retention state. Every operation here does disk I/O.
#[derive(Debug)]
pub struct Retention {
    refs: ReelRefs,
    history: BTreeSet<ReelId>,
    unconfirmed: BTreeSet<ReelId>,
    checkpoints: Vec<(String, ReelId)>,
    pub pruned: BTreeMap<ReelId, String>,
    pub paused: bool,
    budget: u64,
    complete: bool,
}

impl Retention {
    pub fn new(dir: &Path, budget: u64) -> Result<Self, String> {
        let history = match optional_file(&dir.join("pins/wall-history.txt"))? {
            Some(file) => read_pin_file(file, "wall-history")?,
            None => BTreeSet::new(),
        };
        let mut pruned = BTreeMap::new();
        if let Some(file) = optional_file(&dir.join("pruned.jsonl"))? {
            for line in BufReader::new(file).lines() {
                let (id, at): (String, String) =
                    serde_json::from_str(&line.map_err(|error| error.to_string())?)
                        .map_err(|error| error.to_string())?;
                pruned.insert(
                    id.parse()
                        .map_err(|error| format!("bad pruned reel: {error}"))?,
                    at,
                );
            }
        }
        let mut absent = BTreeMap::new();
        for (id, at) in pruned {
            if !dir
                .join(format!("{id}.wav"))
                .try_exists()
                .map_err(|error| error.to_string())?
            {
                absent.insert(id, at);
            }
        }
        let pruned = absent;
        let mut checkpoints = Vec::new();
        if let Some(file) = optional_file(&dir.join("checkpoints.jsonl"))? {
            for line in BufReader::new(file).lines() {
                let (module, id): (String, String) =
                    serde_json::from_str(&line.map_err(|error| error.to_string())?)
                        .map_err(|error| error.to_string())?;
                checkpoints.push((
                    module,
                    id.parse()
                        .map_err(|error| format!("bad checkpoint reel: {error}"))?,
                ));
            }
        }
        Ok(Self {
            refs: ReelRefs::default(),
            history,
            unconfirmed: BTreeSet::new(),
            checkpoints,
            pruned,
            paused: false,
            budget,
            complete: false,
        })
    }

    pub const fn uncertain(&mut self) {
        self.complete = false;
    }

    pub fn pin(&mut self, store: &ReelStore, refs: ReelRefs) -> Result<(), String> {
        self.complete = false;
        let mut refs = refs;
        self.unconfirmed.retain(|id| !refs.ids.contains(id));
        refs.ids.extend(self.unconfirmed.iter().copied());
        refs.ids.extend(self.history.iter().copied());
        for (module, id) in self.checkpoints.iter().rev() {
            refs.newest.entry(module.clone()).or_insert(*id);
        }
        refs.ids.extend(refs.newest.values().copied());
        store
            .pin("wall", &refs.ids)
            .map_err(|error| error.to_string())?;
        self.refs = refs;
        self.complete = true;
        Ok(())
    }

    /// A durable history append and main pin precede the Stored reply.
    pub fn stored(
        &mut self,
        store: &ReelStore,
        dir: &Path,
        module: &str,
        id: ReelId,
        why: TakeWhy,
    ) -> Result<(), String> {
        if why == TakeWhy::Checkpoint {
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("checkpoints.jsonl"))
                .map_err(|error| error.to_string())?;
            serde_json::to_writer(&mut file, &(module, id.to_string()))
                .map_err(|error| error.to_string())?;
            file.write_all(b"\n")
                .and_then(|()| file.sync_all())
                .map_err(|error| error.to_string())?;
            File::open(dir)
                .and_then(|file| file.sync_all())
                .map_err(|error| error.to_string())?;
            self.checkpoints.push((module.to_string(), id));
        } else {
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("pins/wall-history.txt"))
                .map_err(|error| error.to_string())?;
            let line = format!("{id}\n");
            file.write_all(line.as_bytes())
                .map_err(|error| error.to_string())?;
            file.sync_all().map_err(|error| error.to_string())?;
            File::open(dir.join("pins"))
                .and_then(|file| file.sync_all())
                .map_err(|error| error.to_string())?;
            self.history.insert(id);
        }
        self.pruned.remove(&id);
        self.unconfirmed.insert(id);
        self.refs.ids.insert(id);
        self.refs.newest.insert(module.to_string(), id);
        self.refs.ids.extend(self.history.iter().copied());
        if let Err(error) = store.pin("wall", &self.refs.ids) {
            self.complete = false;
            return Err(error.to_string());
        }
        Ok(())
    }

    pub fn sweep(&mut self, store: &ReelStore, dir: &Path, now: SystemTime) -> Result<(), String> {
        if !self.complete {
            return Err("reel references are incomplete; no reels are reclaimed".into());
        }
        let pinned = read_pins(&dir.join("pins"), false)?;
        let mut candidates = Vec::new();
        for entry in fs::read_dir(dir).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            if !entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_file()
            {
                continue;
            }
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != "wav") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            let Ok(id) = stem.parse::<ReelId>() else {
                continue;
            };
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .map_err(|error| error.to_string())?;
            let age = now
                .duration_since(modified)
                .map_err(|error| format!("reel timestamp is ahead of the sweep clock: {error}"))?;
            if !pinned.contains(&id) && age >= SWEEP_GRACE {
                candidates.push(id);
            }
        }
        for id in candidates {
            if self
                .checkpoints
                .iter()
                .any(|(_, checkpoint)| *checkpoint == id)
            {
                self.prune(store, dir, id, now)?;
            } else {
                store.remove(&id).map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    }

    pub fn budget(&mut self, store: &ReelStore, dir: &Path, now: SystemTime) -> Result<(), String> {
        let mut usage = match store.usage() {
            Ok(usage) => usage,
            Err(error) => {
                self.paused = true;
                return Err(error.to_string());
            }
        };
        let target = self.budget / 10 * 9;
        if usage.bytes < target {
            self.paused = false;
            return Ok(());
        }
        if usage.bytes <= self.budget && !self.paused {
            return Ok(());
        }
        if !self.complete {
            self.paused = true;
            return Err("reel references are incomplete; checkpoints cannot be pruned".into());
        }
        let was_paused = self.paused;
        self.paused = true;
        let elsewhere = read_pins(&dir.join("pins"), true)?;
        let mut candidates = self.checkpoints.clone();
        candidates.extend(self.refs.checkpoints.values().cloned());
        let mut seen = BTreeSet::new();
        for (module, id) in &candidates {
            if !seen.insert(*id) {
                continue;
            }
            if usage.bytes < target {
                break;
            }
            if self.unconfirmed.contains(id)
                || self.refs.patch.contains(id)
                || self.refs.protected.contains(id)
                || elsewhere.contains(id)
                || self.refs.newest.get(module) == Some(id)
                || !store.contains(id)
            {
                continue;
            }
            self.prune(store, dir, *id, now)?;
            usage = store.usage().map_err(|error| error.to_string())?;
        }
        self.paused = if was_paused {
            usage.bytes >= target
        } else {
            usage.bytes > self.budget
        };
        Ok(())
    }
    fn prune(
        &mut self,
        store: &ReelStore,
        dir: &Path,
        id: ReelId,
        now: SystemTime,
    ) -> Result<(), String> {
        let at = crate::change::utc(
            now.duration_since(SystemTime::UNIX_EPOCH)
                .map_err(|error| error.to_string())?
                .as_secs(),
        );
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("pruned.jsonl"))
            .map_err(|error| error.to_string())?;
        serde_json::to_writer(&mut file, &(id.to_string(), &at))
            .map_err(|error| error.to_string())?;
        file.write_all(b"\n")
            .and_then(|()| file.sync_all())
            .map_err(|error| error.to_string())?;
        File::open(dir)
            .and_then(|file| file.sync_all())
            .map_err(|error| error.to_string())?;
        store.remove(&id).map_err(|error| error.to_string())?;
        self.pruned.insert(id, at);
        Ok(())
    }
}

pub const DEFAULT_BUDGET: u64 = REELS_BUDGET_BYTES;
pub const SAVE_WAIT: Duration = Duration::from_secs(5);

/// Rotation changes this stamp; ordinary appends leave it alone.
pub fn rotation(store: &Store) -> Result<Option<SystemTime>, String> {
    match fs::metadata(store.dir().join("log.1.jsonl")) {
        Ok(metadata) => metadata
            .modified()
            .map(Some)
            .map_err(|error| error.to_string()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn optional_file(path: &Path) -> Result<Option<File>, String> {
    match File::open(path) {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{} cannot be read: {error}", path.display())),
    }
}

/// Validate every holder before collecting any deletion candidates. The
/// permanent append-only history has no replaceable end marker.
fn read_pins(dir: &Path, except_wall: bool) -> Result<BTreeSet<ReelId>, String> {
    let mut pinned = BTreeSet::new();
    for entry in fs::read_dir(dir).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.extension().is_none_or(|extension| extension != "txt") {
            continue;
        }
        let holder = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or("pin holder is not UTF-8")?;
        let file = File::open(&path)
            .map_err(|error| format!("reel pin list for {holder} is unreadable: {error}"))?;
        let ids = read_pin_file(file, holder)?;
        if !except_wall || holder != "wall" {
            pinned.extend(ids);
        }
    }
    Ok(pinned)
}

fn read_pin_file(file: File, holder: &str) -> Result<BTreeSet<ReelId>, String> {
    let mut reader = BufReader::new(file);
    let mut ids = BTreeSet::new();
    let history = holder == "wall-history";
    let mut ended = history;
    let mut line = Vec::with_capacity(66);
    loop {
        line.clear();
        Read::by_ref(&mut reader)
            .take(66)
            .read_until(b'\n', &mut line)
            .map_err(|error| format!("reel pin list for {holder} is unreadable: {error}"))?;
        if line.is_empty() {
            break;
        }
        let newline = line.last() == Some(&b'\n');
        if newline {
            line.pop();
        }
        if history && !newline {
            return Err(format!("reel pin list for {holder} is cut short"));
        }
        if !history && line == b"end" {
            if ended {
                return Err(format!("reel pin list for {holder} repeats its end marker"));
            }
            ended = true;
        } else {
            if !history && ended {
                return Err(format!(
                    "reel pin list for {holder} has data after its end marker"
                ));
            }
            let text = std::str::from_utf8(&line)
                .map_err(|error| format!("reel pin list for {holder} is unreadable: {error}"))?;
            ids.insert(
                text.parse().map_err(|error| {
                    format!("reel pin list for {holder} is unreadable: {error}")
                })?,
            );
        }
    }
    if !ended {
        return Err(format!(
            "reel pin list for {holder} is cut short; no reels are reclaimed"
        ));
    }
    Ok(ids)
}
