//! Preset save/load.
//!
//! Presets are JSON files under `$HOME/.config/kazoo-cs80/presets`. Saving is
//! atomic (write a temporary file, then rename). A loaded preset is checked
//! field by field before it can reach the synth, so a hand-edited or corrupt
//! file cannot inject NaN or out-of-range values into the audio path.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::synth::SynthParams;

/// File name of the quick-save preset.
const PRESET_FILE: &str = "last_preset.json";

/// Why a preset could not be saved or loaded.
#[derive(Debug)]
pub enum PresetError {
    /// `$HOME` is not set, so the preset directory cannot be located.
    NoHome,
    /// There is no saved preset yet.
    NotFound(PathBuf),
    /// Reading, writing or creating the directory failed.
    Io(PathBuf, std::io::Error),
    /// The file is not a valid preset.
    Parse(PathBuf, serde_json::Error),
    /// The parameters could not be serialised.
    Serialize(serde_json::Error),
    /// A value is not finite or outside the range the synth accepts.
    OutOfRange {
        field: &'static str,
        value: f32,
        min: f32,
        max: f32,
    },
}

impl fmt::Display for PresetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoHome => write!(f, "HOME is not set, so presets have nowhere to live"),
            Self::NotFound(path) => write!(f, "no preset saved yet ({})", path.display()),
            Self::Io(path, err) => write!(f, "{}: {err}", path.display()),
            Self::Parse(path, err) => write!(f, "{} is not a valid preset: {err}", path.display()),
            Self::Serialize(err) => write!(f, "could not encode preset: {err}"),
            Self::OutOfRange {
                field,
                value,
                min,
                max,
            } => write!(f, "preset {field} = {value} is outside {min}..={max}"),
        }
    }
}

impl std::error::Error for PresetError {}

/// The preset directory for this user.
pub fn preset_dir() -> Result<PathBuf, PresetError> {
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .ok_or(PresetError::NoHome)?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("kazoo-cs80")
        .join("presets"))
}

/// Save `params` as the quick-save preset in `dir`, creating `dir` if
/// needed. Returns the path written.
pub fn save_to(dir: &Path, params: &SynthParams) -> Result<PathBuf, PresetError> {
    let json = serde_json::to_string_pretty(params).map_err(PresetError::Serialize)?;
    std::fs::create_dir_all(dir).map_err(|e| PresetError::Io(dir.to_path_buf(), e))?;
    let path = dir.join(PRESET_FILE);
    let tmp = dir.join(format!("{PRESET_FILE}.tmp"));
    std::fs::write(&tmp, json).map_err(|e| PresetError::Io(tmp.clone(), e))?;
    std::fs::rename(&tmp, &path).map_err(|e| PresetError::Io(path.clone(), e))?;
    Ok(path)
}

/// Load and validate the quick-save preset from `dir`.
pub fn load_from(dir: &Path) -> Result<SynthParams, PresetError> {
    let path = dir.join(PRESET_FILE);
    let data = match std::fs::read_to_string(&path) {
        Ok(data) => data,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(PresetError::NotFound(path));
        }
        Err(e) => return Err(PresetError::Io(path, e)),
    };
    let params: SynthParams =
        serde_json::from_str(&data).map_err(|e| PresetError::Parse(path, e))?;
    validate(&params)?;
    Ok(params)
}

