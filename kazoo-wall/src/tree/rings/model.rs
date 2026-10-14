use std::collections::{BTreeMap, BTreeSet};

use crate::migrate;
use crate::patch::{Patch, PatchFile, RetiredWords, split_id};
use crate::protocol::{CableRecord, Change, ModuleRecord, What};
use crate::tree::{Birth, Files, TreeError, to_files};

pub(super) const fn same(a: f64, b: f64) -> bool {
    // Knob values are held to f32 ranges: the narrowing is exact enough.
    a.is_finite() && b.is_finite() && (a as f32).to_bits() == (b as f32).to_bits()
}

/// The default today's catalogue gives knob `knob` of kind `kind`: the
/// value a file without that knob loads with.
fn default_of(kind: &str, knob: &str) -> Option<f64> {
    let spec = crate::catalogue::Kind::from_name(kind)?.spec();
    spec.knob_index(knob)
        .map(|index| f64::from(spec.knobs[index].default))
}

/// The knobs of two records of kind `kind` that would not load the same:
/// a knob only one has differs unless its value is the one the other
/// loads with (a file written by a build without that knob).
fn knob_differences<'a>(
    kind: &'a str,
    a: &'a BTreeMap<String, f64>,
    b: &'a BTreeMap<String, f64>,
) -> impl Iterator<Item = &'a String> {
    a.keys()
        .chain(b.keys().filter(|key| !a.contains_key(*key)))
        .filter(move |key| match (a.get(*key), b.get(*key)) {
            (Some(x), Some(y)) => !same(*x, *y),
            (Some(x), None) | (None, Some(x)) => {
                !default_of(kind, key).is_some_and(|default| same(*x, default))
            }
            (None, None) => false,
        })
}

