//! The patch: every module with its knobs, every cable, and the numbering
//! that makes ids unique for the patch's lifetime.
//!
//! This is the control side's model of the wall, and what is saved to
//! `patch.json`. Every operation checks its request, changes the model, and
//! returns a [`What`] describing exactly what changed — enough to undo it.
//! The daemon mirrors each change into the engine.
//!
//! The patch also keeps the rack's rows: where each module hangs, shared by
//! every console like a real case. Every module is in exactly one row, and
//! no row is empty. Moving a module ([`Patch::arrange`]) is not a change to
//! the sound and returns no [`What`].

mod chains;

pub use chains::{Chain, ChainCable, ChainModule};

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::catalogue::{Jack, Kind, KindSpec};
use crate::fingerprints::Dye;
use crate::protocol::{
    CableRecord, ErrorCode, ModuleRecord, Place, WallError, What, valid_name, widen,
};
use crate::tree::Birth;

/// Glide used when a turn names none, in beats.
pub const DEFAULT_GLIDE_BEATS: f64 = 2.0;

/// Longest glide, in beats.
pub const MAX_GLIDE_BEATS: f64 = 64.0;

/// Version of the `patch.json` format.
pub const FILE_VERSION: u32 = 2;

/// Removed speakers whose words are kept, so undoing a removal brings a
/// speaker back with them; beyond this the first removed is let go.
pub const MAX_RETIRED_WORDS: usize = 256;

/// A module on the wall.
#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    /// Id: kind and number.
    pub id: String,
    /// Kind.
    pub kind: Kind,
    /// Display name.
    pub name: Option<String>,
    /// Knob targets, in catalogue order.
    pub knobs: Vec<f32>,
    /// The module's lasting birth mark.
    pub birth: Option<Birth>,
}

impl Module {
    /// The module as a record.
    #[must_use]
    pub fn record(&self) -> ModuleRecord {
        ModuleRecord {
            id: self.id.clone(),
            kind: self.kind.name().to_string(),
            name: self.name.clone(),
            birth: self.birth.as_ref().map(ToString::to_string),
            knobs: self
                .kind
                .spec()
                .knobs
                .iter()
                .zip(&self.knobs)
                // Targets are held to the knob's range as they are set, so
                // always finite; held again here, so none could travel as a
                // value outside it (`widen` sends a non-number as 0).
                .map(|(spec, value)| (spec.name.to_string(), widen(spec.clamp(*value))))
                .collect(),
        }
    }
}

/// A cable on the wall.
#[derive(Debug, Clone, PartialEq)]
pub struct Cable {
    /// Number, never reused.
    pub id: u32,
    /// Source module id.
    pub from_module: String,
    /// Source output index.
    pub from_port: usize,
    /// Destination module id.
    pub to_module: String,
    /// Destination input or knob jack.
    pub to_jack: Jack,
    /// Attenuverter, -1 to 1.
    pub amount: f32,
}

impl Cable {
    /// The source as `module.port`.
    #[must_use]
    pub fn from_label(&self, kind: Kind) -> String {
        let port = kind
            .spec()
            .outputs
            .get(self.from_port)
            .map_or("?", |port| port.name);
        format!("{}.{port}", self.from_module)
    }

    /// The destination as `module.jack`.
    #[must_use]
    pub fn to_label(&self, kind: Kind) -> String {
        let spec = kind.spec();
        let name = match self.to_jack {
            Jack::Input(index) => spec.inputs.get(index).map_or("?", |port| port.name),
            Jack::Knob(index) => spec.knobs.get(index).map_or("?", |knob| knob.name),
        };
        format!("{}.{name}", self.to_module)
    }
}

/// The saved form of a patch, as in `patch.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PatchFile {
    /// Format version.
    pub version: u32,
    /// The wall's own tempo.
    pub tempo: f64,
    /// The latest change's sequence number.
    pub last_seq: u64,
    /// Which wall this is: the number of the change that put it up (see
    /// [`crate::protocol::Snapshot::wall`]); 0 for a wall never replaced,
    /// and in patches saved before walls were replaced.
    #[serde(default)]
    pub wall: u64,
    /// The seed from which every module derives its random streams.
    #[serde(default)]
    pub seed: u64,
    /// The next number for each kind's ids.
    pub next_ids: BTreeMap<String, u32>,
    /// The next cable number.
    pub next_cable: u32,
    /// Every module, in the order added.
    pub modules: Vec<ModuleRecord>,
    /// Every cable.
    pub cables: Vec<CableRecord>,
    /// The dye each seat has put into each module (see
    /// [`crate::fingerprints`]), by module id then seat. Absent in patches
    /// saved before fingerprints.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dye: BTreeMap<String, BTreeMap<String, f64>>,
    /// What each `speak` module was last given to say, by module id. Kept
    /// here, and never shown to seats: a seat's free text reaches nobody
    /// else.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub speech: BTreeMap<String, Words>,
    /// The words of `speak` modules taken away, oldest removal first, so
    /// undoing a removal brings them back (ids are never given out again).
    /// Kept as privately as [`Self::speech`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retired_speech: Vec<RetiredWords>,
    /// The rack's rows, top to bottom, each its module ids left to right.
    /// Absent in patches saved before the rack had rows (they are laid out
    /// afresh, a row for each group of modules).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rack: Vec<Vec<String>>,
    /// The wall's groups: each group's name and its module ids, in the
    /// order they joined. Absent in patches saved before groups.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub groups: BTreeMap<String, Vec<String>>,
}

/// A removed speaker's words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetiredWords {
    /// The removed module's id.
    pub module: String,
    /// What it was last given to say.
    pub words: Words,
}

/// Words a `speak` module was given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Words {
    /// The text.
    pub text: String,
    /// The `say` voice, if one was named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
}

/// The patch.
#[derive(Debug, Clone, PartialEq)]
pub struct Patch {
    modules: Vec<Module>,
    /// Each module's place in `modules`, by id, so a wall of any size finds
    /// a module without a search.
    places: BTreeMap<String, usize>,
    /// Every cable, in number order (so a cable is found by a binary
    /// search).
    cables: Vec<Cable>,
    /// The next number for each kind's ids, by kind name: kept even for
    /// kinds the wall no longer has, so their ids are never reused.
    next_ids: BTreeMap<String, u32>,
    next_cable: u32,
    /// The wall's own tempo, as saved.
    pub tempo: f64,
    /// The latest change's sequence number, as saved.
    pub last_seq: u64,
    /// Which wall this is (see [`PatchFile::wall`]).
    pub wall: u64,
    /// The saved seed for every module's random streams.
    pub seed: u64,
    /// Who has touched which module, as saved.
    pub dye: Dye,
    /// What each `speak` module says, by module id.
    pub speech: BTreeMap<String, Words>,
    /// Removed speakers' words, oldest removal first.
    retired_speech: VecDeque<RetiredWords>,
    /// The rack's rows: every module in exactly one, none empty.
    rows: Vec<Vec<String>>,
    /// The groups: each one's module ids, in the order they joined; none
    /// empty, every id on the wall.
    groups: BTreeMap<String, Vec<String>>,
}

fn error(code: ErrorCode, message: impl Into<String>) -> WallError {
    WallError::new(code, message)
}

