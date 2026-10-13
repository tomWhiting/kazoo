use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::migrate;
use crate::patch::{Patch, PatchFile, RetiredWords, hash_bytes, split_id};
use crate::protocol::{CableRecord, Change, ModuleRecord, What, valid_save_name};
use crate::store::MAX_WALL_FILE;
use crate::tree::{Birth, Files, TreeError, to_files};

#[derive(Clone)]
pub(super) struct Evidence {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
    pub file: PatchFile,
    pub after: u64,
    pub seeded: bool,
    rank: u8,
}

pub(super) struct Proofs {
    pub files: Vec<Evidence>,
    pub unreadable: Vec<String>,
    pub bad_backups: Vec<String>,
    pub backups: usize,
    pub disagreements: Vec<String>,
    pub count: usize,
}

fn read(path: &Path, rank: u8) -> Result<Evidence, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| e.to_string())?
        .take(MAX_WALL_FILE + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_WALL_FILE {
        return Err(format!(
            "it is too large to be a wall (more than {MAX_WALL_FILE} bytes)"
        ));
    }
    let seeded = serde_json::from_slice::<serde_json::Value>(&bytes)
        .map_err(|e| e.to_string())?
        .get("seed")
        .is_some();
    let (mut file, _) = migrate::read(&bytes)?;
    if !seeded {
        file.seed = 0;
    }
    Ok(Evidence {
        after: file.last_seq,
        file,
        bytes,
        path: path.to_path_buf(),
        seeded,
        rank,
    })
}

