//! Lasting module identities and the wall's deterministic text form.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::str::FromStr;

use serde::Deserialize;

use crate::catalogue::Kind;
use crate::patch::{PatchFile, RetiredWords, Words, printable, valid_id};
use crate::protocol::{CableRecord, ModuleRecord, What};

/// A module's lasting identity, independent of its present wall.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Birth {
    /// Born on a wall that marks births.
    Born {
        seq: u64,
        n: u32,
        at: String,
        print: u64,
        salt: u64,
    },
    /// Found in a file made before births were marked.
    Found { wall: u64, seed: u64, id: String },
}

impl Birth {
    /// The module a found mark names, when that is not `id`.
    ///
    /// A found mark is worked out from its own module's id, and ids never
    /// change, so such a mark is the named module's and never `id`'s. Read
    /// this way, two found marks on one wall can never be equal.
    #[must_use]
    pub fn names_another(&self, id: &str) -> Option<&str> {
        match self {
            Self::Found { id: named, .. } if named != id => Some(named),
            Self::Found { .. } | Self::Born { .. } => None,
        }
    }
}

impl fmt::Display for Birth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Born {
                seq,
                n,
                at,
                print,
                salt,
            } => write!(f, "born {seq}.{n} {at} {print:016x} {salt:016x}"),
            Self::Found { wall, seed, id } => write!(f, "found {wall} {seed:016x} {id}"),
        }
    }
}

impl Ord for Birth {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (
                Self::Found { wall, seed, id },
                Self::Found {
                    wall: other_wall,
                    seed: other_seed,
                    id: other_id,
                },
            ) => (wall, seed, id).cmp(&(other_wall, other_seed, other_id)),
            (
                Self::Born {
                    seq,
                    n,
                    at,
                    print,
                    salt,
                },
                Self::Born {
                    seq: other_seq,
                    n: other_n,
                    at: other_at,
                    print: other_print,
                    salt: other_salt,
                },
            ) => (at, seq, n, print, salt).cmp(&(
                other_at,
                other_seq,
                other_n,
                other_print,
                other_salt,
            )),
            (Self::Found { .. }, Self::Born { .. }) => Ordering::Less,
            (Self::Born { .. }, Self::Found { .. }) => Ordering::Greater,
        }
    }
}

impl PartialOrd for Birth {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn decimal<T: FromStr>(text: &str) -> Result<T, ()> {
    if text.is_empty()
        || !text.bytes().all(|b| b.is_ascii_digit())
        || (text.len() > 1 && text.starts_with('0'))
    {
        return Err(());
    }
    text.parse().map_err(|_| ())
}

fn hex(text: &str) -> Result<u64, ()> {
    if text.len() != 16
        || !text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(());
    }
    u64::from_str_radix(text, 16).map_err(|_| ())
}

fn instant(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 20
        && bytes.iter().enumerate().all(|(i, b)| match i {
            4 | 7 => *b == b'-',
            10 => *b == b'T',
            13 | 16 => *b == b':',
            19 => *b == b'Z',
            _ => b.is_ascii_digit(),
        })
        && bytes[17] <= b'5'
        && chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%SZ").is_ok()
}

impl FromStr for Birth {
    type Err = TreeError;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let parts: Vec<_> = text.split(' ').collect();
        let bad = || TreeError::BadBirth(printable(text));
        match parts.as_slice() {
            ["born", number, at, print, salt] if instant(at) => {
                let (seq, n) = number.split_once('.').ok_or_else(bad)?;
                Ok(Self::Born {
                    seq: decimal(seq).map_err(|()| bad())?,
                    n: decimal(n).map_err(|()| bad())?,
                    at: (*at).to_string(),
                    print: hex(print).map_err(|()| bad())?,
                    salt: hex(salt).map_err(|()| bad())?,
                })
            }
            ["found", wall, seed, id] if valid_id(id) => Ok(Self::Found {
                wall: decimal(wall).map_err(|()| bad())?,
                seed: hex(seed).map_err(|()| bad())?,
                id: (*id).to_string(),
            }),
            _ => Err(bad()),
        }
    }
}