/// Why `what` can never be undone, whatever wall is up, if it cannot.
///
/// That is a note the daemon made, a recording, words, or a change by a
/// newer wall. Checked before anything about walls, so the reason given
/// is the true one.
#[must_use]
pub fn never_undone(what: &What) -> Option<WallError> {
    let refused = |message: String| Some(error(ErrorCode::NotAllowed, message));
    match what {
        What::Abandoned { change } => refused(format!(
            "this says change {change} did not happen; there is nothing to undo"
        )),
        What::Resumed { .. } => {
            refused("the wall starting again from patch.json cannot be undone".to_string())
        }
        What::SetAside { moved_to, .. } => refused(format!(
            "setting patch.json aside cannot be undone; the file is still there as {moved_to}"
        )),
        What::Migrate { .. } => {
            refused("bringing the patch up to date cannot be undone".to_string())
        }
        What::Record { on: true, .. } => {
            refused("a recording cannot be undone; stop it with record".to_string())
        }
        What::Record { on: false, .. } => {
            refused("a recording cannot be undone; start a new one with record".to_string())
        }
        What::Speak { module, .. } => refused(format!(
            "words cannot be taken back; give {module} new ones"
        )),
        What::Unknown => refused(
            "that change was made by a newer wall and cannot be undone by this one".to_string(),
        ),
        What::Chain { changes, .. } => changes.iter().find_map(never_undone),
        What::Turn { .. }
        | What::Patch { .. }
        | What::Unpatch { .. }
        | What::Add { .. }
        | What::Remove { .. }
        | What::Restore { .. }
        | What::Tempo { .. }
        | What::Load { .. } => None,
    }
}

/// Longest module id.
const MAX_ID: usize = 40;

/// The numbering of a file (`saved`) this wall keeps, within the bound its
/// file is held to (see `store::MAX_KINDS`): every kind this build has,
/// then kinds it does not have (a kind since renamed or taken out, whose
/// numbering is kept so its ids are never given again should it come
/// back) while they number fewer than `store::MAX_KINDS` in all and look
/// like a kind's name (lower-case letters, digits, `-` and `_`, at most
/// `store::MAX_CATALOGUE_NAME` bytes). The rest is dropped and said in
/// `notes`: nothing but a file made by hand holds it.
fn numbering_in(saved: &BTreeMap<String, u32>, notes: &mut Vec<String>) -> BTreeMap<String, u32> {
    let mut kept: BTreeMap<String, u32> = saved
        .iter()
        .filter(|(name, _)| Kind::from_name(name).is_some())
        .map(|(name, next)| (name.clone(), (*next).max(1)))
        .collect();
    for (name, next) in saved {
        if kept.contains_key(name) {
            continue;
        }
        let looks = !name.is_empty()
            && name.len() <= crate::store::MAX_CATALOGUE_NAME
            && name.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"-_".contains(&byte)
            });
        if looks && kept.len() < crate::store::MAX_KINDS {
            kept.insert(name.clone(), (*next).max(1));
        } else {
            notes.push(format!(
                "the numbering of '{}', which no kind could have, dropped",
                printable(name)
            ));
        }
    }
    kept
}

/// The most notes [`Patch::load`] gives about one file.
///
/// Each note is short (names go through [`printable`]), so however much junk a file
/// holds, what is said about it fits in one answer with room to spare.
pub const MAX_LOAD_NOTES: usize = 32;

/// Keep the first [`MAX_LOAD_NOTES`] `notes`, the last of them counting
/// those left unsaid.
fn cap_notes(notes: &mut Vec<String>) {
    if notes.len() > MAX_LOAD_NOTES {
        let unsaid = notes.len() - (MAX_LOAD_NOTES - 1);
        notes.truncate(MAX_LOAD_NOTES - 1);
        notes.push(format!("and {unsaid} more like these"));
    }
}

/// Whether `words` are within what a speaker may be given: its text and
/// voice no longer than `speak` takes (so no file, whatever it holds,
/// brings in more than the wall could have been given).
fn words_fit(words: &Words) -> bool {
    words.text.chars().count() <= kazoo_speech::tts::MAX_TEXT_CHARS
        && words
            .voice
            .as_ref()
            .is_none_or(|voice| voice.chars().count() <= kazoo_speech::tts::MAX_VOICE_CHARS)
}

/// `bytes` as a short string that changes whenever they do.
///
/// That is their 64-bit FNV-1a hash and their length, e.g.
/// `5f3c0e2a91b4d7e8-48213`: enough to tell, beyond any accidental doubt,
/// that a file still holds the very bytes it held (the saves live in Tom's
/// own private directory, so nobody is forging one). Empty bytes give the
/// empty string, which no file is ever said to match.
#[must_use]
pub fn hash_bytes(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    let hash = crate::chance::fnv1a64(bytes);
    format!("{hash:016x}-{}", bytes.len())
}

/// The id for number `number` of kind `kind`: the kind's name and the
/// number (`vco12`), with `_` between when the name ends in a digit
/// (`sampler12_3`), so every id reads back one way.
#[must_use]
pub fn make_id(kind: &str, number: u32) -> String {
    if kind.ends_with(|c: char| c.is_ascii_digit()) {
        format!("{kind}_{number}")
    } else {
        format!("{kind}{number}")
    }
}

/// The kind name and number an id was made from by [`make_id`].
#[must_use]
pub fn split_id(id: &str) -> Option<(&str, u32)> {
    let number = |text: &str| match text.parse::<u32>() {
        Ok(number) if number > 0 && !text.starts_with('0') => Some(number),
        Ok(_) | Err(_) => None,
    };
    if let Some((kind, digits)) = id.rsplit_once('_') {
        if kind.ends_with(|c: char| c.is_ascii_digit()) {
            return number(digits).map(|n| (kind, n));
        }
    }
    let kind = id.trim_end_matches(|c: char| c.is_ascii_digit());
    if kind.is_empty() || kind.ends_with('_') {
        return None;
    }
    number(&id[kind.len()..]).map(|n| (kind, n))
}

/// The kind an id names, when it was made by [`make_id`] for a kind the
/// wall has. A module's own kind is in the patch; this is for ids alone.
#[must_use]
pub fn kind_of_id(id: &str) -> Option<Kind> {
    split_id(id).and_then(|(kind, _)| Kind::from_name(kind))
}

