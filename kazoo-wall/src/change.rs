//! Change summaries and timestamps.
//!
//! A summary is one plain sentence built only from sanitised parts: the seat
//! name and module display names (both charset-limited), ids, numbers and
//! units. Nothing a seat typed freely reaches another seat through it.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::catalogue::Kind;
use crate::format;
use crate::patch::kind_of_id;
use crate::protocol::{CableRecord, ModuleRecord, What};

/// The sentence for `what`, done by `seat`; `undoes` names the change it
/// undid, if it was an undo.
///
/// Knob values are read with the units of the kind their module's id
/// names; see [`summarise_with`] for modules whose kind their id does not
/// name.
#[must_use]
pub fn summarise(seat: &str, what: &What, undoes: Option<u64>) -> String {
    summarise_with(seat, what, undoes, kind_of_id)
}

/// As [`summarise`], finding each module's kind with `kind_of`.
#[must_use]
pub fn summarise_with(
    seat: &str,
    what: &What,
    undoes: Option<u64>,
    kind_of: impl Fn(&str) -> Option<Kind>,
) -> String {
    let body = describe_with(what, &kind_of);
    undoes.map_or_else(
        || format!("{seat} {body}"),
        |change| format!("{seat} undid change {change}: {body}"),
    )
}

/// `what` as a sentence without its subject: `turned vcf1 cutoff ...`.
#[must_use]
pub fn describe(what: &What) -> String {
    describe_with(what, &kind_of_id)
}

fn describe_with(what: &What, kind_of: &impl Fn(&str) -> Option<Kind>) -> String {
    match what {
        What::Turn {
            module,
            knob,
            from,
            to,
            glide_beats,
        } => {
            let (from, to) = knob_values(kind_of(module), knob, *from, *to);
            let glide = if *glide_beats > 0.0 {
                format!("over {}", format::beats(*glide_beats))
            } else {
                "at once".to_string()
            };
            format!("turned {module} {knob} {from} → {to} {glide}")
        }
        What::Patch { cable, replaced } => {
            let mut parts = vec![format!("patched {}", cable_words(cable))];
            if let Some(old) = replaced {
                parts.push(format!("; it replaced cable {} from {}", old.id, old.from));
            }
            parts.concat()
        }
        What::Unpatch { cable, replugged } => {
            let mut parts = vec![format!("unplugged {}", cable_words(cable))];
            if let Some(back) = replugged {
                parts.push(format!("; cable {} from {} is back in", back.id, back.from));
            }
            parts.concat()
        }
        What::Add { module } => format!("added {}", module_words(module)),
        What::Remove {
            module,
            cables,
            replugged,
            ..
        } => {
            let mut parts = vec![format!("removed {}", module_words(module))];
            if !cables.is_empty() {
                parts.push(format!(" and {}", count(cables.len(), "cable")));
            }
            if !replugged.is_empty() {
                parts.push(format!("; {} back in", count(replugged.len(), "cable")));
            }
            parts.concat()
        }
        What::Restore {
            module,
            cables,
            replaced,
            skipped,
            ..
        } => {
            let mut parts = vec![format!(
                "brought back {} with {}",
                module_words(module),
                count(cables.len(), "cable")
            )];
            if !replaced.is_empty() {
                parts.push(format!(", replacing {}", count(replaced.len(), "cable")));
            }
            if !skipped.is_empty() {
                parts.push(format!(
                    "; {} could not come back",
                    count(skipped.len(), "cable")
                ));
            }
            parts.concat()
        }
        What::Speak {
            module,
            words,
            seconds,
        } => spoke(module, *words, *seconds),
        What::Unknown => "made a change this wall does not know".to_string(),
        What::Migrate {
            from_version,
            to_version,
            notes,
        } => migrated(*from_version, *to_version, notes),
        What::Record {
            on,
            path,
            seconds,
            dropped,
            continues,
            reason,
        } => recorded(
            *on,
            path,
            *seconds,
            *dropped,
            continues.as_deref(),
            reason.as_deref(),
        ),
        What::Tempo { from, to, desk } => tempo(*from, *to, *desk),
    }
}