/// Stamp unmarked adds in depth-first change order; restores keep their identity.
#[must_use]
pub fn stamp_births(
    what: &mut What,
    seq: u64,
    at: &str,
    print: u64,
    salt: u64,
) -> Vec<(String, Birth)> {
    fn walk(
        what: &mut What,
        seq: u64,
        at: &str,
        print: u64,
        salt: u64,
        n: &mut u32,
        births: &mut Vec<(String, Birth)>,
    ) {
        match what {
            What::Add { module } => {
                if module.birth.is_none() {
                    let birth = Birth::Born {
                        seq,
                        n: *n,
                        at: at.to_string(),
                        print,
                        salt,
                    };
                    *n = n.saturating_add(1);
                    module.birth = Some(birth.to_string());
                    births.push((module.id.clone(), birth));
                }
            }
            What::Chain { changes, .. } => {
                for change in changes {
                    walk(change, seq, at, print, salt, n, births);
                }
            }
            What::Turn { .. }
            | What::Patch { .. }
            | What::Unpatch { .. }
            | What::Remove { .. }
            | What::Restore { .. }
            | What::Speak { .. }
            | What::Migrate { .. }
            | What::Tempo { .. }
            | What::Record { .. }
            | What::Load { .. }
            | What::Abandoned { .. }
            | What::SetAside { .. }
            | What::Resumed { .. }
            | What::Unknown => {}
        }
    }
    let mut births = Vec::new();
    walk(what, seq, at, print, salt, &mut 0, &mut births);
    births
}

pub(crate) fn has_add(what: &What) -> bool {
    match what {
        What::Add { .. } => true,
        What::Chain { changes, .. } => changes.iter().any(has_add),
        What::Turn { .. }
        | What::Patch { .. }
        | What::Unpatch { .. }
        | What::Remove { .. }
        | What::Restore { .. }
        | What::Speak { .. }
        | What::Migrate { .. }
        | What::Tempo { .. }
        | What::Record { .. }
        | What::Load { .. }
        | What::Abandoned { .. }
        | What::SetAside { .. }
        | What::Resumed { .. }
        | What::Unknown => false,
    }
}

/// Relative paths and their exact UTF-8 file bytes.
pub type Files = BTreeMap<String, Vec<u8>>;

/// A malformed identity or wall text form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeError {
    BadBirth(String),
    NotOneWall(String),
    NotANumber { path: String, key: String },
    TooLarge { what: &'static str },
    UnknownFile(String),
    Missing(String),
    Parse { path: String, reason: String },
}

impl fmt::Display for TreeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadBirth(text) => write!(f, "birth mark does not read ('{text}')"),
            Self::NotOneWall(reason) => write!(f, "not one wall: {reason}"),
            Self::NotANumber { path, key } => write!(f, "{path}: '{key}' is not a finite number"),
            Self::TooLarge { what } => write!(f, "{what} is too large for a TOML integer"),
            Self::UnknownFile(path) => write!(f, "unknown wall file '{path}'"),
            Self::Missing(path) => write!(f, "missing wall file '{path}'"),
            Self::Parse { path, reason } => write!(f, "{path} does not read: {reason}"),
        }
    }
}
impl std::error::Error for TreeError {}

fn string(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            '\u{0}'..='\u{1f}' | '\u{7f}' => {
                let escaped = format!("\\u{:04X}", u32::from(c));
                out.push_str(&escaped);
            }
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

fn key(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        value.to_string()
    } else {
        string(value)
    }
}

fn float(value: f64, path: &str, key: &str) -> Result<String, TreeError> {
    if !value.is_finite() {
        return Err(TreeError::NotANumber {
            path: path.into(),
            key: key.into(),
        });
    }
    if value % 1.0 == 0.0 {
        Ok(format!("{value}.0"))
    } else {
        Ok(format!("{value}"))
    }
}

fn line(out: &mut String, name: &str, value: &str) {
    out.push_str(name);
    out.push_str(" = ");
    out.push_str(value);
    out.push('\n');
}

