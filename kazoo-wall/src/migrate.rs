//! Bringing saved patches up to date.
//!
//! [`migrate`] takes a patch file of any version this wall has written and
//! rewrites it, in place, as the current version, saying in plain words
//! what it changed. Module ids, names and cable numbers never change, so
//! the change log and undo stay valid.
//!
//! **Version 1 → 2.** The wall's own `delay` and `reverb` modules are gone;
//! effects come from `kazoo-fx`.
//! - A `delay` becomes `digital`: like the old delay it is a clean line
//!   with a filter in its feedback path, so the patch sounds as it did
//!   (`tape` would add wow, flutter, hiss and wear the patch never had).
//!   Time sets both sides, the sync division keeps its note value, tone
//!   becomes the high cut the old loop filter had, and feedback and mix
//!   carry over. Only if `digital` is missing does `tape` stand in.
//! - A `reverb` becomes `plate`: size becomes the decay time, damping the
//!   high cut, and mix carries over (`hall` stands in if `plate` is
//!   missing).
//! - Their mono `in` becomes `left` (the right input follows it), and their
//!   mono `out` becomes `left`. Where that output fed another effect, or an
//!   `out` module's free right side, a cable for `right` is added beside it
//!   (with a new number), so the chain comes out in stereo.
//! - Knob cables follow their knobs to the new names.

use std::collections::BTreeMap;

use crate::catalogue::Kind;
use crate::patch::{FILE_VERSION, PatchFile, printable};
use crate::protocol::{CableRecord, ModuleRecord};

/// Read a patch file as forgivingly as possible.
///
/// Only JSON that is not an object with a whole-number `version` is
/// refused; a module or cable entry that cannot be read is left out, and
/// every other field that is missing or unreadable takes its default, each
/// with a note.
///
/// # Errors
///
/// A sentence saying why the bytes are not a patch at all.
pub fn read(bytes: &[u8]) -> Result<(PatchFile, Vec<String>), String> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|err| format!("it is not JSON: {err}"))?;
    let serde_json::Value::Object(mut fields) = value else {
        return Err("it is not a JSON object".to_string());
    };
    let version = match fields.get("version").and_then(serde_json::Value::as_u64) {
        Some(version) => match u32::try_from(version) {
            Ok(version) => version,
            Err(_) => return Err(format!("its version {version} is not one this wall knows")),
        },
        None => return Err("it has no version number".to_string()),
    };
    let mut notes = Vec::new();
    let tempo = take(&mut fields, "tempo", &mut notes)
        .as_f64()
        .unwrap_or(120.0);
    let last_seq = take(&mut fields, "last_seq", &mut notes)
        .as_u64()
        .unwrap_or(0);
    let next_cable = take(&mut fields, "next_cable", &mut notes)
        .as_u64()
        .map_or(1, |n| u32::try_from(n).unwrap_or(u32::MAX));
    let next_ids = entries(
        take(&mut fields, "next_ids", &mut notes),
        "id counter",
        &mut notes,
    );
    let modules = list(
        take(&mut fields, "modules", &mut notes),
        "module",
        &mut notes,
    );
    let cables = list(take(&mut fields, "cables", &mut notes), "cable", &mut notes);
    let dye = match fields.remove("dye") {
        None | Some(serde_json::Value::Null) => BTreeMap::new(),
        Some(other) => match serde_json::from_value(other) {
            Ok(dye) => dye,
            Err(err) => {
                notes.push(format!(
                    "the fingerprints could not be read ({err}); they start afresh"
                ));
                BTreeMap::new()
            }
        },
    };
    let speech = match fields.remove("speech") {
        None | Some(serde_json::Value::Null) => BTreeMap::new(),
        Some(other) => entries(other, "speaker's words", &mut notes),
    };
    let retired_speech = match fields.remove("retired_speech") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(other) => list(other, "removed speaker's words", &mut notes),
    };
    // The rack's rows are where modules hang: unreadable ones are laid out
    // afresh as the patch loads.
    let rack = match fields.remove("rack") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(other) => list(other, "rack row", &mut notes),
    };
    Ok((
        PatchFile {
            version,
            tempo,
            last_seq,
            next_ids,
            next_cable,
            modules,
            cables,
            dye,
            speech,
            retired_speech,
            rack,
        },
        notes,
    ))
}