/// Whether `id` is a well-formed module id: 1 to 40 of `a-z 0-9 _`,
/// starting with a letter. (Ids of modules migrated from an older kind keep
/// that kind's name.)
#[must_use]
pub fn valid_id(id: &str) -> bool {
    (1..=MAX_ID).contains(&id.len())
        && id.starts_with(|c: char| c.is_ascii_lowercase())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Most names an error message lists: a wall has no fixed size, and a
/// message naming thousands of modules helps nobody.
const LISTED: usize = 32;

/// `names` joined with commas, for error messages: the first [`LISTED`],
/// and how many more there are.
fn list(names: impl IntoIterator<Item = impl AsRef<str>>) -> String {
    let mut out = String::new();
    let mut more = 0_usize;
    for (at, name) in names.into_iter().enumerate() {
        if at >= LISTED {
            more += 1;
            continue;
        }
        if !out.is_empty() {
            out.push_str(", ");
        }
        out.push_str(name.as_ref());
    }
    if out.is_empty() {
        out.push_str("none");
    }
    if more > 0 {
        out = format!("{out} and {more} more");
    }
    out
}

impl Patch {
    /// An empty wall at `tempo`.
    #[must_use]
    pub const fn empty(tempo: f64) -> Self {
        Self {
            modules: Vec::new(),
            places: BTreeMap::new(),
            cables: Vec::new(),
            next_ids: BTreeMap::new(),
            next_cable: 1,
            tempo,
            last_seq: 0,
            wall: 0,
            seed: 0,
            dye: Dye::new(),
            speech: BTreeMap::new(),
            retired_speech: VecDeque::new(),
            rows: Vec::new(),
            groups: BTreeMap::new(),
        }
    }

    /// The rack's rows, top to bottom, each its module ids left to right.
    #[must_use]
    pub fn rows(&self) -> &[Vec<String>] {
        &self.rows
    }

    /// Every module, in the order added.
    #[must_use]
    pub fn modules(&self) -> &[Module] {
        &self.modules
    }

    /// Every cable, in number order.
    #[must_use]
    pub fn cables(&self) -> &[Cable] {
        &self.cables
    }

    /// The module with id `id`.
    #[must_use]
    pub fn module(&self, id: &str) -> Option<&Module> {
        self.places.get(id).and_then(|&at| self.modules.get(at))
    }

    /// The module with id `id`, to change.
    fn module_mut(&mut self, id: &str) -> Option<&mut Module> {
        self.places.get(id).and_then(|&at| self.modules.get_mut(at))
    }

    /// Put `module` on the wall, after the rest.
    fn push_module(&mut self, module: Module) {
        self.places.insert(module.id.clone(), self.modules.len());
        self.modules.push(module);
    }

    /// Where cable `id` is in the list, or where it would go.
    fn cable_at(&self, id: u32) -> Result<usize, usize> {
        self.cables.binary_search_by_key(&id, |cable| cable.id)
    }

    /// Put module `id`'s knob targets back to `knobs`, exactly as they
    /// were (taken from [`Module::knobs`] before a turn that was then not
    /// made). Nothing happens if the module has gone or the knobs do not
    /// fit it.
    pub fn put_knobs_back(&mut self, id: &str, knobs: &[f32]) {
        if let Some(module) = self.module_mut(id) {
            if module.knobs.len() == knobs.len() {
                module.knobs.copy_from_slice(knobs);
            }
        }
    }

    /// The module with id `id`, or an `unknown_module` error listing the
    /// ids there are.
    ///
    /// # Errors
    ///
    /// `unknown_module`.
    pub fn find(&self, id: &str) -> Result<&Module, WallError> {
        self.module(id).ok_or_else(|| {
            error(
                ErrorCode::UnknownModule,
                format!(
                    "no module '{}' on the wall; modules: {}",
                    printable(id),
                    list(self.modules.iter().map(|m| m.id.as_str()))
                ),
            )
        })
    }

    /// A cable as a record.
    #[must_use]
    pub fn cable_record(&self, cable: &Cable) -> CableRecord {
        let kind_of = |id: &str| self.module(id).map(|m| m.kind);
        CableRecord {
            id: cable.id,
            from: kind_of(&cable.from_module).map_or_else(
                || format!("{}.?", cable.from_module),
                |kind| cable.from_label(kind),
            ),
            to: kind_of(&cable.to_module).map_or_else(
                || format!("{}.?", cable.to_module),
                |kind| cable.to_label(kind),
            ),
            // Amounts are held to -1..1 as cables are plugged, so always
            // finite; held again here (`widen` sends a non-number as 0).
            amount: widen(cable.amount.clamp(-1.0, 1.0)),
        }
    }

    /// Every cable as a record.
    #[must_use]
    pub fn cable_records(&self) -> Vec<CableRecord> {
        self.cables.iter().map(|c| self.cable_record(c)).collect()
    }

    // -----------------------------------------------------------------
    // Operations
    // -----------------------------------------------------------------

    /// Turn `module`'s `knob` to `value` over `glide_beats` (default 2).
    ///
    /// # Errors
    ///
    /// `unknown_module`, `unknown_knob`, or `bad_request` for a value or
    /// glide that is not a number.
    pub fn turn(
        &mut self,
        module: &str,
        knob: &str,
        value: f64,
        glide_beats: Option<f64>,
    ) -> Result<What, WallError> {
        if !value.is_finite() {
            return Err(error(ErrorCode::BadRequest, "the value must be a number"));
        }
        let glide = glide_beats.unwrap_or(DEFAULT_GLIDE_BEATS);
        if !glide.is_finite() {
            return Err(error(ErrorCode::BadRequest, "glide_beats must be a number"));
        }
        let glide = glide.clamp(0.0, MAX_GLIDE_BEATS);
        let found = self.find(module)?;
        let spec = found.kind.spec();
        let index = spec.knob_index(knob).ok_or_else(|| {
            error(
                ErrorCode::UnknownKnob,
                format!(
                    "{module} has no knob '{}'; knobs: {}",
                    printable(knob),
                    list(spec.knobs.iter().map(|k| k.name))
                ),
            )
        })?;
        // Values past f32's range clamp like any other.
        let target = spec.knobs[index].clamp(value.clamp(-1.0e30, 1.0e30) as f32);
        let Some(entry) = self.module_mut(module) else {
            return Err(error(
                ErrorCode::UnknownModule,
                format!("no module '{module}'"),
            ));
        };
        let from = entry.knobs[index];
        entry.knobs[index] = target;
        Ok(What::Turn {
            module: module.to_string(),
            knob: spec.knobs[index].name.to_string(),
            // Both held to the knob's range (always finite: `target` was
            // clamped above, `from` when it was set).
            from: widen(spec.knobs[index].clamp(from)),
            to: widen(spec.knobs[index].clamp(target)),
            glide_beats: glide,
        })
    }

    /// Check a [`Self::plug`] of output `from` into jack `to` without
    /// making it, so the daemon can judge the room for it first. Returns
    /// whether it would add a cable: `false` when one in `to` would be
    /// replaced.
    ///
    /// # Errors
    ///
    /// `unknown_module`, `unknown_port`, or `bad_request` for an amount
    /// that is not a number, as [`Self::plug`].
    pub fn check_plug(&self, from: &str, to: &str, amount: Option<f64>) -> Result<bool, WallError> {
        if !amount.unwrap_or(1.0).is_finite() {
            return Err(error(ErrorCode::BadRequest, "the amount must be a number"));
        }
        self.output(from)?;
        let (to_module, to_jack) = self.input(to)?;
        Ok(!self
            .cables
            .iter()
            .any(|cable| cable.to_module == to_module && cable.to_jack == to_jack))
    }

    /// Plug output `from` into jack `to`, replacing any cable already in
    /// it. `id` restores a cable under its old number (when undoing).
    ///
    /// # Errors
    ///
    /// `unknown_module`, `unknown_port`, `bad_request` for an amount that
    /// is not a number, `not_allowed` when `id` is already plugged in.
    /// The patch holds any number of cables: what the machine can render
    /// is the daemon's to judge.
    pub fn plug(
        &mut self,
        from: &str,
        to: &str,
        amount: Option<f64>,
        id: Option<u32>,
    ) -> Result<What, WallError> {
        let amount = amount.unwrap_or(1.0);
        if !amount.is_finite() {
            return Err(error(ErrorCode::BadRequest, "the amount must be a number"));
        }
        let (from_module, from_port) = self.output(from)?;
        let (to_module, to_jack) = self.input(to)?;
        if let Some(id) = id {
            if self.cable_at(id).is_ok() {
                return Err(error(
                    ErrorCode::NotAllowed,
                    format!("cable {id} is already plugged in"),
                ));
            }
        }
        let existing = self
            .cables
            .iter()
            .position(|cable| cable.to_module == to_module && cable.to_jack == to_jack);
        let replaced = existing.map(|index| {
            let old = self.cables.remove(index);
            self.cable_record(&old)
        });
        let id = id.unwrap_or_else(|| {
            let id = self.next_cable;
            self.next_cable += 1;
            id
        });
        self.next_cable = self.next_cable.max(id.saturating_add(1));
        let cable = Cable {
            id,
            from_module,
            from_port,
            to_module,
            to_jack,
            // Held to -1..1, so the narrowing is exact enough.
            amount: amount.clamp(-1.0, 1.0) as f32,
        };
        let record = self.cable_record(&cable);
        // Cables stay in number order, so a cable plugged back by an undo
        // takes its old place in the list.
        let (Ok(at) | Err(at)) = self.cable_at(cable.id);
        self.cables.insert(at, cable);
        Ok(What::Patch {
            cable: record,
            replaced,
        })
    }

    /// Unplug cable `id`, then plug `replug` back in if given and its ends
    /// are still on the wall (when undoing a patch that replaced it).
    ///
    /// # Errors
    ///
    /// `unknown_cable`.
    pub fn unplug(&mut self, id: u32, replug: Option<&CableRecord>) -> Result<What, WallError> {
        let index = self.cable_at(id).map_err(|_| {
            error(
                ErrorCode::UnknownCable,
                format!(
                    "no cable {id}; cables: {}",
                    list(self.cables.iter().map(|c| c.id.to_string()))
                ),
            )
        })?;
        let cable = self.cable_record(&self.cables[index]);
        self.cables.remove(index);
        // A cable whose other end is gone, or that is back already, stays
        // out.
        let replugged = replug
            .and_then(|old| plugged(self.plug(&old.from, &old.to, Some(old.amount), Some(old.id))));
        Ok(What::Unpatch { cable, replugged })
    }

    /// Whether a cable is plugged into jack `to` (false for a jack that is
    /// not on the wall).
    #[must_use]
    pub fn is_plugged(&self, to: &str) -> bool {
        self.input(to).is_ok_and(|(module, jack)| {
            self.cables
                .iter()
                .any(|cable| cable.to_module == module && cable.to_jack == jack)
        })
    }

    /// Unplug whatever is in jack `to`.
    ///
    /// # Errors
    ///
    /// `unknown_module`, `unknown_port`, or `unknown_cable` when nothing is
    /// plugged in there.
    pub fn unplug_jack(&mut self, to: &str) -> Result<What, WallError> {
        let (module, jack) = self.input(to)?;
        let id = self
            .cables
            .iter()
            .find(|cable| cable.to_module == module && cable.to_jack == jack)
            .map(|cable| cable.id)
            .ok_or_else(|| {
                error(
                    ErrorCode::UnknownCable,
                    format!("nothing is plugged into {}", printable(to)),
                )
            })?;
        self.unplug(id, None)
    }

    /// Check an [`Self::add`] of a module of `kind` named `name` without
    /// making it, so the daemon can judge the room for it first. Returns
    /// the kind.
    ///
    /// # Errors
    ///
    /// `unknown_kind` or `bad_name`, as [`Self::add`].
    pub fn check_add(kind: &str, name: Option<&str>) -> Result<Kind, WallError> {
        let found = Kind::from_name(kind).ok_or_else(|| {
            error(
                ErrorCode::UnknownKind,
                format!(
                    "no module kind '{}'; kinds: {}",
                    printable(kind),
                    list(Kind::all().map(Kind::name))
                ),
            )
        })?;
        if let Some(name) = name {
            if !valid_name(name) {
                return Err(bad_name(name));
            }
        }
        Ok(found)
    }

    /// Add a module of `kind`, with its knobs at their defaults, at `place`
    /// on the rack (or at the end of the row holding the newest module of
    /// its group, or on a new bottom row).
    ///
    /// # Errors
    ///
    /// `unknown_kind` or `bad_name`. The patch holds any number of modules:
    /// what the machine can render is the daemon's to judge.
    pub fn add(
        &mut self,
        kind: &str,
        name: Option<&str>,
        place: Option<&Place>,
    ) -> Result<What, WallError> {
        let kind = Self::check_add(kind, name)?;
        let mut number = self.next_ids.get(kind.name()).copied().unwrap_or(1).max(1);
        let mut id = make_id(kind.name(), number);
        // Migrated modules keep ids of other kinds: never take one.
        while self.module(&id).is_some() {
            number = number.saturating_add(1);
            id = make_id(kind.name(), number);
        }
        self.next_ids
            .insert(kind.name().to_string(), number.saturating_add(1));
        let module = Module {
            id,
            kind,
            name: name.map(str::to_string),
            knobs: kind.spec().defaults(),
            birth: None,
        };
        let record = module.record();
        let place = place.cloned().unwrap_or_else(|| self.usual_place(kind));
        put(&mut self.rows, &module.id, &place);
        self.push_module(module);
        Ok(What::Add { module: record })
    }

    /// Take `module` away with its cables, then plug `replug` back in
    /// where their ends are still on the wall and the jack is free (when
    /// undoing a restore).
    ///
    /// # Errors
    ///
    /// `unknown_module`.
    pub fn remove(&mut self, module: &str, replug: &[CableRecord]) -> Result<What, WallError> {
        let record = self.find(module)?.record();
        let place = self.place_of(module);
        take_out(&mut self.rows, module);
        self.rows.retain(|row| !row.is_empty());
        self.leave_groups(&record.id);
        let (gone, kept): (Vec<Cable>, Vec<Cable>) = std::mem::take(&mut self.cables)
            .into_iter()
            .partition(|cable| cable.from_module == module || cable.to_module == module);
        // Records while both ends are still on the wall.
        self.cables = gone;
        let cables = self.cable_records();
        self.cables = kept;
        if let Some(words) = self.speech.remove(module) {
            self.retired_speech.push_back(RetiredWords {
                module: module.to_string(),
                words,
            });
            while self.retired_speech.len() > MAX_RETIRED_WORDS {
                self.retired_speech.pop_front();
            }
        }
        if let Some(at) = self.places.remove(module) {
            self.modules.remove(at);
            // Those after it move up one.
            for later in self.places.values_mut() {
                if *later > at {
                    *later -= 1;
                }
            }
        }
        let mut replugged = Vec::new();
        for old in replug {
            let free = self.input(&old.to).is_ok_and(|(to, jack)| {
                !self
                    .cables
                    .iter()
                    .any(|c| c.to_module == to && c.to_jack == jack)
            });
            if !free {
                continue;
            }
            // A cable whose other end is gone too stays out; the change
            // record shows only what came back.
            replugged.extend(plugged(self.plug(
                &old.from,
                &old.to,
                Some(old.amount),
                Some(old.id),
            )));
        }
        Ok(What::Remove {
            module: record,
            cables,
            replugged,
            place,
        })
    }

    /// Bring back a removed module under its old id, with its knobs, at
    /// `place` on the rack (where it was; without one, where a new module
    /// of its kind would go), and plug its cables back in where the other
    /// end is still on the wall.
    ///
    /// # Errors
    ///
    /// `not_allowed` if the id is on the wall already, or the record's own
    /// problems (`unknown_knob`, `bad_name`).
    pub fn restore(
        &mut self,
        record: &ModuleRecord,
        cables: &[CableRecord],
        place: Option<&Place>,
    ) -> Result<What, WallError> {
        if self.module(&record.id).is_some() {
            return Err(error(
                ErrorCode::NotAllowed,
                format!("{} is on the wall already", record.id),
            ));
        }
        let (mut module, ignored, bad_birth) = module_from_record(record)?;
        for knob in ignored {
            eprintln!(
                "{} has no knob '{}'; its value was dropped",
                module.id,
                printable(&knob)
            );
        }
        if let Some(why) = bad_birth {
            eprintln!(
                "module '{}' had a birth mark that does not read ('{why}'); it is found afresh",
                module.id
            );
        }
        if let Some(birth) = &module.birth {
            if let Some(holder) = self
                .modules
                .iter()
                .find(|existing| existing.birth.as_ref() == Some(birth))
            {
                eprintln!(
                    "module '{}' had the birth mark of {}; it is found afresh",
                    module.id, holder.id
                );
                module.birth = None;
            }
        }
        if module.birth.is_none() {
            module.birth = Some(Birth::Found {
                wall: self.wall,
                seed: self.seed,
                id: module.id.clone(),
            });
        }
        self.count_id(&module.id);
        let restored = module.record();
        if let Some(at) = self
            .retired_speech
            .iter()
            .position(|retired| retired.module == module.id)
        {
            if let Some(retired) = self.retired_speech.remove(at) {
                self.speech.insert(retired.module, retired.words);
            }
        }
        let place = place
            .cloned()
            .unwrap_or_else(|| self.usual_place(module.kind));
        put(&mut self.rows, &module.id, &place);
        let place = self.place_of(&module.id);
        self.push_module(module);
        let mut back = Vec::new();
        let mut replaced = Vec::new();
        let mut skipped = Vec::new();
        for old in cables {
            match self.plug(&old.from, &old.to, Some(old.amount), Some(old.id)) {
                Ok(What::Patch {
                    cable,
                    replaced: displaced,
                }) => {
                    back.push(cable);
                    replaced.extend(displaced);
                }
                Ok(_) | Err(_) => skipped.push(old.clone()),
            }
        }
        Ok(What::Restore {
            module: restored,
            cables: back,
            replaced,
            skipped,
            place,
        })
    }

    // -----------------------------------------------------------------
    // The rack's rows
    // -----------------------------------------------------------------

    /// Move `module` to `place` on the rack. Returns whether the rows
    /// changed (moving a module to where it is changes nothing).
    ///
    /// # Errors
    ///
    /// `unknown_module`.
    pub fn arrange(&mut self, module: &str, place: &Place) -> Result<bool, WallError> {
        let id = self.find(module)?.id.clone();
        if place.before.as_deref() == Some(id.as_str()) {
            return Ok(false);
        }
        let before = self.rows.clone();
        arrange_rows(&mut self.rows, &id, place);
        Ok(self.rows != before)
    }

    // -----------------------------------------------------------------
    // Groups
    // -----------------------------------------------------------------

    /// The groups: each one's module ids, in the order they joined.
    #[must_use]
    pub const fn groups(&self) -> &BTreeMap<String, Vec<String>> {
        &self.groups
    }

    /// Put `modules` (ids or names) in group `name`, or take them out of it
    /// (`join` false). A group is made by its first module and goes with
    /// its last. Returns whether any group changed.
    ///
    /// # Errors
    ///
    /// `bad_name` for a group name that is not a name (or has a space),
    /// `unknown_module` for a module not on the wall; nothing changes.
    pub fn group(&mut self, name: &str, modules: &[String], join: bool) -> Result<bool, WallError> {
        if !valid_group_name(name) {
            return Err(error(
                ErrorCode::BadName,
                format!(
                    "'{}' is not a group name: use 1 to 24 of A-Z a-z 0-9 _ . - (no spaces)",
                    printable(name)
                ),
            ));
        }
        let ids = modules
            .iter()
            .map(|module| self.find(module).map(|found| found.id.clone()))
            .collect::<Result<Vec<String>, WallError>>()?;
        let before = self.groups.get(name).cloned();
        let members = self.groups.entry(name.to_string()).or_default();
        for id in ids {
            if join {
                if !members.contains(&id) {
                    members.push(id);
                }
            } else {
                members.retain(|member| *member != id);
            }
        }
        if members.is_empty() {
            self.groups.remove(name);
        }
        Ok(self.groups.get(name) != before.as_ref())
    }

    /// Take module `id` out of every group (it is leaving the wall).
    fn leave_groups(&mut self, id: &str) {
        for members in self.groups.values_mut() {
            members.retain(|member| member != id);
        }
        self.groups.retain(|_, members| !members.is_empty());
    }

    /// Where module `id` is on the rack: its row, the module after it, and
    /// whether it is alone in its row.
    fn place_of(&self, id: &str) -> Option<Place> {
        self.rows.iter().enumerate().find_map(|(row, ids)| {
            let at = ids.iter().position(|other| other == id)?;
            Some(Place {
                row,
                before: ids.get(at + 1).cloned(),
                own: ids.len() == 1,
            })
        })
    }

    /// Where a new module of `kind` goes when no place is asked for: at the
    /// end of the row holding the newest module of its group on the rack,
    /// or on a new bottom row.
    fn usual_place(&self, kind: Kind) -> Place {
        let group = kind.spec().group();
        let row = self
            .modules
            .iter()
            .rev()
            .filter(|module| module.kind.spec().group() == group)
            .find_map(|module| self.rows.iter().position(|ids| ids.contains(&module.id)));
        Place::end_of(row.unwrap_or(self.rows.len()))
    }

    /// Lay the rack out afresh: a row for each group of modules, in the
    /// groups' order (see [`crate::catalogue::GROUPS`]).
    pub fn regroup(&mut self) {
        self.hang_rows(&[]);
    }

    /// Lay the rack out from `saved` rows: modules not on the wall, and
    /// second mentions, are left out, and modules in no row go where a new
    /// one of their kind would. With no saved rows, a row for each group
    /// of modules, in the groups' order.
    fn hang_rows(&mut self, saved: &[Vec<String>]) {
        let mut seen: Vec<&str> = Vec::new();
        let mut rows = Vec::with_capacity(saved.len());
        for ids in saved {
            let mut row = Vec::with_capacity(ids.len());
            for id in ids {
                if self.module(id).is_some() && !seen.contains(&id.as_str()) {
                    seen.push(id);
                    row.push(id.clone());
                }
            }
            if !row.is_empty() {
                rows.push(row);
            }
        }
        if saved.is_empty() {
            for group in 0..crate::catalogue::GROUPS.len() {
                let row: Vec<String> = self
                    .modules
                    .iter()
                    .filter(|module| module.kind.spec().group() == group)
                    .map(|module| module.id.clone())
                    .collect();
                if !row.is_empty() {
                    rows.push(row);
                }
            }
        }
        self.rows = rows;
        let missing: Vec<(String, Kind)> = self
            .modules
            .iter()
            .filter(|module| !self.rows.iter().any(|ids| ids.contains(&module.id)))
            .map(|module| (module.id.clone(), module.kind))
            .collect();
        for (id, kind) in missing {
            let place = self.usual_place(kind);
            put(&mut self.rows, &id, &place);
        }
    }

    /// Apply the inverse of `what` as a new change. Tempo changes and loads
    /// are the daemon's to undo (they involve the clock, or the whole
    /// wall), and refused here.
    ///
    /// # Errors
    ///
    /// Whatever the inverse operation refuses: a module that has gone, a
    /// cable already back.
    pub fn undo(&mut self, what: &What) -> Result<What, WallError> {
        match what {
            What::Chain { name, changes } => self.undo_chain(name, changes),
            What::Turn {
                module,
                knob,
                from,
                glide_beats,
                ..
            } => self.turn(module, knob, *from, Some(*glide_beats)),
            What::Patch { cable, replaced } => self.unplug(cable.id, replaced.as_ref()),
            What::Unpatch { cable, .. } => {
                if self.cable_at(cable.id).is_ok() {
                    return Err(error(
                        ErrorCode::NotAllowed,
                        format!("cable {} is plugged in already", cable.id),
                    ));
                }
                self.plug(&cable.from, &cable.to, Some(cable.amount), Some(cable.id))
            }
            What::Add { module } => self.remove(&module.id, &[]),
            What::Remove {
                module,
                cables,
                place,
                ..
            } => self.restore(module, cables, place.as_ref()),
            What::Restore {
                module, replaced, ..
            } => self.remove(&module.id, replaced),
            What::Tempo { .. } => Err(error(
                ErrorCode::Internal,
                "tempo changes are undone by the daemon",
            )),
            What::Load { .. } => Err(error(ErrorCode::Internal, "loads are undone by the daemon")),
            What::Abandoned { .. }
            | What::Resumed { .. }
            | What::SetAside { .. }
            | What::Migrate { .. }
            | What::Record { .. }
            | What::Speak { .. }
            | What::Unknown => Err(never_undone(what)
                .unwrap_or_else(|| error(ErrorCode::Internal, "that change cannot be undone"))),
        }
    }

    // -----------------------------------------------------------------
    // Ports
    // -----------------------------------------------------------------

    /// Split `module.port`.
    fn split(port: &str) -> Result<(&str, &str), WallError> {
        port.split_once('.')
            .filter(|(module, name)| !module.is_empty() && !name.is_empty())
            .ok_or_else(|| {
                error(
                    ErrorCode::UnknownPort,
                    format!(
                        "'{}' is not a port; write module.port, e.g. lfo1.out",
                        printable(port)
                    ),
                )
            })
    }

    /// Resolve an output `module.port`.
    fn output(&self, port: &str) -> Result<(String, usize), WallError> {
        let (id, name) = Self::split(port)?;
        let module = self.find(id)?;
        let spec = module.kind.spec();
        spec.output_index(name)
            .map(|index| (module.id.clone(), index))
            .ok_or_else(|| {
                let also = if spec.jack(name).is_some() {
                    format!(" ('{}' is an input)", printable(name))
                } else {
                    String::new()
                };
                error(
                    ErrorCode::UnknownPort,
                    format!(
                        "{id} has no output '{}'{also}; outputs: {}",
                        printable(name),
                        list(spec.outputs.iter().map(|p| p.name))
                    ),
                )
            })
    }

    /// Resolve an input or knob jack `module.port`.
    fn input(&self, port: &str) -> Result<(String, Jack), WallError> {
        let (id, name) = Self::split(port)?;
        let module = self.find(id)?;
        let spec: &KindSpec = module.kind.spec();
        spec.jack(name)
            .map(|jack| (module.id.clone(), jack))
            .ok_or_else(|| {
                let also = if spec.output_index(name).is_some() {
                    format!(" ('{}' is an output)", printable(name))
                } else {
                    String::new()
                };
                error(
                    ErrorCode::UnknownPort,
                    format!(
                        "{id} has no input or knob '{}'{also}; inputs and knobs: {}",
                        printable(name),
                        list(spec.jack_names())
                    ),
                )
            })
    }

    // -----------------------------------------------------------------
    // Saving and loading
    // -----------------------------------------------------------------

    /// The patch as saved.
    #[must_use]
    pub fn to_file(&self) -> PatchFile {
        PatchFile {
            version: FILE_VERSION,
            tempo: self.tempo,
            last_seq: self.last_seq,
            wall: self.wall,
            seed: self.seed,
            next_ids: self.next_ids.clone(),
            next_cable: self.next_cable,
            modules: self.modules.iter().map(Module::record).collect(),
            cables: self.cable_records(),
            dye: self.dye.deposits().clone(),
            speech: self.speech.clone(),
            retired_speech: self.retired_speech.iter().cloned().collect(),
            rack: self.rows.clone(),
            groups: self.groups.clone(),
        }
    }

    /// What the wall holds, as a short string that changes whenever any of
    /// it does: every module and knob target, every cable, the numbering,
    /// the tempo, the rows, the dye and the words; but not the latest
    /// change's number nor which wall it is, which move with every change
    /// and every load without the wall itself changing.
    ///
    /// It is [`hash_bytes`] of the patch's file as this build writes it
    /// (compact JSON, deterministic: every map in it is ordered). It is for
    /// telling, within one running daemon, whether the wall has changed
    /// since it was saved; it is never kept anywhere, since another build
    /// (a new format version, a new migration) may write the same wall
    /// differently. What is kept, to recognise a save later, is
    /// [`hash_bytes`] of the save's file as it is on disk.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        hash_bytes(&self.print_bytes())
    }

    /// The fingerprint as the stable 64-bit word used by birth marks.
    #[must_use]
    pub fn fingerprint_word(&self) -> u64 {
        crate::chance::fnv1a64(&self.print_bytes())
    }

    fn print_bytes(&self) -> Vec<u8> {
        let file = PatchFile {
            last_seq: 0,
            wall: 0,
            ..self.to_file()
        };
        match serde_json::to_vec(&file) {
            Ok(bytes) => bytes,
            Err(err) => {
                eprintln!("wall fingerprint could not be encoded: {err}");
                Vec::new()
            }
        }
    }

    /// Set a newborn module's identity; return whether it is on the wall.
    pub fn mark_birth(&mut self, id: &str, birth: Birth) -> bool {
        match self.module_mut(id) {
            Some(module) => {
                module.birth = Some(birth);
                true
            }
            None => false,
        }
    }

    /// [`Self::fingerprint`] of everything but the tempo: what the wall is
    /// while the desk keeps the tempo (and the wall's saved tempo follows
    /// the desk's, which is no change of the wall's own).
    #[must_use]
    pub fn fingerprint_but_tempo(&self) -> String {
        let file = PatchFile {
            last_seq: 0,
            wall: 0,
            tempo: 0.0,
            ..self.to_file()
        };
        serde_json::to_vec(&file).map_or_else(|_| String::new(), |bytes| hash_bytes(&bytes))
    }

    /// Make sure the numbering never gives out `id` again.
    fn count_id(&mut self, id: &str) {
        if let Some((kind, number)) = split_id(id) {
            let next = self.next_ids.entry(kind.to_string()).or_insert(1);
            *next = (*next).max(number.saturating_add(1));
        }
    }

    /// A patch from its saved form, checked through and through: any
    /// problem at all is an error (see [`Self::load`] for the forgiving
    /// version the daemon uses).
    ///
    /// # Errors
    ///
    /// A sentence saying everything that is wrong.
    pub fn from_file(file: &PatchFile) -> Result<Self, String> {
        let (patch, notes) = Self::load(file)?;
        if notes.is_empty() {
            Ok(patch)
        } else {
            Err(notes.join("; "))
        }
    }

    /// A patch from its saved form, keeping everything that can be kept.
    /// A module that cannot load is left out with its cables, a cable that
    /// cannot plug in is left out, a knob that is not the module's is
    /// ignored, and dye on nothing is dropped; each is described in the
    /// notes returned.
    ///
    /// # Errors
    ///
    /// Only a version this wall does not understand: migrate older files
    /// first (see [`crate::migrate`]).
    pub fn load(file: &PatchFile) -> Result<(Self, Vec<String>), String> {
        if file.version != FILE_VERSION {
            return Err(format!(
                "patch version {} is not version {FILE_VERSION}",
                file.version
            ));
        }
        let mut notes = Vec::new();
        let tempo = if file.tempo.is_finite() {
            file.tempo
        } else {
            notes.push("the tempo was not a number; it is 120 BPM".to_string());
            120.0
        };
        let mut patch = Self::empty(crate::engine::clamp_bpm(tempo));
        patch.last_seq = file.last_seq;
        patch.wall = file.wall;
        patch.seed = file.seed;
        patch.next_cable = file.next_cable.max(1);
        patch.next_ids = numbering_in(&file.next_ids, &mut notes);
        for record in &file.modules {
            if let Err(why) = patch.load_module(record, &mut notes) {
                notes.push(format!(
                    "module '{}' left out: {why}",
                    printable(&record.id)
                ));
            }
        }
        patch.find_births(&mut notes);
        let mut plugged = HashSet::with_capacity(file.cables.len());
        for record in &file.cables {
            if let Err(why) = patch.load_cable(record, &mut plugged) {
                notes.push(format!(
                    "cable {} ({} → {}) left out: {why}",
                    record.id,
                    printable(&record.from),
                    printable(&record.to)
                ));
            }
        }
        patch.dye = Dye::from_deposits(patch.dye_in(&file.dye, &mut notes))?;
        for (module, words) in &file.speech {
            let speaks = patch.module(module).is_some_and(|m| {
                matches!(
                    m.kind.spec().build,
                    crate::catalogue::Builder::Adapted(crate::adapters::Adapter::Speak)
                )
            });
            if speaks && !words_fit(words) {
                // Said without the words themselves: they are private.
                notes.push(format!(
                    "words for '{}' longer than a speaker may be given, dropped",
                    printable(module)
                ));
            } else if speaks {
                patch.speech.insert(module.clone(), words.clone());
            } else {
                notes.push(format!(
                    "words for '{}', which is not a speak module on the wall, dropped",
                    printable(module)
                ));
            }
        }
        for retired in &file.retired_speech {
            let away = patch.module(&retired.module).is_none();
            let fits = retired.module.len() <= MAX_ID && words_fit(&retired.words);
            if away && !fits {
                notes.push(format!(
                    "kept words for '{}' longer than a speaker may be given, dropped",
                    printable(&retired.module)
                ));
            } else if away {
                patch.retired_speech.push_back(retired.clone());
            } else {
                notes.push(format!(
                    "kept words for '{}', which is on the wall, dropped",
                    printable(&retired.module)
                ));
            }
        }
        while patch.retired_speech.len() > MAX_RETIRED_WORDS {
            patch.retired_speech.pop_front();
        }
        // The rows are where modules hang, not the patch's sound: they are
        // mended without a note.
        patch.hang_rows(&file.rack);
        // So are the groups: a module no longer on the wall leaves them.
        for (name, ids) in &file.groups {
            let ids: Vec<String> = ids
                .iter()
                .filter(|id| patch.places.contains_key(id.as_str()))
                .cloned()
                .collect();
            if valid_group_name(name) && !ids.is_empty() {
                patch.groups.insert(name.clone(), ids);
            }
        }
        cap_notes(&mut notes);
        Ok((patch, notes))
    }

    /// The dye of a file (`saved`) this wall keeps: on its modules, from
    /// seats with a seat's name, in positive amounts, and the strongest
    /// [`crate::fingerprints::MAX_DYE_SEATS`] seats on each; what is left
    /// out is said in `notes`.
    fn dye_in(
        &self,
        saved: &BTreeMap<String, BTreeMap<String, f64>>,
        notes: &mut Vec<String>,
    ) -> BTreeMap<String, BTreeMap<String, f64>> {
        let mut dye = saved.clone();
        dye.retain(|module, _| {
            let there = self.module(module).is_some();
            if !there {
                notes.push(format!(
                    "dye on '{}', which is not on the wall, dropped",
                    printable(module)
                ));
            }
            there
        });
        for (on, seats) in &mut dye {
            seats.retain(|who, amount| {
                let positive = amount.is_finite() && *amount > 0.0;
                let named = who.chars().count() <= crate::protocol::MAX_NAME;
                if !positive {
                    notes.push(format!(
                        "{}'s dye on {on} was not a positive number, dropped",
                        printable(who)
                    ));
                } else if !named {
                    notes.push(format!(
                        "dye on {on} from a seat whose name is longer than a seat's may be, \
                         dropped"
                    ));
                }
                positive && named
            });
            let faintest = crate::fingerprints::trim_seats(seats);
            if faintest > 0 {
                notes.push(format!(
                    "the faintest dye of {faintest} seats on {on} dropped (a module keeps the \
                     {} strongest)",
                    crate::fingerprints::MAX_DYE_SEATS
                ));
            }
        }
        dye.retain(|_, seats| !seats.is_empty());
        dye
    }

    pub(crate) fn find_births(&mut self, notes: &mut Vec<String>) {
        let mut seen = HashSet::new();
        let mut holders = BTreeMap::new();
        for module in &mut self.modules {
            if let Some(birth) = &module.birth {
                if seen.contains(birth) {
                    let id = holders.get(birth).map_or("another module", String::as_str);
                    notes.push(format!(
                        "module '{}' had the birth mark of {id}; it is found afresh",
                        module.id
                    ));
                    module.birth = None;
                }
            }
            let birth = module.birth.get_or_insert_with(|| Birth::Found {
                wall: self.wall,
                seed: self.seed,
                id: module.id.clone(),
            });
            seen.insert(birth.clone());
            holders.insert(birth.clone(), module.id.clone());
        }
    }

    fn load_module(
        &mut self,
        record: &ModuleRecord,
        notes: &mut Vec<String>,
    ) -> Result<(), String> {
        if self.module(&record.id).is_some() {
            return Err("its id appears twice".to_string());
        }
        let (module, ignored, bad_birth) = module_from_record(record).map_err(|err| err.message)?;
        if let Some(why) = bad_birth {
            notes.push(format!(
                "module '{}' had a birth mark that does not read ('{why}'); it is found afresh",
                module.id
            ));
        }
        for knob in ignored {
            notes.push(format!(
                "{} has no knob '{}'; its value was dropped",
                module.id,
                printable(&knob)
            ));
        }
        self.count_id(&module.id);
        self.push_module(module);
        Ok(())
    }

    /// Plug in a cable from a file, as [`Self::plug`] would under its old
    /// number. `plugged` holds the jacks the file's cables are in so far,
    /// so a wall of any size loads in one pass over its cables.
    fn load_cable(
        &mut self,
        record: &CableRecord,
        plugged: &mut HashSet<(String, Jack)>,
    ) -> Result<(), String> {
        if self.cable_at(record.id).is_ok() {
            return Err("its number appears twice".to_string());
        }
        if !record.amount.is_finite() {
            return Err("the amount must be a number".to_string());
        }
        let (from_module, from_port) = self.output(&record.from).map_err(|err| err.message)?;
        let (to_module, to_jack) = self.input(&record.to).map_err(|err| err.message)?;
        if !plugged.insert((to_module.clone(), to_jack)) {
            return Err(format!("another cable is plugged into {}", record.to));
        }
        self.next_cable = self.next_cable.max(record.id.saturating_add(1));
        let cable = Cable {
            id: record.id,
            from_module,
            from_port,
            to_module,
            to_jack,
            // Held to -1..1, so the narrowing is exact enough.
            amount: record.amount.clamp(-1.0, 1.0) as f32,
        };
        // A file keeps its cables in number order: each goes at the end.
        let (Ok(at) | Err(at)) = self.cable_at(cable.id);
        self.cables.insert(at, cable);
        Ok(())
    }
}

