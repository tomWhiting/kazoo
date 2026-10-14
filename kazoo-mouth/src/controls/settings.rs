use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use kazoo_core::synthesis::OscillatorShape;
use kazoo_core::voice::quantise::{NOTE_NAMES, SCALE_NAMES};
use serde::{Deserialize, Serialize};

pub use super::Settings;
use crate::engine::fx::{Slot, mouth_default, permutation};
use crate::engine::looper::{BAR_CHOICES, LoopLength};

#[derive(Debug)]
pub struct Loaded {
    pub settings: Settings,
    pub notes: Vec<String>,
    pub writable: bool,
}

impl Default for Loaded {
    fn default() -> Self {
        Self {
            settings: Settings::default(),
            notes: Vec::new(),
            writable: true,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Document {
    version: u64,
    quantise: Quantise,
    synth: Synth,
    #[serde(deserialize_with = "number")]
    tempo: Option<f64>,
    fx: Effects,
    #[serde(default, rename = "loop")]
    looper: Option<LoopSettings>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum Length {
    Free(String),
    Bars(u8),
}

#[derive(Debug, Serialize, Deserialize)]
struct LoopSettings {
    length: Length,
    #[serde(deserialize_with = "number")]
    volume: Option<f32>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Quantise {
    on: bool,
    scale: String,
    root: String,
    #[serde(deserialize_with = "number")]
    snap_ms: Option<f32>,
    keep_vibrato: bool,
    #[serde(deserialize_with = "number")]
    hysteresis_cents: Option<f32>,
    #[serde(deserialize_with = "number")]
    blend: Option<f32>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Synth {
    shape: String,
    #[serde(deserialize_with = "number")]
    cutoff_hz: Option<f32>,
    #[serde(deserialize_with = "number")]
    air: Option<f32>,
    #[serde(deserialize_with = "number")]
    volume: Option<f32>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Effects {
    order: [String; 5],
    slots: BTreeMap<String, Pedal>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Pedal {
    on: bool,
    kind: String,
    kinds: BTreeMap<String, BTreeMap<String, Option<f32>>>,
}

fn number<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

impl From<&Settings> for Document {
    fn from(settings: &Settings) -> Self {
        let quantise = settings.quantise;
        let slots = Slot::ALL
            .into_iter()
            .map(|slot| {
                let saved = settings.fx.slots[slot as usize];
                let kinds = slot
                    .ids()
                    .iter()
                    .enumerate()
                    .map(|(kind, id)| {
                        let params = slot.kind(kind).map_or_else(BTreeMap::new, |description| {
                            description
                                .params
                                .iter()
                                .enumerate()
                                .map(|(index, spec)| {
                                    (spec.name.into(), Some(saved.values[kind][index]))
                                })
                                .collect()
                        });
                        ((*id).into(), params)
                    })
                    .collect();
                (
                    slot.name().into(),
                    Pedal {
                        on: saved.on,
                        kind: slot.ids()[saved.active].into(),
                        kinds,
                    },
                )
            })
            .collect();
        Self {
            version: 1,
            quantise: Quantise {
                on: quantise.blend > 0.0,
                scale: SCALE_NAMES[quantise.scale].into(),
                root: NOTE_NAMES[usize::from(quantise.root)].into(),
                snap_ms: Some(quantise.snap_ms),
                keep_vibrato: quantise.keep_vibrato,
                hysteresis_cents: Some(quantise.hysteresis_cents),
                blend: Some(settings.last_blend),
            },
            synth: Synth {
                shape: shape_name(settings.synth.shape).into(),
                cutoff_hz: Some(settings.synth.cutoff_hz),
                air: Some(settings.synth.air),
                volume: Some(settings.synth.volume),
            },
            tempo: Some(settings.own_bpm),
            looper: Some(LoopSettings {
                length: match settings.loop_length {
                    LoopLength::Free => Length::Free("free".into()),
                    LoopLength::Bars(bars) => Length::Bars(bars),
                },
                volume: Some(settings.loop_volume),
            }),
            fx: Effects {
                order: settings.fx.order.map(|slot| slot.name().into()),
                slots,
            },
        }
    }
}

pub const fn shape_name(shape: OscillatorShape) -> &'static str {
    match shape {
        OscillatorShape::Sine => "sine",
        OscillatorShape::Triangle => "triangle",
        OscillatorShape::Saw => "saw",
        OscillatorShape::Square => "square",
    }
}

fn finite(value: Option<f32>, default: f32, name: &str, notes: &mut Vec<String>) -> f32 {
    value.filter(|value| value.is_finite()).unwrap_or_else(|| {
        notes.push(format!("{name} is not finite: using its default"));
        default
    })
}

impl Quantise {
    fn apply(self, settings: &mut Settings, notes: &mut Vec<String>) {
        if let Some(scale) = SCALE_NAMES.iter().position(|name| *name == self.scale) {
            settings.quantise.scale = scale;
        } else {
            notes.push(format!("unknown scale: {}", self.scale));
        }
        if let Some(root) = NOTE_NAMES.iter().position(|name| *name == self.root) {
            settings.quantise.root = root as u8;
        } else {
            notes.push(format!("unknown note: {}", self.root));
        }
        settings.quantise.snap_ms = held(
            self.snap_ms,
            settings.quantise.snap_ms,
            0.0,
            400.0,
            "snap_ms",
            notes,
        );
        settings.quantise.hysteresis_cents = held(
            self.hysteresis_cents,
            settings.quantise.hysteresis_cents,
            0.0,
            50.0,
            "hysteresis_cents",
            notes,
        );
        settings.quantise.keep_vibrato = self.keep_vibrato;
        settings.last_blend = held(self.blend, 1.0, 0.0, 1.0, "blend", notes);
        settings.quantise.blend = if self.on { settings.last_blend } else { 0.0 };
        settings.quantise = settings.quantise.held();
    }
}

fn held(
    value: Option<f32>,
    default: f32,
    min: f32,
    max: f32,
    name: &str,
    notes: &mut Vec<String>,
) -> f32 {
    let value = finite(value, default, name, notes);
    if !(min..=max).contains(&value) {
        notes.push(format!("{name} outside its range: held to {min}..{max}"));
    }
    value.clamp(min, max)
}

impl Synth {
    fn apply(self, settings: &mut Settings, notes: &mut Vec<String>) {
        match self.shape.as_str() {
            "sine" => settings.synth.shape = OscillatorShape::Sine,
            "triangle" => settings.synth.shape = OscillatorShape::Triangle,
            "saw" => settings.synth.shape = OscillatorShape::Saw,
            "square" => settings.synth.shape = OscillatorShape::Square,
            name => notes.push(format!("unknown synth shape: {name}")),
        }
        settings.synth.cutoff_hz = held(
            self.cutoff_hz,
            settings.synth.cutoff_hz,
            100.0,
            12_000.0,
            "cutoff_hz",
            notes,
        );
        settings.synth.air = held(self.air, settings.synth.air, 0.0, 1.0, "air", notes);
        settings.synth.volume = held(
            self.volume,
            settings.synth.volume,
            0.0,
            1.0,
            "volume",
            notes,
        );
        settings.synth = settings.synth.held();
    }
}

impl Effects {
    fn apply(self, settings: &mut Settings, notes: &mut Vec<String>) {
        let mut order = Slot::ALL;
        let mut known = true;
        for (index, name) in self.order.iter().enumerate() {
            if let Some(slot) = Slot::ALL.into_iter().find(|slot| slot.name() == name) {
                order[index] = slot;
            } else {
                known = false;
                notes.push(format!("unknown slot in order: {name}"));
            }
        }
        if known && permutation(order) {
            settings.fx.order = order;
        } else {
            notes.push("effect order is not a permutation: using default order".into());
        }
        for (name, pedal) in self.slots {
            let Some(slot) = Slot::ALL.into_iter().find(|slot| slot.name() == name) else {
                notes.push(format!("unknown slot: {name}"));
                continue;
            };
            pedal.apply(slot, settings, notes);
        }
    }
}

impl Pedal {
    fn apply(self, slot: Slot, settings: &mut Settings, notes: &mut Vec<String>) {
        let saved = &mut settings.fx.slots[slot as usize];
        saved.on = self.on;
        if let Some(kind) = slot.ids().iter().position(|id| *id == self.kind) {
            saved.active = kind;
        } else {
            notes.push(format!("unknown {} kind: {}", slot.name(), self.kind));
        }
        for (id, params) in self.kinds {
            let Some(kind) = slot.ids().iter().position(|entry| *entry == id) else {
                notes.push(format!("unknown {} kind: {id}", slot.name()));
                continue;
            };
            let Some(description) = slot.kind(kind) else {
                notes.push(format!("effect kind is missing: {id}"));
                continue;
            };
            for (name, value) in params {
                let Some(index) = description.params.iter().position(|spec| spec.name == name)
                else {
                    notes.push(format!("unknown {id} parameter: {name}"));
                    continue;
                };
                let spec = &description.params[index];
                let value = held(
                    value,
                    mouth_default(slot, kind, spec),
                    spec.min,
                    spec.max,
                    &format!("{id}.{name}"),
                    notes,
                );
                saved.values[kind][index] = spec.clamp(value);
            }
        }
    }
}

impl Document {
    fn apply(self, loaded: &mut Loaded) {
        self.quantise.apply(&mut loaded.settings, &mut loaded.notes);
        self.synth.apply(&mut loaded.settings, &mut loaded.notes);
        if let Some(tempo) = self.tempo.filter(|tempo| tempo.is_finite()) {
            loaded.settings.own_bpm = tempo.clamp(30.0, 300.0);
            if !(30.0..=300.0).contains(&tempo) {
                loaded
                    .notes
                    .push("tempo outside its range: held to 30..300".into());
            }
        } else {
            loaded.notes.push("tempo is not finite: using 120".into());
        }
        self.fx.apply(&mut loaded.settings, &mut loaded.notes);
        if let Some(looper) = self.looper {
            match looper.length {
                Length::Free(name) if name == "free" => {
                    loaded.settings.loop_length = LoopLength::Free
                }
                Length::Bars(bars) if BAR_CHOICES.contains(&bars) => {
                    loaded.settings.loop_length = LoopLength::Bars(bars)
                }
                _ => loaded.notes.push("unknown loop length: using free".into()),
            }
            loaded.settings.loop_volume = held(
                looper.volume,
                loaded.settings.loop_volume,
                0.0,
                1.0,
                "loop.volume",
                &mut loaded.notes,
            );
        }
    }
}

pub fn load(path: &Path) -> Loaded {
    let mut loaded = Loaded::default();
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return loaded,
        Err(error) => {
            loaded.notes.push(format!("settings not read: {error}"));
            loaded.writable = false;
            return loaded;
        }
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        quarantine(path, &mut loaded);
        return loaded;
    };
    if value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|version| version > 1)
    {
        loaded.writable = false;
        loaded
            .notes
            .push("settings are from a newer kazoo: not saved this run".into());
        return loaded;
    }
    match serde_json::from_value::<Document>(value) {
        Ok(document) if document.version == 1 => document.apply(&mut loaded),
        Ok(_) | Err(_) => quarantine(path, &mut loaded),
    }
    loaded
}

fn quarantine(path: &Path, loaded: &mut Loaded) {
    let seconds = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(time) => time.as_secs(),
        Err(error) => {
            loaded
                .notes
                .push(format!("settings not kept aside: {error}"));
            loaded.writable = false;
            return;
        }
    };
    let bad = match quarantine_name(path, seconds) {
        Ok(bad) => bad,
        Err(error) => {
            loaded
                .notes
                .push(format!("settings not kept aside: {error}"));
            loaded.writable = false;
            return;
        }
    };
    match fs::rename(path, &bad) {
        Ok(()) => {
            loaded.notes.push(format!(
                "invalid settings kept as {}; using defaults",
                bad.display()
            ));
            match path
                .parent()
                .map(File::open)
                .transpose()
                .and_then(|directory| directory.map_or(Ok(()), |directory| directory.sync_all()))
            {
                Ok(()) => {}
                Err(error) => loaded
                    .notes
                    .push(format!("settings rename not synced: {error}")),
            }
        }
        Err(error) => {
            loaded
                .notes
                .push(format!("settings not kept aside: {error}"));
            loaded.writable = false;
        }
    }
}

fn quarantine_name(path: &Path, mut seconds: u64) -> io::Result<std::path::PathBuf> {
    loop {
        let mut bad = path.as_os_str().to_os_string();
        bad.push(format!(".bad-{seconds}"));
        let bad = std::path::PathBuf::from(bad);
        if !bad.try_exists()? {
            return Ok(bad);
        }
        // Two corrupt starts in one second must preserve both originals.
        seconds = seconds
            .checked_add(1)
            .ok_or_else(|| io::Error::other("settings quarantine suffix exhausted"))?;
    }
}

pub fn save(path: &Path, settings: &Settings) -> io::Result<()> {
    // Keep newer documents intact even if a caller misses Loaded::writable.
    match fs::read(path) {
        Ok(bytes) => {
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                if value
                    .get("version")
                    .and_then(serde_json::Value::as_u64)
                    .is_some_and(|version| version > 1)
                {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "settings are from a newer kazoo: not saved this run",
                    ));
                }
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid settings must be kept aside before saving",
                ));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return write_settings(path, settings);
        }
        Err(error) => return Err(error),
    }
    write_settings(path, settings)
}

