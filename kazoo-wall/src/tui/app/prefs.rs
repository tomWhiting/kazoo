//! What the console remembers between runs: the view, how densely the
//! rack is drawn, whether the log is put away and how big the picker is.
//! It lives in `console.json` in the wall's state folder, and is written
//! whenever one of them changes.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use kazoo_wall::paths;

use super::super::rack::geometry::Density;
use super::{App, Tone, View};

/// The file, in the wall's state folder.
const FILE: &str = "console.json";

/// The picker's smallest size, a percentage of the screen.
pub const PICKER_MIN: u8 = 40;

/// The picker's largest size: the whole screen.
pub const PICKER_MAX: u8 = 100;

/// What `<` and `>` change the picker's size by.
pub const PICKER_STEP: u8 = 10;

/// The remembered settings, as written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// The rack view rather than the list.
    pub rack: bool,
    /// `full`, `compact` or `overview`.
    pub density: Dense,
    /// The log put away in the rack view.
    pub log_hidden: bool,
    /// The picker's size, a percentage of the screen.
    pub picker_percent: u8,
}

/// [`Density`] as written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dense {
    /// [`Density::Full`].
    #[default]
    Full,
    /// [`Density::Compact`].
    Compact,
    /// [`Density::Overview`].
    Overview,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            rack: false,
            density: Dense::Full,
            log_hidden: false,
            picker_percent: PICKER_MAX,
        }
    }
}

/// Where the settings are kept.
///
/// # Errors
///
/// When there is no home folder to keep them in.
pub fn path() -> Result<PathBuf, String> {
    paths::state_dir()
        .map(|dir| dir.join(FILE))
        .map_err(|err| format!("no place to remember the console's settings: {err}"))
}

/// The settings in `file`; the defaults when there is no file yet.
///
/// # Errors
///
/// When the file cannot be read or is not settings.
pub fn load(file: &Path) -> Result<Prefs, String> {
    match fs::read_to_string(file) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|err| format!("{} is not the console's settings: {err}", file.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Prefs::default()),
        Err(err) => Err(format!("could not read {}: {err}", file.display())),
    }
}

/// Write `prefs` to `file`, whole: to a neighbour first, then renamed over.
///
/// # Errors
///
/// When the folder cannot be made or the file cannot be written.
pub fn save(file: &Path, prefs: Prefs) -> Result<(), String> {
    let fail = |err: &dyn std::fmt::Display| {
        format!(
            "could not remember the console's settings in {}: {err}",
            file.display()
        )
    };
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).map_err(|err| fail(&err))?;
    }
    let text = serde_json::to_string_pretty(&prefs).map_err(|err| fail(&err))?;
    let temp = file.with_extension("json.new");
    fs::write(&temp, text).map_err(|err| fail(&err))?;
    fs::rename(&temp, file).map_err(|err| fail(&err))
}

impl App {
    /// The settings as they are now.
    #[must_use]
    pub fn prefs(&self) -> Prefs {
        Prefs {
            rack: self.view == View::Rack,
            density: match self.rack.density {
                Density::Full => Dense::Full,
                Density::Compact => Dense::Compact,
                Density::Overview => Dense::Overview,
            },
            log_hidden: self.log_hidden,
            picker_percent: self.picker_percent,
        }
    }

    /// Take up remembered settings.
    pub fn apply_prefs(&mut self, prefs: Prefs) {
        self.view = if prefs.rack { View::Rack } else { View::List };
        self.rack.set_density(match prefs.density {
            Dense::Full => Density::Full,
            Dense::Compact => Density::Compact,
            Dense::Overview => Density::Overview,
        });
        self.log_hidden = prefs.log_hidden;
        self.picker_percent = prefs.picker_percent.clamp(PICKER_MIN, PICKER_MAX);
    }

    /// Say the settings could not be read or remembered.
    pub fn prefs_trouble(&mut self, why: String, now: Instant) {
        self.say(Tone::Trouble, why, now);
    }

    /// The picker's size, a percentage of the screen.
    #[must_use]
    pub const fn picker_percent(&self) -> u8 {
        self.picker_percent
    }

    /// Make the picker a step bigger or smaller.
    pub(super) fn resize_picker(&mut self, bigger: bool) {
        self.picker_percent = if bigger {
            self.picker_percent.saturating_add(PICKER_STEP)
        } else {
            self.picker_percent.saturating_sub(PICKER_STEP)
        }
        .clamp(PICKER_MIN, PICKER_MAX);
    }
}