/// Move module `id` to `place` in `rows` (see [`Place`]): a module not in
/// any row is hung there. Rows left empty are dropped.
pub fn arrange_rows(rows: &mut Vec<Vec<String>>, id: &str, place: &Place) {
    // Taken out, its row stays (empty, if it was alone) so the place's row
    // counts the rows as they stood.
    take_out(rows, id);
    put(rows, id, place);
}

/// The place that moves module `id` from where it hangs in `from` to where
/// it hangs in `to`.
///
/// `to` is `from` with that one module moved (see [`arrange_rows`]);
/// `None` when `to` does not hold it. The place names the module after it
/// where there is one, so it holds up when other seats move other modules
/// meanwhile.
#[must_use]
pub fn place_for(from: &[Vec<String>], to: &[Vec<String>], id: &str) -> Option<Place> {
    let (row, at) = to
        .iter()
        .enumerate()
        .find_map(|(row, ids)| ids.iter().position(|other| other == id).map(|at| (row, at)))?;
    let row_of = |other: &str| from.iter().position(|ids| ids.iter().any(|x| x == other));
    let ids = &to[row];
    if let Some(next) = ids.get(at + 1) {
        return Some(Place {
            row: row_of(next).unwrap_or(row),
            before: Some(next.clone()),
            own: false,
        });
    }
    if let Some(previous) = at.checked_sub(1).and_then(|at| ids.get(at)) {
        return Some(Place::end_of(row_of(previous).unwrap_or(from.len())));
    }
    // Alone in its row: a row of its own just below the row above it.
    let row = row
        .checked_sub(1)
        .and_then(|above| to[above].first())
        .map_or(0, |above| row_of(above).map_or(from.len(), |row| row + 1));
    Some(Place {
        row,
        before: None,
        own: true,
    })
}