impl Proofs {
    pub fn read(state: &Path, changes: &[Change]) -> Result<Self, std::io::Error> {
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
        let saves = match fs::read_dir(state.join("saves")) {
            Ok(entries) => Some(entries),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        if let Some(entries) = saves {
            for entry in entries {
                let entry = entry?;
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.strip_suffix(".json").is_some_and(valid_save_name)
                    && entry.file_type()?.is_file()
                {
                    paths.push((entry.path(), 4));
                }
            }
        }
        paths.sort();
        let mut proofs = Self {
            files: Vec::new(),
            unreadable: Vec::new(),
            bad_backups: Vec::new(),
            backups: 0,
            disagreements: Vec::new(),
            count: 0,
        };
        for (path, rank) in paths {
            match read(&path, rank) {
                Ok(file) => {
                    proofs.files.push(file);
                    proofs.count += 1;
                }
                Err(e) => proofs.unreadable.push(format!("{}: {e}", path.display())),
            }
        }
        for change in changes {
            if let What::Load {
                backup,
                fingerprint,
                ..
            } = &change.what
            {
                let path = state.join("saves").join(format!("{backup}.json"));
                match proofs.files.iter().find(|e| e.path == path) {
                    Some(e) if hash_bytes(&e.bytes) == *fingerprint => {
                        let mut e = e.clone();
                        e.rank = 0;
                        e.after = change.seq.saturating_sub(1);
                        proofs.files.push(e);
                        proofs.backups += 1;
                    }
                    _ => proofs.bad_backups.push(backup.clone()),
                }
            }
        }
        proofs
            .files
            .sort_by(|a, b| (a.after, a.rank, &a.path).cmp(&(b.after, b.rank, &b.path)));
        let mut files: Vec<Evidence> = Vec::new();
        for evidence in std::mem::take(&mut proofs.files) {
            if let Some(first) = files.last().filter(|f| f.after == evidence.after) {
                let differences = differences(&first.file, &evidence.file, evidence.seeded);
                if !differences.is_empty() {
                    proofs.disagreements.push(format!(
                        "{} and {} disagree after change {}: {}",
                        first.path.display(),
                        evidence.path.display(),
                        evidence.after,
                        describe(&differences)
                    ));
                }
            } else {
                files.push(evidence);
            }
        }
        proofs.files = files;
        Ok(proofs)
    }

    pub fn at(&self, seq: u64) -> Option<&Evidence> {
        self.files.iter().find(|e| e.after == seq)
    }

    pub fn incoming(&self, state: &Path, change: &Change, changes: &[Change]) -> Option<PatchFile> {
        let What::Load {
            save,
            modules,
            cables,
            tempo,
            ..
        } = &change.what
        else {
            return None;
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
            return Some(e.file.clone());
        }
        if save.is_none() && change.undoes.is_none() {
            let mut file = Patch::empty(*tempo).to_file();
            file.seed = self
                .files
                .iter()
                .find(|e| e.file.wall == change.seq && e.seeded)
                .map_or(0, |e| e.file.seed);
            return Some(file);
        }
        if let Some(undone) = change.undoes {
            return self
                .files
                .iter()
                .find(|e| e.rank == 0 && e.after == undone.saturating_sub(1))
                .map(|e| e.file.clone());
        }
        let path = state.join("saves").join(format!("{}.json", save.as_ref()?));
        let Ok(e) = read(&path, 4) else { return None };
        let at = match chrono::NaiveDateTime::parse_from_str(&change.at, "%Y-%m-%dT%H:%M:%SZ") {
            Ok(at) => at.and_utc(),
            Err(_) => return None,
        };
        let modified = match fs::metadata(&path).and_then(|m| m.modified()) {
            Ok(t) => chrono::DateTime::<chrono::Utc>::from(t),
            Err(_) => return None,
        };
        if modified < at
            && u32::try_from(e.file.modules.len()) == Ok(*modules)
            && u32::try_from(e.file.cables.len()) == Ok(*cables)
            && same(e.file.tempo, *tempo)
        {
            Some(e.file)
        } else {
            None
        }
    }
}

pub(super) const fn same(a: f64, b: f64) -> bool {
    // Knob values are held to f32 ranges: the narrowing is exact enough.
    a.is_finite() && b.is_finite() && (a as f32).to_bits() == (b as f32).to_bits()
}

fn module_same(a: &ModuleRecord, b: &ModuleRecord) -> bool {
    a.id == b.id
        && a.kind == b.kind
        && a.name == b.name
        && a.knobs.len() == b.knobs.len()
        && a.knobs
            .iter()
            .all(|(key, a)| b.knobs.get(key).is_some_and(|b| same(*a, *b)))
}

fn cable_same(a: &CableRecord, b: &CableRecord) -> bool {
    a.id == b.id && a.from == b.from && a.to == b.to && same(a.amount, b.amount)
}

pub(super) fn differences(a: &PatchFile, b: &PatchFile, seeded: bool) -> Vec<String> {
    let mut out = Vec::new();
    if !same(a.tempo, b.tempo) {
        out.push(format!("tempo {} BPM instead of {} BPM", b.tempo, a.tempo));
    }
    if seeded && a.seed != b.seed {
        out.push(format!("seed {:016x} instead of {:016x}", b.seed, a.seed));
    }
    for module in &a.modules {
        match b.modules.iter().find(|m| m.id == module.id) {
            None => out.push(format!("{} is absent", module.id)),
            Some(other) => {
                if module.kind != other.kind {
                    out.push(format!(
                        "{} kind {} instead of {}",
                        module.id, other.kind, module.kind
                    ));
                }
                if module.name != other.name {
                    out.push(format!("{} name differs", module.id));
                }
                for key in module.knobs.keys().chain(
                    other
                        .knobs
                        .keys()
                        .filter(|k| !module.knobs.contains_key(*k)),
                ) {
                    match (module.knobs.get(key), other.knobs.get(key)) {
                        (Some(a), Some(b)) if same(*a, *b) => {}
                        _ => out.push(format!("{}.{} differs", module.id, key)),
                    }
                }
            }
        }
    }
    for module in &b.modules {
        if !a.modules.iter().any(|m| m.id == module.id) {
            out.push(format!("{} is present", module.id));
        }
    }
    for cable in &a.cables {
        if !b.cables.iter().any(|c| cable_same(c, cable)) {
            out.push(format!("cable {} differs", cable.id));
        }
    }
    for cable in &b.cables {
        if !a.cables.iter().any(|c| c.id == cable.id) {
            out.push(format!("cable {} is present", cable.id));
        }
    }
    out
}

pub(super) fn describe(items: &[String]) -> String {
    let mut text = items.iter().take(8).cloned().collect::<Vec<_>>().join(", ");
    if items.len() > 8 {
        text = format!("{text}, and {} more", items.len() - 8);
    }
    text
}

pub(super) struct Model {
    pub file: PatchFile,
    pub known: bool,
    pub doubt: Option<String>,
    pub agreed: u64,
    pub births: BTreeMap<(u64, String), Birth>,
    pub found_start: usize,
    pub born: usize,
    pub kept_load: usize,
    pub found_load: usize,
    pub omitted: std::collections::BTreeSet<String>,
    tempo_since: u64,
}

pub(super) struct Applied {
    pub doubt: Option<String>,
    pub unknown: Option<String>,
}

impl Model {
    pub fn new(file: PatchFile, known: bool) -> Result<Self, TreeError> {
        let mut model = Self {
            file,
            known,
            doubt: None,
            agreed: 0,
            births: BTreeMap::new(),
            found_start: 0,
            born: 0,
            kept_load: 0,
            found_load: 0,
            omitted: std::collections::BTreeSet::new(),
            tempo_since: 0,
        };
        model.adopt_births(false)?;
        model.found_start = model
            .file
            .modules
            .iter()
            .filter(|m| {
                m.birth
                    .as_ref()
                    .is_some_and(|birth| birth.starts_with("found "))
            })
            .count();
        Ok(model)
    }