fn array(values: &[String]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(|s| string(s))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn validate(file: &PatchFile) -> Result<(), TreeError> {
    let mut ids = HashSet::new();
    for module in &file.modules {
        if !valid_id(&module.id) {
            return Err(TreeError::NotOneWall(format!(
                "invalid module id '{}'",
                printable(&module.id)
            )));
        }
        if !ids.insert(module.id.as_str()) {
            return Err(TreeError::NotOneWall(format!(
                "module '{}' appears twice",
                module.id
            )));
        }
        if let Some(birth) = &module.birth {
            birth.parse::<Birth>()?;
        }
    }
    let mut previous = None;
    let mut jacks = HashSet::new();
    for cable in &file.cables {
        if previous.is_some_and(|id| id >= cable.id) {
            return Err(TreeError::NotOneWall(
                "cable numbers are not strictly ascending".into(),
            ));
        }
        previous = Some(cable.id);
        let Some((id, jack)) = cable.to.split_once('.') else {
            return Err(TreeError::NotOneWall(format!(
                "cable {} has no destination jack",
                cable.id
            )));
        };
        if !ids.contains(id) || jack.is_empty() {
            return Err(TreeError::NotOneWall(format!(
                "cable {} has no module and jack on this wall",
                cable.id
            )));
        }
        if !jacks.insert(cable.to.as_str()) {
            return Err(TreeError::NotOneWall(format!(
                "two cables feed '{}'",
                printable(&cable.to)
            )));
        }
    }
    for id in file.dye.keys().chain(file.speech.keys()) {
        if !ids.contains(id.as_str()) {
            return Err(TreeError::NotOneWall(format!(
                "dye or words belong to missing module '{}'",
                printable(id)
            )));
        }
    }
    Ok(())
}

fn module_text(file: &PatchFile, module: &ModuleRecord) -> Result<String, TreeError> {
    let path = format!("modules/{}.toml", module.id);
    let mut out = String::new();
    line(&mut out, "kind", &string(&module.kind));
    if let Some(name) = &module.name {
        line(&mut out, "name", &string(name));
    }
    if let Some(birth) = &module.birth {
        line(&mut out, "birth", &string(birth));
    }
    out.push_str("\n[knobs]\n");
    let ordered: Vec<_> = Kind::from_name(&module.kind).map_or_else(Vec::new, |kind| {
        kind.spec()
            .knobs
            .iter()
            .filter_map(|knob| module.knobs.get(knob.name).map(|value| (knob.name, value)))
            .collect()
    });
    for (name, value) in &ordered {
        line(&mut out, &key(name), &float(**value, &path, name)?);
    }
    for (name, value) in &module.knobs {
        if !ordered.iter().any(|(known, _)| *known == name) {
            line(&mut out, &key(name), &float(*value, &path, name)?);
        }
    }
    let cables: Vec<_> = file
        .cables
        .iter()
        .filter_map(|cable| {
            cable
                .to
                .split_once('.')
                .filter(|(id, _)| *id == module.id)
                .map(|(_, jack)| (jack, cable))
        })
        .collect();
    if !cables.is_empty() {
        out.push_str("\n[inputs]\n");
        for (jack, cable) in cables {
            line(
                &mut out,
                &key(jack),
                &format!(
                    "{{ cable = {}, from = {}, amount = {} }}",
                    cable.id,
                    string(&cable.from),
                    float(cable.amount, &path, jack)?
                ),
            );
        }
    }
    if let Some(dye) = file.dye.get(&module.id) {
        out.push_str("\n[dye]\n");
        for (seat, value) in dye {
            line(&mut out, &key(seat), &float(*value, &path, seat)?);
        }
    }
    if let Some(words) = file.speech.get(&module.id) {
        out.push_str("\n[words]\n");
        line(&mut out, "text", &string(&words.text));
        if let Some(voice) = &words.voice {
            line(&mut out, "voice", &string(voice));
        }
    }
    Ok(out)
}

fn wall_text(file: &PatchFile) -> Result<String, TreeError> {
    for (what, number) in [("last_seq", file.last_seq), ("wall", file.wall)] {
        if i64::try_from(number).is_err() {
            return Err(TreeError::TooLarge { what });
        }
    }
    let mut out = String::new();
    line(&mut out, "version", &file.version.to_string());
    line(&mut out, "tempo", &float(file.tempo, "wall.toml", "tempo")?);
    line(&mut out, "last_seq", &file.last_seq.to_string());
    line(&mut out, "wall", &file.wall.to_string());
    line(&mut out, "seed", &string(&format!("{:016x}", file.seed)));
    line(&mut out, "next_cable", &file.next_cable.to_string());
    if file.modules.is_empty() {
        out.push_str("modules = []\n");
    } else {
        out.push_str("modules = [\n");
        for module in &file.modules {
            out.push_str("    ");
            out.push_str(&string(&module.id));
            out.push_str(",\n");
        }
        out.push_str("]\n");
    }
    if file.rack.is_empty() {
        out.push_str("rack = []\n");
    } else {
        out.push_str("rack = [\n");
        for row in &file.rack {
            out.push_str("    ");
            out.push_str(&array(row));
            out.push_str(",\n");
        }
        out.push_str("]\n");
    }
    out.push_str("\n[next_ids]\n");
    for (kind, number) in &file.next_ids {
        line(&mut out, &key(kind), &number.to_string());
    }
    out.push_str("\n[groups]\n");
    for (name, modules) in &file.groups {
        line(&mut out, &key(name), &array(modules));
    }
    for retired in &file.retired_speech {
        out.push_str("\n[[retired_speech]]\n");
        line(&mut out, "module", &string(&retired.module));
        line(&mut out, "text", &string(&retired.words.text));
        if let Some(voice) = &retired.words.voice {
            line(&mut out, "voice", &string(voice));
        }
    }
    Ok(out)
}

/// Encode one save as canonical text files, without I/O.
pub fn to_files(file: &PatchFile) -> Result<Files, TreeError> {
    validate(file)?;
    let mut files = Files::new();
    files.insert("wall.toml".into(), wall_text(file)?.into_bytes());
    for module in &file.modules {
        files.insert(
            format!("modules/{}.toml", module.id),
            module_text(file, module)?.into_bytes(),
        );
    }
    Ok(files)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WallToml {
    version: u32,
    tempo: f64,
    last_seq: u64,
    wall: u64,
    seed: String,
    next_cable: u32,
    modules: Vec<String>,
    rack: Vec<Vec<String>>,
    next_ids: BTreeMap<String, u32>,
    groups: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    retired_speech: Vec<RetiredToml>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModuleToml {
    kind: String,
    name: Option<String>,
    birth: Option<String>,
    knobs: BTreeMap<String, f64>,
    #[serde(default)]
    inputs: BTreeMap<String, InputToml>,
    dye: Option<BTreeMap<String, f64>>,
    words: Option<WordsToml>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InputToml {
    cable: u32,
    from: String,
    amount: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WordsToml {
    text: String,
    voice: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RetiredToml {
    module: String,
    text: String,
    voice: Option<String>,
}

fn parse<T: serde::de::DeserializeOwned>(files: &Files, path: &str) -> Result<T, TreeError> {
    let bytes = files
        .get(path)
        .ok_or_else(|| TreeError::Missing(path.into()))?;
    let text = std::str::from_utf8(bytes).map_err(|err| TreeError::Parse {
        path: path.into(),
        reason: err.to_string(),
    })?;
    toml::from_str(text).map_err(|err| TreeError::Parse {
        path: path.into(),
        reason: err.to_string(),
    })
}

/// Read a complete wall tree strictly, without I/O or catalogue migration.
pub fn from_files(files: &Files) -> Result<PatchFile, TreeError> {
    for path in files.keys() {
        if path != "wall.toml"
            && !path
                .strip_prefix("modules/")
                .and_then(|s| s.strip_suffix(".toml"))
                .is_some_and(valid_id)
        {
            return Err(TreeError::UnknownFile(path.clone()));
        }
    }
    let wall: WallToml = parse(files, "wall.toml")?;
    let seed = hex(&wall.seed).map_err(|()| TreeError::Parse {
        path: "wall.toml".into(),
        reason: "seed must be 16 lower-case hex digits".into(),
    })?;
    let mut file = PatchFile {
        version: wall.version,
        tempo: wall.tempo,
        last_seq: wall.last_seq,
        wall: wall.wall,
        seed,
        next_ids: wall.next_ids,
        next_cable: wall.next_cable,
        modules: Vec::new(),
        cables: Vec::new(),
        dye: BTreeMap::new(),
        speech: BTreeMap::new(),
        rack: wall.rack,
        groups: wall.groups,
        retired_speech: wall
            .retired_speech
            .into_iter()
            .map(|r| RetiredWords {
                module: r.module,
                words: Words {
                    text: r.text,
                    voice: r.voice,
                },
            })
            .collect(),
    };
    let mut expected = HashSet::new();
    expected.insert("wall.toml".to_string());
    for id in wall.modules {
        if !valid_id(&id) || !expected.insert(format!("modules/{id}.toml")) {
            return Err(TreeError::NotOneWall(format!(
                "invalid or duplicate module id '{}'",
                printable(&id)
            )));
        }
        let path = format!("modules/{id}.toml");
        let module: ModuleToml = parse(files, &path)?;
        if let Some(birth) = &module.birth {
            birth.parse::<Birth>()?;
        }
        for (jack, input) in module.inputs {
            file.cables.push(CableRecord {
                id: input.cable,
                from: input.from,
                to: format!("{id}.{jack}"),
                amount: input.amount,
            });
        }
        if let Some(dye) = module.dye {
            file.dye.insert(id.clone(), dye);
        }
        if let Some(words) = module.words {
            file.speech.insert(
                id.clone(),
                Words {
                    text: words.text,
                    voice: words.voice,
                },
            );
        }
        file.modules.push(ModuleRecord {
            id,
            kind: module.kind,
            name: module.name,
            knobs: module.knobs,
            birth: module.birth,
        });
    }
    for path in files.keys() {
        if !expected.contains(path) {
            return Err(TreeError::UnknownFile(path.clone()));
        }
    }
    file.cables.sort_by_key(|c| c.id);
    // The writer checks finite scalars and all structural invariants too.
    to_files(&file)?;
    Ok(file)
}

#[cfg(test)]
mod tests;