/// A tempo set on the wall's own clock, or asked of the desk.
fn tempo(from: f64, to: f64, desk: bool) -> String {
    if desk {
        format!("asked the desk for {}", format::bpm(to))
    } else {
        format!("set the tempo {} → {}", format::bpm(from), format::bpm(to))
    }
}

/// A knob's before and after with units, when its module and knob are
/// known; plain numbers otherwise.
fn knob_values(kind: Option<Kind>, knob: &str, from: f64, to: f64) -> (String, String) {
    // Knob values are held to f32 ranges: the narrowing is exact enough.
    let (from, to) = (from as f32, to as f32);
    kind.and_then(|kind| {
        let spec = kind.spec();
        spec.knob_index(knob).map(|index| {
            let knob = &spec.knobs[index];
            (format::knob_value(knob, from), format::knob_value(knob, to))
        })
    })
    .unwrap_or_else(|| (format::number(from), format::number(to)))
}

fn spoke(module: &str, words: u32, seconds: f64) -> String {
    format!(
        "gave {module} {} to say ({})",
        count(usize::try_from(words).unwrap_or(usize::MAX), "word"),
        // A phrase's length is far inside f32's range.
        format::seconds(seconds as f32)
    )
}

/// A recording's start or stop. Files are named by the wall itself
/// (`wall-2026-09-27-203001.wav`), and only the name is told, not where it
/// is.
fn recorded(
    on: bool,
    path: &str,
    seconds: Option<f64>,
    dropped: u64,
    continues: Option<&str>,
    reason: Option<&str>,
) -> String {
    let mut parts = vec![if on {
        format!("started recording {}", file_name(path))
    } else {
        format!("stopped recording {}", file_name(path))
    }];
    if let Some(seconds) = seconds {
        parts.push(format!(" after {}", format::clock(seconds)));
    }
    if dropped > 0 {
        parts.push(format!(
            ", {} lost",
            count(usize::try_from(dropped).unwrap_or(usize::MAX), "sample")
        ));
    }
    if let Some(before) = continues {
        parts.push(format!(", carrying on from {}", file_name(before)));
    }
    if let Some(reason) = reason {
        parts.push(format!(": {reason}"));
    }
    parts.concat()
}

/// The last part of `path`: the file's own name.
fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn migrated(from_version: u32, to_version: u32, notes: &[String]) -> String {
    let what = if from_version == to_version {
        format!("loaded the patch (version {to_version})")
    } else {
        format!("updated the patch from version {from_version} to {to_version}")
    };
    match notes {
        [] => what,
        [only] => format!("{what}: {only}"),
        [first, rest @ ..] => format!("{what}: {first}; and {} more", rest.len()),
    }
}

fn cable_words(cable: &CableRecord) -> String {
    // Amounts are held to -1..1: the narrowing is exact enough.
    let amount = cable.amount as f32;
    if (amount - 1.0).abs() < 1e-6 {
        format!("{} → {} (cable {})", cable.from, cable.to, cable.id)
    } else {
        format!(
            "{} → {} (cable {}, amount {})",
            cable.from,
            cable.to,
            cable.id,
            format::number(amount)
        )
    }
}

fn module_words(module: &ModuleRecord) -> String {
    module.name.as_ref().map_or_else(
        || module.id.clone(),
        |name| format!("{} \"{name}\"", module.id),
    )
}

fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// The time now in UTC, as `2026-09-26T12:00:01Z`.
#[must_use]
pub fn utc_now() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    utc(seconds)
}