/// Take module `id` out of its row, leaving the row (even if empty).
fn take_out(rows: &mut [Vec<String>], id: &str) {
    for row in rows {
        row.retain(|other| other != id);
    }
}

/// Hang module `id` (in no row) at `place`, then drop empty rows. The
/// place counts rows as they stand; one past the last (or further) is a
/// new bottom row; a `before` that is not on the rack is the end of the
/// row.
fn put(rows: &mut Vec<Vec<String>>, id: &str, place: &Place) {
    let row = place.row.min(rows.len());
    let found = place.before.as_deref().and_then(|before| {
        rows.iter().enumerate().find_map(|(row, ids)| {
            ids.iter()
                .position(|other| other == before)
                .map(|at| (row, at))
        })
    });
    if place.own {
        rows.insert(row, vec![id.to_string()]);
    } else if let Some((row, at)) = found {
        rows[row].insert(at, id.to_string());
    } else if let Some(ids) = rows.get_mut(row) {
        ids.push(id.to_string());
    } else {
        rows.push(vec![id.to_string()]);
    }
    rows.retain(|ids| !ids.is_empty());
}

/// A module from a record, with the names of any knobs in the record the
/// kind does not have (ignored). The id must be well formed and the name
/// valid; knobs are held to their ranges, and missing ones take their
/// defaults.
fn module_from_record(
    record: &ModuleRecord,
) -> Result<(Module, Vec<String>, Option<String>), WallError> {
    let kind = Kind::from_name(&record.kind).ok_or_else(|| {
        error(
            ErrorCode::UnknownKind,
            format!("no module kind '{}'", printable(&record.kind)),
        )
    })?;
    if !valid_id(&record.id) {
        return Err(error(
            ErrorCode::BadRequest,
            format!("'{}' is not a module id", printable(&record.id)),
        ));
    }
    if let Some(name) = &record.name {
        if !valid_name(name) {
            return Err(bad_name(name));
        }
    }
    let spec = kind.spec();
    let ignored = record
        .knobs
        .keys()
        .filter(|name| spec.knob_index(name).is_none())
        .cloned()
        .collect();
    let knobs = spec
        .knobs
        .iter()
        .map(|knob| {
            record
                .knobs
                .get(knob.name)
                .filter(|value| value.is_finite())
                .map_or(knob.default, |value| {
                    // Held to the knob's range, so the narrowing is safe.
                    knob.clamp(value.clamp(-1.0e30, 1.0e30) as f32)
                })
        })
        .collect();
    let (birth, bad_birth) = record.birth.as_ref().map_or((None, None), |text| {
        text.parse::<Birth>().map_or_else(
            |_| (None, Some(printable(text))),
            |birth| (Some(birth), None),
        )
    });
    Ok((
        Module {
            id: record.id.clone(),
            kind,
            name: record.name.clone(),
            knobs,
            birth,
        },
        ignored,
        bad_birth,
    ))
}