fn module_same(a: &ModuleRecord, b: &ModuleRecord) -> bool {
    a.id == b.id
        && a.kind == b.kind
        && a.name == b.name
        && knob_differences(&a.kind, &a.knobs, &b.knobs)
            .next()
            .is_none()
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
            None => out.push(format!("{} is not in it", module.id)),
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
                for key in knob_differences(&module.kind, &module.knobs, &other.knobs) {
                    out.push(format!("{}.{key} differs", module.id));
                }
            }
        }
    }
    for module in &b.modules {
        if !a.modules.iter().any(|m| m.id == module.id) {
            out.push(format!("{} is in it as well", module.id));
        }
    }
    for cable in &a.cables {
        if !b.cables.iter().any(|c| cable_same(c, cable)) {
            out.push(format!("cable {} differs", cable.id));
        }
    }
    for cable in &b.cables {
        if !a.cables.iter().any(|c| c.id == cable.id) {
            out.push(format!("cable {} is in it as well", cable.id));
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

/// How a module the import gives a found mark came onto the model.
#[derive(Clone, Copy)]
pub(super) enum Found {
    Start,
    Load,
    /// A file adopted after a gap, a settle or a migration.
    File,
}

pub(super) struct Model {
    pub file: PatchFile,
    pub known: bool,
    pub births: BTreeMap<(u64, String), Birth>,
    pub found_start: usize,
    pub born: usize,
    pub kept_load: usize,
    pub found_load: usize,
    pub found_file: usize,
    /// What this build leaves out of the model's walls: `id.knob` or `id`.
    pub omitted: BTreeSet<String>,
    /// The change that last set each knob (`id.knob`), module (`id`),
    /// input (`in id.jack`) and the tempo (`tempo`), since `base`.
    since: BTreeMap<String, u64>,
    /// The change the model was last taken from a file after.
    base: u64,
}

pub(super) struct Applied {
    pub doubt: Option<String>,
    pub unknown: Option<String>,
}

impl Model {
    pub fn new(file: PatchFile, known: bool) -> Result<Self, TreeError> {
        let mut model = Self {
            base: file.last_seq,
            file,
            known,
            births: BTreeMap::new(),
            found_start: 0,
            born: 0,
            kept_load: 0,
            found_load: 0,
            found_file: 0,
            omitted: BTreeSet::new(),
            since: BTreeMap::new(),
        };
        model.adopt_births(Found::Start)?;
        model.found_start = model
            .file
            .modules
            .iter()
            .filter(|m| m.birth.as_ref().is_some_and(|b| b.starts_with("found ")))
            .count();
        Ok(model)
    }

    fn adopt_births(&mut self, how: Found) -> Result<(), TreeError> {
        for module in &mut self.file.modules {
            let key = (self.file.wall, module.id.clone());
            if matches!(how, Found::Load) && self.births.contains_key(&key) {
                self.kept_load += 1;
            }
            let birth = if let Some(birth) = &module.birth {
                birth.parse()?
            } else if let Some(birth) = self.births.get(&key) {
                birth.clone()
            } else {
                match how {
                    Found::Start => self.found_start += 1,
                    Found::Load => self.found_load += 1,
                    Found::File => self.found_file += 1,
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

    /// Take the wall from `file`, as the wall after change `seq`: a load
    /// (`how` is `Load`) makes it wall `seq`.
    pub fn adopt(&mut self, file: &PatchFile, seq: u64, how: Found) -> Result<(), TreeError> {
        self.file = file.clone();
        self.adopt_births(how)?;
        if matches!(how, Found::Load) {
            self.file.wall = seq;
            for module in &self.file.modules {
                if let Some(birth) = &module.birth {
                    self.births.insert((seq, module.id.clone()), birth.parse()?);
                }
            }
        }
        self.file.last_seq = seq;
        self.known = true;
        self.since.clear();
        self.base = seq;
        Ok(())
    }

    /// The model as today's wall loads it, births marked, and the saved
    /// form it was loaded from (migrated while the model is version 1).
    fn patch(&self) -> Result<(Patch, PatchFile), TreeError> {
        let mut view = self.file.clone();
        migrate::migrate(&mut view).map_err(TreeError::Git)?;
        view.seed = self.file.seed;
        let (mut patch, _) = Patch::load(&view).map_err(TreeError::Git)?;
        for module in &view.modules {
            if let Some(birth) = &module.birth {
                if !patch.mark_birth(&module.id, birth.parse()?)
                    && patch.module(&module.id).is_some()
                {
                    return Err(TreeError::Git(format!(
                        "birth for {} was not marked",
                        module.id
                    )));
                }
            }
        }
        Ok((patch, view))
    }

    /// The tree's files for the model, after `change` moved the
    /// fingerprints; the model keeps the rows, groups, fingerprints and
    /// retired words the wall makes of it.
    pub fn files(&mut self, change: Option<&Change>) -> Result<Files, TreeError> {
        let (mut patch, saved) = self.patch()?;
        if let Some(change) = change {
            let mut dye = std::mem::take(&mut patch.dye);
            dye.touch(&change.seat, &change.what, &patch);
            patch.dye = dye;
        }
        let view = patch.to_file();
        for module in &saved.modules {
            match view.modules.iter().find(|m| m.id == module.id) {
                None => {
                    self.omitted.insert(module.id.clone());
                }
                Some(loaded) => {
                    for knob in module
                        .knobs
                        .keys()
                        .filter(|k| !loaded.knobs.contains_key(*k))
                    {
                        self.omitted.insert(format!("{}.{knob}", module.id));
                    }
                }
            }
        }
        self.file.dye.clone_from(&view.dye);
        self.file.rack.clone_from(&view.rack);
        self.file.groups.clone_from(&view.groups);
        self.file.retired_speech.clone_from(&view.retired_speech);
        to_files(&view)
    }

    /// Give each module `change` adds its birth: the one its logged line
    /// carries, or KZ-T01's born mark with the logged `line`'s hash as salt.
    pub fn stamp(&mut self, change: &Change, salt: u64) -> Result<(), TreeError> {
        if crate::tree::has_add(&change.what) {
            // `Patch::fingerprint_word` of the wall with the newborns on it
            // and not yet marked, as the daemon stamps a live change:
            // `Patch::load` marks every module, so the newborns' marks are
            // taken off the file form it hashes.
            let mut print_file = self.patch()?.0.to_file();
            let mut unmarked = change.what.clone();
            for (id, _) in crate::tree::stamp_births(&mut unmarked, change.seq, &change.at, 0, 0) {
                if !self.births.contains_key(&(self.file.wall, id.clone())) {
                    if let Some(module) = print_file.modules.iter_mut().find(|m| m.id == id) {
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
            for (id, birth) in
                crate::tree::stamp_births(&mut what, change.seq, &change.at, print, salt)
            {
                let birth = self
                    .births
                    .entry((self.file.wall, id.clone()))
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

    /// Apply one change at the save form's level, checking every
    /// before-value it carries.
    pub fn apply(&mut self, change: &Change) -> Applied {
        let mut result = Applied {
            doubt: None,
            unknown: None,
        };
        self.forward(&change.what, change, &mut result);
        self.file.last_seq = change.seq;
        if result.unknown.is_some() {
            self.known = false;
        }
        result
    }

    /// The change that last set the first of `keys` the log has set
    /// since the model was last taken from a file, or that change.
    fn since(&self, keys: &[&str]) -> u64 {
        keys.iter()
            .find_map(|key| self.since.get(*key))
            .copied()
            .unwrap_or(self.base)
    }

    fn check(result: &mut Applied, ok: bool, reason: impl FnOnce() -> String) {
        if !ok && result.doubt.is_none() {
            result.doubt = Some(reason());
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

    /// Note that `change` set `key`, or, for a module, set it afresh.
    fn set(&mut self, key: String, change: &Change) {
        if !key.contains('.') {
            self.since.retain(|held, _| {
                held.trim_start_matches("in ").split('.').next() != Some(key.as_str())
            });
        }
        self.since.insert(key, change.seq);
    }

    fn forward(&mut self, what: &What, change: &Change, result: &mut Applied) {
        let seq = change.seq;
        match what {
            What::Turn { .. } | What::Tempo { .. } => {
                self.forward_value(what, change, result);
            }
            What::Patch { cable, replaced } => {
                if !self.ends(cable) {
                    result.unknown = Some(format!(
                        "change {seq} patches cable {} whose end is missing",
                        cable.id
                    ));
                    return;
                }
                let input = format!("in {}", cable.to);
                let held = self
                    .file
                    .cables
                    .iter()
                    .find(|c| c.to == cable.to)
                    .map(|c| c.id);
                let taken = self.file.cables.iter().any(|c| c.id == cable.id);
                let since = self.since(&[input.as_str()]);
                Self::check(
                    result,
                    held == replaced.as_ref().map(|c| c.id) && !taken,
                    || {
                        format!(
                            "{} held {}, not {} as change {seq} has it, and the log has it so since change {since}",
                            cable.to,
                            held.map_or_else(|| "nothing".into(), |id| format!("cable {id}")),
                            replaced
                                .as_ref()
                                .map_or_else(|| "nothing".into(), |c| format!("cable {}", c.id)),
                        )
                    },
                );
                self.plug(cable);
                self.set(input, change);
            }
            What::Unpatch { cable, replugged } => {
                let input = format!("in {}", cable.to);
                let since = self.since(&[input.as_str()]);
                Self::check(
                    result,
                    self.file.cables.iter().any(|c| cable_same(c, cable)),
                    || {
                        format!(
                            "cable {} from {} to {} was not in as change {seq} says it unplugged, and the log has it so since change {since}",
                            cable.id, cable.from, cable.to
                        )
                    },
                );
                self.file.cables.retain(|c| c.id != cable.id);
                if let Some(c) = replugged {
                    self.plug(c);
                }
                self.set(input, change);
            }
            What::Add { .. } | What::Remove { .. } | What::Restore { .. } => {
                self.forward_module(what, change, result);
            }
            What::Batch { changes, .. } | What::Chain { changes, .. } => {
                for inner in changes {
                    self.forward(inner, change, result);
                    if result.unknown.is_some() {
                        break;
                    }
                }
            }
            What::Record { .. } | What::Speak { .. } | What::Abandoned { .. } => {}
            // Every other op, and any op added later: the model cannot say.
            _ => result.unknown = Some(change.summary.clone()),
        }
    }

    fn forward_value(&mut self, what: &What, change: &Change, result: &mut Applied) {
        let seq = change.seq;
        match what {
            What::Turn {
                module,
                knob,
                from,
                to,
                ..
            } => {
                let key = format!("{module}.{knob}");
                let since = self.since(&[key.as_str(), module.as_str()]);
                // A knob a file from an older build lacks loads as its default.
                let Some(target) = self
                    .file
                    .modules
                    .iter_mut()
                    .find(|m| m.id == *module)
                    .and_then(|m| match default_of(&m.kind, knob) {
                        Some(default) => Some(m.knobs.entry(knob.clone()).or_insert(default)),
                        None => m.knobs.get_mut(knob),
                    })
                else {
                    result.unknown = Some(format!(
                        "change {seq} turns {module}.{knob}, which the wall the log makes does not have"
                    ));
                    return;
                };
                let held = *target;
                *target = *to;
                Self::check(result, same(held, *from), || {
                    format!("{key} was {from}, not {held} as the log has it since change {since}")
                });
                self.set(key, change);
            }
            What::Tempo { from, to, .. } => {
                let held = self.file.tempo;
                let since = self.since(&["tempo"]);
                Self::check(result, same(held, *from), || {
                    format!(
                        "the tempo was {from} BPM, not {held} BPM as the log has it since change {since}"
                    )
                });
                self.file.tempo = *to;
                self.set("tempo".into(), change);
            }
            _ => result.unknown = Some(change.summary.clone()),
        }
    }

    fn forward_module(&mut self, what: &What, change: &Change, result: &mut Applied) {
        let seq = change.seq;
        let (module, cables) = match what {
            What::Add { module } => (module, &[][..]),
            What::Remove { module, cables, .. } | What::Restore { module, cables, .. } => {
                (module, &cables[..])
            }
            _ => return,
        };
        let id = &module.id;
        let since = self.since(&[id.as_str()]);
        let on = self.file.modules.iter().any(|m| m.id == *id);
        if let What::Remove { replugged, .. } = what {
            let held = self
                .file
                .cables
                .iter()
                .filter(|c| at_module(c, id))
                .collect::<Vec<_>>();
            let same_cables = held.len() == cables.len()
                && held
                    .iter()
                    .all(|c| cables.iter().any(|other| cable_same(c, other)));
            let same_module = self.file.modules.iter().any(|m| module_same(m, module));
            Self::check(result, same_module && same_cables, || {
                format!(
                    "{id} or its cables were not as change {seq} says it removed them, and the log has them so since change {since}"
                )
            });
            self.remove(id);
            for cable in replugged {
                self.plug(cable);
            }
            self.set(id.clone(), change);
            return;
        }
        Self::check(result, !on, || {
            format!(
                "{id} was on the wall when change {seq} brought it in, and the log has it so since change {since}"
            )
        });
        self.add(module);
        self.set(id.clone(), change);
        let What::Restore { replaced, .. } = what else {
            return;
        };
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
            .rposition(|w| w.module == *id)
        {
            let retired = self.file.retired_speech.remove(index);
            self.file.speech.insert(id.clone(), retired.words);
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
                let ok = same(self.file.tempo, *to);
                if ok {
                    self.file.tempo = *from;
                }
                ok
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
            What::Batch { changes, .. } | What::Chain { changes, .. } => {
                changes.iter().rev().all(|c| self.undo(c))
            }
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