fn write_settings(path: &Path, settings: &Settings) -> io::Result<()> {
    let document = Document::from(settings);
    atomic_write(path, |file| {
        serde_json::to_writer_pretty(file, &document).map_err(io::Error::other)
    })
}

pub fn atomic_write(
    path: &Path,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<()> {
    let directory = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "settings path needs a directory",
            )
        })?;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(0o700).create(directory)?;
    let mut temporary = path.as_os_str().to_os_string();
    temporary.push(format!(".tmp-{}", std::process::id()));
    let temporary = std::path::PathBuf::from(temporary);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    let result = write(&mut file)
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_all())
        .and_then(|()| fs::rename(&temporary, path))
        .and_then(|()| File::open(directory)?.sync_all());
    drop(file);
    if let Err(error) = result {
        match fs::remove_file(&temporary) {
            Ok(()) => return Err(error),
            Err(cleanup) if cleanup.kind() == io::ErrorKind::NotFound => return Err(error),
            Err(cleanup) => {
                return Err(io::Error::other(format!(
                    "{error}; temporary cleanup failed: {cleanup}"
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;
    use std::path::PathBuf;

    use serde_json::json;

    use super::{Document, Settings, Slot, atomic_write, load, save};

    fn fixture(name: &str) -> io::Result<PathBuf> {
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../target/kz-logs")
            .join(format!("mouth-settings-{name}"));
        if directory.exists() {
            fs::remove_dir_all(&directory)?;
        }
        fs::create_dir_all(&directory)?;
        Ok(directory.join("settings.json"))
    }

    #[test]
    fn settings_round_trip() -> io::Result<()> {
        let path = fixture("settings_round_trip")?;
        let mut settings = Settings::default();
        settings.quantise.scale = 3;
        settings.quantise.root = 7;
        settings.quantise.snap_ms = 123.0;
        settings.quantise.keep_vibrato = false;
        settings.quantise.hysteresis_cents = 23.0;
        settings.quantise.blend = 0.6;
        settings.last_blend = 0.6;
        settings.synth.shape = kazoo_core::synthesis::OscillatorShape::Triangle;
        settings.synth.cutoff_hz = 3200.0;
        settings.synth.air = 0.4;
        settings.synth.volume = 0.3;
        settings.own_bpm = 147.3;
        settings.loop_length = crate::engine::looper::LoopLength::Bars(4);
        settings.loop_volume = 0.35;
        settings.fx.order.swap(1, 2);
        for slot in Slot::ALL {
            let saved = &mut settings.fx.slots[slot as usize];
            saved.on = true;
            saved.active = slot.ids().len() - 1;
            for kind in 0..slot.ids().len() {
                for (index, spec) in slot.kind(kind).unwrap().params.iter().enumerate() {
                    saved.values[kind][index] = spec.clamp(spec.min.midpoint(spec.max));
                }
            }
        }
        save(&path, &settings)?;
        let loaded = load(&path);
        assert_eq!(loaded.settings, settings);
        assert!(loaded.notes.is_empty());
        assert!(loaded.writable);
        for entry in fs::read_dir(path.parent().unwrap())? {
            assert!(!entry?.file_name().to_string_lossy().contains(".tmp-"));
        }
        settings.silent = true;
        save(&path, &settings)?;
        assert!(!load(&path).settings.silent);
        let mut old: serde_json::Value = serde_json::from_slice(&fs::read(&path)?)?;
        assert!(old.as_object_mut().unwrap().remove("loop").is_some());
        fs::write(&path, serde_json::to_vec(&old)?)?;
        let migrated = load(&path);
        assert_eq!(
            migrated.settings.loop_length,
            crate::engine::looper::LoopLength::Free
        );
        assert!((migrated.settings.loop_volume - 0.8).abs() < f32::EPSILON);
        assert!(migrated.notes.is_empty());
        save(&path, &migrated.settings)?;
        let upgraded: serde_json::Value = serde_json::from_slice(&fs::read(&path)?)?;
        assert_eq!(upgraded["version"], 1);
        assert_eq!(upgraded["loop"]["length"], "free");
        Ok(())
    }

    #[test]
    fn a_corrupt_file_is_kept_aside_and_defaults_are_used() -> io::Result<()> {
        let path = fixture("a_corrupt_file_is_kept_aside_and_defaults_are_used")?;
        fs::write(&path, b"{not json")?;
        let loaded = load(&path);
        assert_eq!(loaded.settings, Settings::default());
        assert!(loaded.writable);
        assert!(!path.exists());
        let entries = fs::read_dir(path.parent().unwrap())?.collect::<io::Result<Vec<_>>>()?;
        assert_eq!(entries.len(), 1);
        let name = entries[0].file_name().to_string_lossy().into_owned();
        assert!(name.starts_with("settings.json.bad-"));
        assert!(loaded.notes.iter().any(|note| note.contains(&name)));
        assert_eq!(fs::read(entries[0].path())?, b"{not json");
        fs::write(&path, b"{\"version\":1,\"quantise\":false}")?;
        let loaded = load(&path);
        assert_eq!(loaded.settings, Settings::default());
        assert!(
            loaded
                .notes
                .iter()
                .any(|note| note.contains("settings.json.bad-"))
        );
        Ok(())
    }

    #[test]
    fn unknown_and_out_of_range_values_are_skipped_or_held() -> io::Result<()> {
        let path = fixture("unknown_and_out_of_range_values_are_skipped_or_held")?;
        let mut value =
            serde_json::to_value(Document::from(&Settings::default())).map_err(io::Error::other)?;
        value["fx"]["slots"]["distortion"]["kind"] = json!("unknown-drive");
        value["fx"]["slots"]["delay"]["kinds"]["digital"]["unknown-knob"] = json!(0.5);
        value["synth"]["cutoff_hz"] = json!(1e9);
        value["quantise"]["blend"] = json!(f32::NAN);
        value["fx"]["order"] = json!(["delay", "delay", "echo", "reverb", "megaphone"]);
        fs::write(&path, serde_json::to_vec(&value).map_err(io::Error::other)?)?;
        let loaded = load(&path);
        assert_eq!(loaded.settings.fx.slots[1].active, 0);
        assert_eq!(
            loaded.settings.synth.cutoff_hz.to_bits(),
            12_000.0_f32.to_bits()
        );
        assert_eq!(loaded.settings.quantise.blend.to_bits(), 0.0_f32.to_bits());
        assert_eq!(loaded.settings.fx.order, Slot::ALL);
        for name in [
            "unknown-drive",
            "unknown-knob",
            "cutoff_hz",
            "blend",
            "permutation",
        ] {
            assert_eq!(
                loaded
                    .notes
                    .iter()
                    .filter(|note| note.contains(name))
                    .count(),
                1,
                "{name}: {:?}",
                loaded.notes
            );
        }
        Ok(())
    }

    #[test]
    fn a_newer_version_is_never_overwritten() -> io::Result<()> {
        let path = fixture("a_newer_version_is_never_overwritten")?;
        let bytes = b"{\"version\":2,\"future\":true}";
        fs::write(&path, bytes)?;
        let loaded = load(&path);
        assert_eq!(loaded.settings, Settings::default());
        assert!(!loaded.writable);
        assert!(
            loaded
                .notes
                .iter()
                .any(|note| note == "settings are from a newer kazoo: not saved this run")
        );
        assert_eq!(
            save(&path, &loaded.settings).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(fs::read(&path)?, bytes);
        Ok(())
    }

    #[test]
    fn a_failed_atomic_write_keeps_the_old_file_and_removes_its_temporary() -> io::Result<()> {
        let path = fixture("a_failed_atomic_write")?;
        fs::write(&path, b"original")?;
        let error = atomic_write(&path, |file| {
            std::io::Write::write_all(file, b"partial")?;
            Err(io::Error::other("injected write failure"))
        })
        .unwrap_err();
        assert!(error.to_string().contains("injected write failure"));
        assert_eq!(fs::read(&path)?, b"original");
        assert_eq!(fs::read_dir(path.parent().unwrap())?.count(), 1);
        Ok(())
    }
}