/// The cable a successful [`Patch::plug`] plugged in; `None` when the plug
/// was refused (the caller reports that as a cable that stayed out).
fn plugged(result: Result<What, WallError>) -> Option<CableRecord> {
    match result {
        Ok(What::Patch { cable, .. }) => Some(cable),
        Ok(_) | Err(_) => None,
    }
}

/// Whether `name` can name a group: a name, with no spaces (so a command
/// line can take it as one word).
fn valid_group_name(name: &str) -> bool {
    valid_name(name) && !name.contains(' ')
}

fn bad_name(name: &str) -> WallError {
    error(
        ErrorCode::BadName,
        format!(
            "'{}' is not a valid name: use 1 to 24 of A-Z a-z 0-9 space _ . -",
            printable(name)
        ),
    )
}

/// Text from a request made safe to quote back: printable ASCII only, and
/// at most 40 characters.
#[must_use]
pub fn printable(text: &str) -> String {
    let mut out = String::new();
    for (count, c) in text.chars().enumerate() {
        if count == 40 {
            out.push('…');
            break;
        }
        if c.is_ascii_graphic() || c == ' ' {
            out.push(c);
        } else {
            // Writing to a String cannot fail.
            if write!(out, "\\u{{{:x}}}", u32::from(c)).is_err() {
                out.push('?');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;