/// Seconds since the Unix epoch as `YYYY-MM-DDTHH:MM:SSZ`.
#[must_use]
pub fn utc(seconds: u64) -> String {
    let days = seconds / 86_400;
    let rest = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

/// The calendar date `days` after 1970-01-01 (Howard Hinnant's algorithm).
const fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn cable(id: u32, from: &str, to: &str, amount: f64) -> CableRecord {
        CableRecord {
            id,
            from: from.to_string(),
            to: to.to_string(),
            amount,
        }
    }

    #[test]
    fn turns_read_with_units_and_glides() {
        let what = What::Turn {
            module: "vcf1".to_string(),
            knob: "cutoff".to_string(),
            from: 420.0,
            to: 800.0,
            glide_beats: 4.0,
        };
        assert_eq!(
            summarise("Tom", &what, None),
            "Tom turned vcf1 cutoff 420 Hz → 800 Hz over 4 beats"
        );
        let what = What::Turn {
            module: "env2".to_string(),
            knob: "attack".to_string(),
            from: 0.25,
            to: 1.5,
            glide_beats: 0.0,
        };
        assert_eq!(
            summarise("Waffles", &what, Some(57)),
            "Waffles undid change 57: turned env2 attack 250 ms → 1.5 s at once"
        );
    }

    #[test]
    fn cables_modules_and_tempo_read_plainly() {
        let what = What::Patch {
            cable: cable(12, "lfo1.out", "vcf1.cutoff", 0.4),
            replaced: Some(cable(7, "lfo2.out", "vcf1.cutoff", 1.0)),
        };
        assert_eq!(
            summarise("Vesper", &what, None),
            "Vesper patched lfo1.out → vcf1.cutoff (cable 12, amount 0.4); it replaced cable 7 from lfo2.out"
        );
        let module = ModuleRecord {
            id: "lfo3".to_string(),
            kind: "lfo".to_string(),
            name: Some("slow wobble".to_string()),
            knobs: BTreeMap::new(),
        };
        assert_eq!(
            summarise(
                "Tom",
                &What::Add {
                    module: module.clone()
                },
                None
            ),
            "Tom added lfo3 \"slow wobble\""
        );
        let what = What::Remove {
            module,
            cables: vec![cable(1, "lfo3.out", "vco1.pitch", 1.0)],
            replugged: Vec::new(),
            place: None,
        };
        assert_eq!(
            summarise("Tom", &what, None),
            "Tom removed lfo3 \"slow wobble\" and 1 cable"
        );
        let what = What::Tempo {
            from: 96.0,
            to: 120.0,
            desk: false,
        };
        assert_eq!(
            summarise("Tom", &what, None),
            "Tom set the tempo 96 BPM → 120 BPM"
        );
        let what = What::Tempo {
            from: 96.0,
            to: 120.0,
            desk: true,
        };
        assert_eq!(
            summarise("Tom", &what, None),
            "Tom asked the desk for 120 BPM"
        );
    }

    #[test]
    fn recordings_read_by_their_file_name() {
        let started = What::Record {
            on: true,
            path: "/Users/tom/Music/kazoo-wall/wall-2026-09-27-203001.wav".to_string(),
            seconds: None,
            dropped: 0,
            continues: None,
            reason: None,
        };
        assert_eq!(
            summarise("Tom", &started, None),
            "Tom started recording wall-2026-09-27-203001.wav"
        );
        let stopped = What::Record {
            on: false,
            path: "/Users/tom/Music/kazoo-wall/wall-2026-09-27-203001.wav".to_string(),
            seconds: Some(200.5),
            dropped: 64,
            continues: None,
            reason: Some("the audio restarted at 44100 Hz".to_string()),
        };
        assert_eq!(
            summarise("kazoo-wall", &stopped, None),
            "kazoo-wall stopped recording wall-2026-09-27-203001.wav after 3:20, 64 samples lost: \
             the audio restarted at 44100 Hz"
        );
        let carried = What::Record {
            on: true,
            path: "/m/wall-2026-09-27-203404.wav".to_string(),
            seconds: None,
            dropped: 0,
            continues: Some("/m/wall-2026-09-27-203001.wav".to_string()),
            reason: Some("the audio restarted at 44100 Hz".to_string()),
        };
        assert_eq!(
            summarise("kazoo-wall", &carried, None),
            "kazoo-wall started recording wall-2026-09-27-203404.wav, carrying on from \
             wall-2026-09-27-203001.wav: the audio restarted at 44100 Hz"
        );
    }

    #[test]
    fn timestamps_are_utc_calendar_dates() {
        assert_eq!(utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(utc(1_790_424_001), "2026-09-26T12:00:01Z");
        assert!(utc_now().ends_with('Z'));
    }
}