    pub fn adopt_births(&mut self, load: bool) -> Result<(), TreeError> {
        for module in &mut self.file.modules {
            let key = (self.file.wall, module.id.clone());
            let birth = if let Some(birth) = self.births.get(&key) {
                if load {
                    self.kept_load += 1;
                }
                birth.clone()
            } else if let Some(birth) = &module.birth {
                birth.parse()?
            } else {
                if load {
                    self.found_load += 1;
                } else {
                    self.found_start += 1;
                }
                Birth::Found {
                    wall: self.file.wall,
                    seed: self.file.seed,
                    id: module.id.clone(),
                }
            };
            module.birth = Some(birth.to_string());
            self.births.insert(key, birth);
        }
        Ok(())
    }

    pub fn adopt(&mut self, file: &PatchFile, seq: u64, load: bool) -> Result<(), TreeError> {
        self.file = file.clone();
        self.adopt_births(load)?;
        if load {
            self.file.wall = seq;
            for module in &self.file.modules {
                if let Some(birth) = &module.birth {
                    self.births.insert((seq, module.id.clone()), birth.parse()?);
                }
            }
        }
        self.file.last_seq = seq;
        self.known = true;
        self.doubt = None;
        self.agreed = seq;
        self.tempo_since = seq;
        Ok(())
    }

    pub fn patch(&mut self) -> Result<Patch, TreeError> {
        let mut view = self.file.clone();
        migrate::migrate(&mut view).map_err(TreeError::Git)?;
        view.seed = self.file.seed;
        let (mut patch, notes) = Patch::load(&view).map_err(TreeError::Git)?;
        for note in notes {
            self.omitted.insert(note);
        }
        for module in &self.file.modules {
            if let Some(birth) = &module.birth {
                let birth = birth.parse()?;
                if !patch.mark_birth(&module.id, birth) && patch.module(&module.id).is_some() {
                    return Err(TreeError::Git(format!(
                        "birth for {} was not marked",
                        module.id
                    )));
                }
            }
        }
        Ok(patch)
    }

    pub fn files(&mut self, change: Option<&Change>) -> Result<Files, TreeError> {
        let mut patch = self.patch()?;
        if let Some(change) = change {
            let mut dye = std::mem::take(&mut patch.dye);
            dye.touch(&change.seat, &change.what, &patch);
            patch.dye = dye;
        }
        let view = patch.to_file();
        self.file.dye = view.dye.clone();
        self.file.rack.clone_from(&view.rack);
        self.file.groups = view.groups.clone();
        self.file.retired_speech.clone_from(&view.retired_speech);
        to_files(&view)
    }