/// Take field `name`, noting when it is missing (it then takes its
/// default).
fn take(
    fields: &mut serde_json::Map<String, serde_json::Value>,
    name: &str,
    notes: &mut Vec<String>,
) -> serde_json::Value {
    let value = fields.remove(name).unwrap_or(serde_json::Value::Null);
    if value.is_null() {
        notes.push(format!("the patch had no {name}; it takes its default"));
    }
    value
}

/// The entries of a JSON array that read as `T`, noting the others.
fn list<T: serde::de::DeserializeOwned>(
    value: serde_json::Value,
    what: &str,
    notes: &mut Vec<String>,
) -> Vec<T> {
    let serde_json::Value::Array(items) = value else {
        if !value.is_null() {
            notes.push(format!("the {what}s were not a list; there are none"));
        }
        return Vec::new();
    };
    let mut out = Vec::with_capacity(items.len());
    for (index, item) in items.into_iter().enumerate() {
        match serde_json::from_value(item) {
            Ok(entry) => out.push(entry),
            Err(err) => notes.push(format!(
                "{what} entry {} could not be read ({err}); left out",
                index + 1
            )),
        }
    }
    out
}

/// The entries of a JSON object that read as `T`, noting the others.
fn entries<T: serde::de::DeserializeOwned>(
    value: serde_json::Value,
    what: &str,
    notes: &mut Vec<String>,
) -> BTreeMap<String, T> {
    let serde_json::Value::Object(items) = value else {
        return BTreeMap::new();
    };
    let mut out = BTreeMap::new();
    for (key, item) in items {
        match serde_json::from_value(item) {
            Ok(entry) => {
                out.insert(key, entry);
            }
            Err(err) => notes.push(format!(
                "{what} '{}' could not be read ({err}); left out",
                printable(&key)
            )),
        }
    }
    out
}

/// What the old delay's sync knob stood for, by position (0 is free).
const OLD_DELAY_SYNC: [&str; 7] = ["free", "1/16", "1/8", "1/8d", "1/4", "1/4d", "1/2"];

/// Bring `file` up to [`FILE_VERSION`]. Returns what was changed, in
/// words; nothing for a file already current.
///
/// # Errors
///
/// A version this wall never wrote (0, or newer than it knows).
pub fn migrate(file: &mut PatchFile) -> Result<Vec<String>, String> {
    let mut notes = Vec::new();
    if file.version == 0 || file.version > FILE_VERSION {
        return Err(format!(
            "patch version {} is not one this wall knows (it knows 1 to {FILE_VERSION})",
            file.version
        ));
    }
    if file.version == 1 {
        one_to_two(file, &mut notes);
        file.version = 2;
    }
    Ok(notes)
}

/// How one old module's knobs and jacks move to its new kind.
struct Plan {
    old: &'static str,
    new: Kind,
    /// Old knob → new knobs, with how the value converts.
    knobs: &'static [(&'static str, &'static [&'static str], Convert)],
    /// New knobs set to fixed values, where the old module had nothing.
    fixed: &'static [(&'static str, f64)],
}

/// How a knob value converts.
#[derive(Clone, Copy)]
enum Convert {
    /// Unchanged.
    Same,
    /// The old delay's sync position, by note value.
    DelaySync,
    /// The old delay's tone (0..1) as the loop filter's cutoff in Hz.
    ToneHz,
    /// The old reverb's size (0..1) as a decay time in seconds.
    SizeSeconds,
    /// The old reverb's damping (0..1) as a high cut in Hz.
    DampingHz,
}