/// A named parameter value and the range the editor allows for it.
type Range = (&'static str, f32, f32, f32);

/// Layer I parameters.
const fn layer1_ranges(p: &SynthParams) -> [Range; 17] {
    [
        ("layer1_fine_tune", p.layer1_fine_tune, -100.0, 100.0),
        ("layer1_pulse_width", p.layer1_pulse_width, 0.5, 0.9),
        ("layer1_hpf_cutoff", p.layer1_hpf_cutoff, 20.0, 20_000.0),
        ("layer1_hpf_resonance", p.layer1_hpf_resonance, 0.0, 0.95),
        ("layer1_lpf_cutoff", p.layer1_lpf_cutoff, 20.0, 20_000.0),
        ("layer1_lpf_resonance", p.layer1_lpf_resonance, 0.0, 0.95),
        ("layer1_filter_env_il", p.layer1_filter_env_il, 0.0, 1.0),
        ("layer1_filter_env_al", p.layer1_filter_env_al, 0.0, 1.0),
        (
            "layer1_filter_env_attack",
            p.layer1_filter_env_attack,
            0.001,
            10.0,
        ),
        (
            "layer1_filter_env_decay",
            p.layer1_filter_env_decay,
            0.001,
            10.0,
        ),
        (
            "layer1_filter_env_release",
            p.layer1_filter_env_release,
            0.001,
            10.0,
        ),
        (
            "layer1_filter_env_depth",
            p.layer1_filter_env_depth,
            0.0,
            20_000.0,
        ),
        ("layer1_vca_attack", p.layer1_vca_attack, 0.001, 10.0),
        ("layer1_vca_decay", p.layer1_vca_decay, 0.001, 10.0),
        ("layer1_vca_sustain", p.layer1_vca_sustain, 0.0, 1.0),
        ("layer1_vca_release", p.layer1_vca_release, 0.001, 10.0),
        ("layer1_level", p.layer1_level, 0.0, 1.0),
    ]
}

/// Layer II parameters.
const fn layer2_ranges(p: &SynthParams) -> [Range; 17] {
    [
        ("layer2_fine_tune", p.layer2_fine_tune, -100.0, 100.0),
        ("layer2_pulse_width", p.layer2_pulse_width, 0.5, 0.9),
        ("layer2_hpf_cutoff", p.layer2_hpf_cutoff, 20.0, 20_000.0),
        ("layer2_hpf_resonance", p.layer2_hpf_resonance, 0.0, 0.95),
        ("layer2_lpf_cutoff", p.layer2_lpf_cutoff, 20.0, 20_000.0),
        ("layer2_lpf_resonance", p.layer2_lpf_resonance, 0.0, 0.95),
        ("layer2_filter_env_il", p.layer2_filter_env_il, 0.0, 1.0),
        ("layer2_filter_env_al", p.layer2_filter_env_al, 0.0, 1.0),
        (
            "layer2_filter_env_attack",
            p.layer2_filter_env_attack,
            0.001,
            10.0,
        ),
        (
            "layer2_filter_env_decay",
            p.layer2_filter_env_decay,
            0.001,
            10.0,
        ),
        (
            "layer2_filter_env_release",
            p.layer2_filter_env_release,
            0.001,
            10.0,
        ),
        (
            "layer2_filter_env_depth",
            p.layer2_filter_env_depth,
            0.0,
            20_000.0,
        ),
        ("layer2_vca_attack", p.layer2_vca_attack, 0.001, 10.0),
        ("layer2_vca_decay", p.layer2_vca_decay, 0.001, 10.0),
        ("layer2_vca_sustain", p.layer2_vca_sustain, 0.0, 1.0),
        ("layer2_vca_release", p.layer2_vca_release, 0.001, 10.0),
        ("layer2_level", p.layer2_level, 0.0, 1.0),
    ]
}

/// Ring mod, LFO and mixer parameters.
const fn shared_ranges(p: &SynthParams) -> [Range; 11] {
    [
        ("ring_mod_depth", p.ring_mod_depth, 0.0, 1.0),
        (
            "ring_mod_carrier_freq",
            p.ring_mod_carrier_freq,
            20.0,
            5000.0,
        ),
        ("ring_mod_attack", p.ring_mod_attack, 0.0005, 1.0),
        ("ring_mod_decay", p.ring_mod_decay, 0.001, 10.0),
        ("lfo_rate", p.lfo_rate, 0.01, 100.0),
        (
            "lfo_routing.pitch_cents",
            p.lfo_routing.pitch_cents,
            0.0,
            100.0,
        ),
        (
            "lfo_routing.filter_depth",
            p.lfo_routing.filter_depth,
            0.0,
            1.0,
        ),
        ("lfo_routing.vca_depth", p.lfo_routing.vca_depth, 0.0, 1.0),
        ("layer_mix", p.layer_mix, 0.0, 1.0),
        ("master_level", p.master_level, 0.0, 1.0),
        ("drift_cents", p.drift_cents, 0.0, 10.0),
    ]
}

/// Check every continuous parameter is finite and within the range the
/// editor allows.
pub fn validate(p: &SynthParams) -> Result<(), PresetError> {
    let all = layer1_ranges(p)
        .into_iter()
        .chain(layer2_ranges(p))
        .chain(shared_ranges(p));
    for (field, value, min, max) in all {
        // `contains` is false for NaN, and infinities are out of range.
        if !(min..=max).contains(&value) {
            return Err(PresetError::OutOfRange {
                field,
                value,
                min,
                max,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, Section};

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("kazoo-cs80-preset-{name}-{}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        dir
    }

    #[test]
    fn defaults_are_valid() {
        validate(&SynthParams::default()).unwrap();
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = temp_dir("roundtrip");
        let params = SynthParams {
            master_level: 0.4,
            layer2_lpf_cutoff: 1234.0,
            ..SynthParams::default()
        };
        let path = save_to(&dir, &params).unwrap();
        assert!(path.exists());
        assert!(!dir.join(format!("{PRESET_FILE}.tmp")).exists());

        let loaded = load_from(&dir).unwrap();
        assert!((loaded.master_level - 0.4).abs() < f32::EPSILON);
        assert!((loaded.layer2_lpf_cutoff - 1234.0).abs() < f32::EPSILON);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_preset_is_reported() {
        let dir = temp_dir("missing");
        assert!(matches!(load_from(&dir), Err(PresetError::NotFound(_))));
    }

    #[test]
    fn corrupt_preset_is_reported() {
        let dir = temp_dir("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(PRESET_FILE), "{ not json").unwrap();
        assert!(matches!(load_from(&dir), Err(PresetError::Parse(..))));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn out_of_range_preset_is_rejected() {
        let dir = temp_dir("range");
        let params = SynthParams {
            layer1_lpf_resonance: 4.0,
            ..SynthParams::default()
        };
        save_to(&dir, &params).unwrap();
        match load_from(&dir) {
            Err(PresetError::OutOfRange { field, .. }) => {
                assert_eq!(field, "layer1_lpf_resonance");
            }
            other => panic!("expected range error, got {other:?}"),
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn non_finite_value_is_rejected() {
        let mut params = SynthParams {
            lfo_rate: f32::NAN,
            ..SynthParams::default()
        };
        assert!(matches!(
            validate(&params),
            Err(PresetError::OutOfRange {
                field: "lfo_rate",
                ..
            })
        ));
        params.lfo_rate = f32::INFINITY;
        assert!(validate(&params).is_err());
    }

    /// The validation ranges must accept everything the editor can produce:
    /// drive every parameter to both ends and check the result.
    #[test]
    fn editor_extremes_are_valid() {
        let sections = [
            Section::Layer1,
            Section::Layer2,
            Section::RingMod,
            Section::Lfo,
            Section::Mixer,
        ];
        for up in [true, false] {
            let mut app = App::new(44100.0);
            for section in sections {
                while app.section != section {
                    app.next_section();
                }
                for idx in 0..section.param_count() {
                    app.param_index = idx;
                    for _ in 0..3000 {
                        if up {
                            app.increment_param();
                        } else {
                            app.decrement_param();
                        }
                    }
                }
            }
            validate(&app.synth.params).unwrap();
        }
    }
}