    pub fn stamp(&mut self, change: &Change, line: &[u8]) -> Result<(), TreeError> {
        if crate::tree::has_add(&change.what) {
            let patch = self.patch()?;
            let mut print_file = patch.to_file();
            let mut unmarked = change.what.clone();
            let pending = crate::tree::stamp_births(&mut unmarked, change.seq, &change.at, 0, 0);
            for (id, _) in &pending {
                if !self.births.contains_key(&(self.file.wall, id.clone())) {
                    if let Some(module) = print_file.modules.iter_mut().find(|m| m.id == *id) {
                        module.birth = None;
                    }
                }
            }
            print_file.last_seq = 0;
            print_file.wall = 0;
            let print = crate::chance::fnv1a64(
                &serde_json::to_vec(&print_file)
                    .map_err(|e| TreeError::Git(format!("birth fingerprint: {e}")))?,
            );
            let mut what = change.what.clone();
            let stamped = crate::tree::stamp_births(
                &mut what,
                change.seq,
                &change.at,
                print,
                crate::chance::fnv1a64(line),
            );
            for (id, birth) in stamped {
                let key = (self.file.wall, id.clone());
                let birth = self
                    .births
                    .entry(key)
                    .or_insert_with(|| {
                        self.born += 1;
                        birth
                    })
                    .clone();
                if let Some(module) = self.file.modules.iter_mut().find(|m| m.id == id) {
                    module.birth = Some(birth.to_string());
                }
            }
        }
        for module in &mut self.file.modules {
            let key = (self.file.wall, module.id.clone());
            if let Some(birth) = self.births.get(&key) {
                module.birth = Some(birth.to_string());
            } else if let Some(birth) = &module.birth {
                self.births.insert(key, birth.parse()?);
            }
        }
        Ok(())
    }

    pub fn apply(&mut self, change: &Change) -> Applied {
        let mut result = Applied {
            doubt: None,
            unknown: None,
        };
        self.forward(&change.what, change.seq, &mut result);
        self.file.last_seq = change.seq;
        if result.unknown.is_some() {
            self.known = false;
        }
        if let Some(reason) = &result.doubt {
            self.doubt = Some(reason.clone());
        } else if result.unknown.is_none() {
            self.agreed = change.seq;
        }
        result
    }

    fn check(result: &mut Applied, ok: bool, reason: String) {
        if !ok && result.doubt.is_none() {
            result.doubt = Some(reason);
        }
    }

    fn ends(&self, cable: &CableRecord) -> bool {
        [&cable.from, &cable.to].iter().all(|end| {
            end.split_once('.')
                .is_some_and(|(id, _)| self.file.modules.iter().any(|m| m.id == id))
        })
    }

    fn plug(&mut self, cable: &CableRecord) {
        self.file
            .cables
            .retain(|c| c.id != cable.id && c.to != cable.to);
        self.file.cables.push(cable.clone());
        self.file.cables.sort_by_key(|c| c.id);
        self.file.next_cable = self.file.next_cable.max(cable.id.saturating_add(1));
    }

    fn add(&mut self, module: &ModuleRecord) {
        self.file.modules.retain(|m| m.id != module.id);
        self.file.modules.push(module.clone());
        if let Some((kind, n)) = split_id(&module.id) {
            let next = self.file.next_ids.entry(kind.into()).or_insert(1);
            *next = (*next).max(n.saturating_add(1));
        }
    }

    fn remove(&mut self, id: &str) {
        self.file.modules.retain(|m| m.id != id);
        self.file.cables.retain(|c| !at_module(c, id));
        self.file.dye.remove(id);
        for row in &mut self.file.rack {
            row.retain(|m| m != id);
        }
        self.file.rack.retain(|row| !row.is_empty());
        for group in self.file.groups.values_mut() {
            group.retain(|m| m != id);
        }
        self.file.groups.retain(|_, ids| !ids.is_empty());
        if let Some(words) = self.file.speech.remove(id) {
            self.file.retired_speech.push(RetiredWords {
                module: id.into(),
                words,
            });
        }
    }