const DIGITAL: &[(&str, &[&str], Convert)] = &[
    ("time", &["ltime", "rtime"], Convert::Same),
    ("sync", &["lsync", "rsync"], Convert::DelaySync),
    ("feedback", &["feedback"], Convert::Same),
    ("tone", &["highcut"], Convert::ToneHz),
    ("mix", &["mix"], Convert::Same),
];

const TAPE: &[(&str, &[&str], Convert)] = &[
    ("time", &["time"], Convert::Same),
    ("sync", &["sync"], Convert::DelaySync),
    ("feedback", &["feedback"], Convert::Same),
    ("mix", &["mix"], Convert::Same),
];

const PLATE: &[(&str, &[&str], Convert)] = &[
    ("size", &["decay"], Convert::SizeSeconds),
    ("damping", &["highcut"], Convert::DampingHz),
    ("mix", &["mix"], Convert::Same),
];

const HALL: &[(&str, &[&str], Convert)] = &[
    ("size", &["decay"], Convert::SizeSeconds),
    ("mix", &["mix"], Convert::Same),
];

/// The plans for the removed kinds, with whichever effects are in the
/// catalogue.
fn plans() -> Vec<Plan> {
    let mut plans = Vec::new();
    if let Some(new) = Kind::from_name("digital") {
        plans.push(Plan {
            old: "delay",
            new,
            knobs: DIGITAL,
            // One line, as the old delay had: no crossfeed, no low cut.
            fixed: &[("cross", 0.0), ("lowcut", 20.0), ("routing", 0.0)],
        });
    } else if let Some(new) = Kind::from_name("tape") {
        plans.push(Plan {
            old: "delay",
            new,
            knobs: TAPE,
            fixed: &[],
        });
    }
    if let Some(new) = Kind::from_name("plate") {
        plans.push(Plan {
            old: "reverb",
            new,
            knobs: PLATE,
            // The old reverb had no pre-delay.
            fixed: &[("predelay", 0.0)],
        });
    } else if let Some(new) = Kind::from_name("hall") {
        plans.push(Plan {
            old: "reverb",
            new,
            knobs: HALL,
            fixed: &[],
        });
    }
    plans
}

fn convert(how: Convert, value: f64, new: Kind, knob: &str) -> Option<f64> {
    match how {
        Convert::Same => Some(value),
        Convert::DelaySync => {
            let position = value.round();
            // Rounded and at least 0: the cast is exact.
            let label = OLD_DELAY_SYNC.get(position.max(0.0) as usize).copied()?;
            let spec = new.spec();
            let target = &spec.knobs[spec.knob_index(knob)?];
            target
                .labels
                .iter()
                .position(|l| *l == label)
                // A label position is small: exact as f64.
                .map(|index| f64::from(target.min) + index as f64)
        }
        // The old loop filter: 200 Hz at tone 0, about 18 kHz at 1.
        Convert::ToneHz => Some(200.0 * (value.clamp(0.0, 1.0) * 6.5).exp2()),
        // Freeverb's room from 0 to 1 rings from about half a second to
        // eight; the plate's decay is set to match.
        Convert::SizeSeconds => Some(0.5 * 16.0_f64.powf(value.clamp(0.0, 1.0))),
        // Freeverb's damping from none to full rolls the tail off from
        // about 16 kHz down to 2 kHz.
        Convert::DampingHz => Some(16_000.0 * (-3.0 * value.clamp(0.0, 1.0)).exp2()),
    }
}

fn one_to_two(file: &mut PatchFile, notes: &mut Vec<String>) {
    let plans = plans();
    let moved = move_modules(file, &plans, notes);
    if !moved.is_empty() {
        move_cables(file, &moved, notes);
    }
}

/// Knob names that changed before version 2 was cut: kind, old, new.
const RENAMED_KNOBS: [(&str, &str, &str); 5] = [
    ("mix", "a", "level_a"),
    ("mix", "b", "level_b"),
    ("mix", "c", "level_c"),
    ("mix", "d", "level_d"),
    ("vco", "fm", "fm_depth"),
];