    fn forward(&mut self, what: &What, seq: u64, result: &mut Applied) {
        match what {
            What::Turn { .. } | What::Tempo { .. } => self.forward_values(what, seq, result),
            What::Patch { .. } | What::Unpatch { .. } => self.forward_cables(what, seq, result),
            What::Add { .. } | What::Remove { .. } | What::Restore { .. } => {
                self.forward_modules(what, seq, result);
            }
            What::Chain { changes, .. } => {
                for inner in changes {
                    self.forward(inner, seq, result);
                    if result.unknown.is_some() {
                        break;
                    }
                }
            }
            What::Record { .. } | What::Speak { .. } | What::Abandoned { .. } => {}
            _ => {
                result.unknown = Some(crate::change::describe(what));
            }
        }
    }
    fn forward_values(&mut self, what: &What, seq: u64, result: &mut Applied) {
        match what {
            What::Turn {
                module,
                knob,
                from,
                to,
                ..
            } => {
                let Some(target) = self
                    .file
                    .modules
                    .iter_mut()
                    .find(|m| m.id == *module)
                    .and_then(|m| m.knobs.get_mut(knob))
                else {
                    result.unknown = Some(format!(
                        "change {seq} turns {module}.{knob}, which the known wall does not have"
                    ));
                    return;
                };
                Self::check(
                    result,
                    same(*target, *from),
                    format!(
                        "{module}.{knob} was {from}, not {target} as the log has it since change {}",
                        self.agreed
                    ),
                );
                *target = *to;
            }
            What::Tempo { from, to, .. } => {
                Self::check(
                    result,
                    same(self.file.tempo, *from),
                    format!(
                        "the tempo was {from} BPM, not {} BPM as the log has it since change {}",
                        self.file.tempo, self.tempo_since
                    ),
                );
                self.file.tempo = *to;
                self.tempo_since = seq;
            }
            _ => {
                result.unknown = Some(crate::change::describe(what));
            }
        }
    }
    fn forward_cables(&mut self, what: &What, seq: u64, result: &mut Applied) {
        match what {
            What::Patch { cable, replaced } => {
                if !self.ends(cable) {
                    result.unknown = Some(format!(
                        "change {seq} patches cable {} whose end is missing",
                        cable.id
                    ));
                    return;
                }
                let old = self.file.cables.iter().find(|c| c.to == cable.to);
                Self::check(
                    result,
                    old.map(|c| c.id) == replaced.as_ref().map(|c| c.id)
                        && !self.file.cables.iter().any(|c| c.id == cable.id),
                    format!(
                        "the input {} did not hold the cable change {seq} says it replaced",
                        cable.to
                    ),
                );
                self.plug(cable);
            }
            What::Unpatch { cable, replugged } => {
                Self::check(
                    result,
                    self.file.cables.iter().any(|c| cable_same(c, cable)),
                    format!(
                        "cable {} was not as change {seq} says it unplugged",
                        cable.id
                    ),
                );
                self.file.cables.retain(|c| c.id != cable.id);
                if let Some(c) = replugged {
                    self.plug(c);
                }
            }
            _ => {
                result.unknown = Some(crate::change::describe(what));
            }
        }
    }
    fn forward_modules(&mut self, what: &What, seq: u64, result: &mut Applied) {
        match what {
            What::Add { module } => {
                Self::check(
                    result,
                    !self.file.modules.iter().any(|m| m.id == module.id),
                    format!("{} was present before change {seq} added it", module.id),
                );
                self.add(module);
            }
            What::Remove {
                module,
                cables,
                replugged,
                ..
            } => {
                let held = self
                    .file
                    .cables
                    .iter()
                    .filter(|c| at_module(c, &module.id))
                    .collect::<Vec<_>>();
                Self::check(
                    result,
                    self.file.modules.iter().any(|m| module_same(m, module))
                        && held.len() == cables.len()
                        && held
                            .iter()
                            .all(|c| cables.iter().any(|other| cable_same(c, other))),
                    format!(
                        "{} or its cables differed before change {seq} removed it",
                        module.id
                    ),
                );
                self.remove(&module.id);
                for cable in replugged {
                    self.plug(cable);
                }
            }
            What::Restore {
                module,
                cables,
                replaced,
                ..
            } => {
                Self::check(
                    result,
                    !self.file.modules.iter().any(|m| m.id == module.id),
                    format!("{} was present before change {seq} restored it", module.id),
                );
                self.add(module);
                self.file
                    .cables
                    .retain(|c| !replaced.iter().any(|r| r.id == c.id));
                for cable in cables {
                    if self.ends(cable) {
                        self.plug(cable);
                    } else {
                        result.unknown = Some(format!(
                            "change {seq} restores cable {} whose end is missing",
                            cable.id
                        ));
                    }
                }
                if let Some(index) = self
                    .file
                    .retired_speech
                    .iter()
                    .rposition(|w| w.module == module.id)
                {
                    let retired = self.file.retired_speech.remove(index);
                    self.file.speech.insert(module.id.clone(), retired.words);
                }
            }
            _ => {
                result.unknown = Some(crate::change::describe(what));
            }
        }
    }
    pub fn undo(&mut self, what: &What) -> bool {
        match what {
            What::Turn {
                module,
                knob,
                from,
                to,
                ..
            } => {
                match self
                    .file
                    .modules
                    .iter_mut()
                    .find(|m| m.id == *module)
                    .and_then(|m| m.knobs.get_mut(knob))
                {
                    Some(value) if same(*value, *to) => {
                        *value = *from;
                        true
                    }
                    _ => false,
                }
            }
            What::Tempo { from, to, .. } => {
                if !same(self.file.tempo, *to) {
                    return false;
                }
                self.file.tempo = *from;
                true
            }
            What::Patch { cable, replaced } => {
                if !self.file.cables.iter().any(|c| cable_same(c, cable)) {
                    return false;
                }
                self.file.cables.retain(|c| c.id != cable.id);
                if let Some(c) = replaced {
                    self.plug(c);
                }
                true
            }
            What::Unpatch { cable, replugged } => {
                if self.file.cables.iter().any(|c| c.id == cable.id) {
                    return false;
                }
                if let Some(c) = replugged {
                    if !self.file.cables.iter().any(|held| cable_same(held, c)) {
                        return false;
                    }
                    self.file.cables.retain(|held| held.id != c.id);
                }
                self.plug(cable);
                true
            }
            What::Add { .. } | What::Remove { .. } | What::Restore { .. } => self.undo_module(what),
            What::Chain { changes, .. } => changes.iter().rev().all(|c| self.undo(c)),
            What::Record { .. } | What::Speak { .. } | What::Abandoned { .. } => true,
            _ => false,
        }
    }
    fn undo_module(&mut self, what: &What) -> bool {
        match what {
            What::Add { module } => {
                if !self.file.modules.iter().any(|m| module_same(m, module))
                    || self.file.cables.iter().any(|c| at_module(c, &module.id))
                {
                    return false;
                }
                self.remove(&module.id);
                true
            }
            What::Remove {
                module,
                cables,
                replugged,
                ..
            } => {
                if self.file.modules.iter().any(|m| m.id == module.id)
                    || cables
                        .iter()
                        .any(|c| self.file.cables.iter().any(|held| held.id == c.id))
                {
                    return false;
                }
                self.file
                    .cables
                    .retain(|held| !replugged.iter().any(|c| c.id == held.id));
                self.add(module);
                for c in cables {
                    self.plug(c);
                }
                true
            }
            What::Restore {
                module,
                cables,
                replaced,
                ..
            } => {
                if !self.file.modules.iter().any(|m| module_same(m, module))
                    || !cables
                        .iter()
                        .all(|c| self.file.cables.iter().any(|held| cable_same(c, held)))
                {
                    return false;
                }
                self.remove(&module.id);
                for c in replaced {
                    self.plug(c);
                }
                true
            }
            _ => false,
        }
    }
}

fn at_module(cable: &CableRecord, id: &str) -> bool {
    [&cable.from, &cable.to]
        .iter()
        .any(|end| end.split_once('.').is_some_and(|(module, _)| module == id))
}