/// Rename old knobs, and give every removed kind's module its new kind and
/// knobs. Returns the plan each moved module followed, by id.
fn move_modules<'a>(
    file: &mut PatchFile,
    plans: &'a [Plan],
    notes: &mut Vec<String>,
) -> BTreeMap<String, &'a Plan> {
    let mut moved = BTreeMap::new();
    for module in &mut file.modules {
        for (kind, old, new) in RENAMED_KNOBS {
            if module.kind == kind {
                if let Some(value) = module.knobs.remove(old) {
                    module.knobs.insert(new.to_string(), value);
                }
            }
        }
        let Some(plan) = plans.iter().find(|plan| plan.old == module.kind) else {
            continue;
        };
        let mut knobs = BTreeMap::new();
        for (old, news, how) in plan.knobs {
            let Some(value) = module.knobs.get(*old) else {
                continue;
            };
            for new in *news {
                if let Some(converted) = convert(*how, *value, plan.new, new) {
                    knobs.insert((*new).to_string(), converted);
                }
            }
        }
        for (knob, value) in plan.fixed {
            if plan.new.spec().knob_index(knob).is_some() {
                knobs.insert((*knob).to_string(), *value);
            }
        }
        notes.push(format!(
            "{} is now a {} (the wall's own {} is gone)",
            printable(&module.id),
            plan.new.name(),
            plan.old
        ));
        module.kind = plan.new.name().to_string();
        module.knobs = knobs;
        moved.insert(module.id.clone(), plan);
    }
    moved
}

/// Re-point cables at the moved modules' new ports, and add the right-hand
/// partners that keep their chains in stereo.
fn move_cables(file: &mut PatchFile, moved: &BTreeMap<String, &Plan>, notes: &mut Vec<String>) {
    let first_knob = |plan: &Plan, old: &str| -> Option<&'static str> {
        plan.knobs
            .iter()
            .find(|(name, _, _)| *name == old)
            .and_then(|(_, news, _)| news.first().copied())
    };
    let used: Vec<String> = file.cables.iter().map(|c| c.to.clone()).collect();
    let mut added = Vec::new();
    for cable in &mut file.cables {
        let (Some((from_module, from_port)), Some((to_module, to_port))) =
            (split_port(&cable.from), split_port(&cable.to))
        else {
            continue;
        };
        let source_moved = moved.contains_key(&from_module) && from_port == "out";
        if source_moved {
            cable.from = format!("{from_module}.left");
        }
        let into_effect = moved.contains_key(&to_module) && to_port == "in";
        if let Some(plan) = moved.get(&to_module) {
            let port = if into_effect {
                Some("left")
            } else {
                first_knob(plan, &to_port)
            };
            if let Some(port) = port {
                cable.to = format!("{to_module}.{port}");
            }
        }
        // A moved effect feeding another, or an out's left side: its right
        // output goes beside it, where that input is free.
        let into_out = to_port == "left" && is_out(file_kind(&to_module, &file.modules));
        let partner = format!("{to_module}.right");
        if source_moved && (into_effect || into_out) && !used.contains(&partner) {
            added.push(CableRecord {
                id: 0,
                from: format!("{from_module}.right"),
                to: partner,
                amount: cable.amount,
            });
        }
    }
    for mut cable in added {
        cable.id = file.next_cable.max(1);
        file.next_cable = cable.id + 1;
        notes.push(format!(
            "added cable {} ({} → {}) so the effects play in stereo",
            cable.id, cable.from, cable.to
        ));
        file.cables.push(cable);
    }
}

/// `module.port`, split.
fn split_port(port: &str) -> Option<(String, String)> {
    port.split_once('.')
        .map(|(module, name)| (module.to_string(), name.to_string()))
}

fn file_kind<'a>(id: &str, modules: &'a [ModuleRecord]) -> Option<&'a str> {
    modules.iter().find(|m| m.id == id).map(|m| m.kind.as_str())
}

fn is_out(kind: Option<&str>) -> bool {
    kind == Some("out")
}

#[cfg(test)]
mod tests;
