//! A limb's colour, from the wall's own settings.
//!
//! Every kind of module belongs to a family, and every family has its own
//! hue: voices amber, drums yellow, filters green, rhythm cyan, space blue,
//! modulation violet, tape and speech pink, drive red. Each module alive
//! is its own shade of its family's hue: turned to one side or the other
//! and lightened or darkened partly by its place (fixed for its life) and
//! partly by its settings, every knob and cable leaning it its own way by
//! how far it is turned. A moment is every module's shade together. So a
//! wall of oscillators and filters reads amber-green and one gone to
//! reverb and delay reads blue; one knob turned a little moves one shade
//! of the hundred or so a little, so the colour drifts rather than
//! jumping, and a wall put back to its settings comes back to exactly its
//! colour. The plumbing (out, mix, vca, scope, readout) has no hue of its
//! own. Mixed families make a softer colour, one family a vivid one. How
//! loud it was, when the moment kept its sound, makes it lighter or
//! darker.

use crate::history::Sound;

/// The families, in hue order.
pub const FAMILIES: usize = 8;

/// Each family's hue, degrees: voices, drums, filters, rhythm, space,
/// modulation, tape and speech, drive.
const HUES: [f64; FAMILIES] = [32.0, 58.0, 125.0, 178.0, 215.0, 270.0, 318.0, 356.0];

/// How far a module's shade turns from its family's hue, either way,
/// degrees: wide enough that neighbouring families' shades meet.
const SPREAD: f64 = 55.0;
/// How much of a module's shade is its place's own, the rest its
/// settings': enough that two modules of a kind differ, little enough that
/// turning a knob through its travel moves the shade across the spread.
const PLACE_SHARE: f64 = 0.4;
/// How far the settings lean a shade, before it is held to -1 to 1: a
/// module with every knob at an end leans about this far.
const SETTINGS_LEAN: f64 = 2.0;
/// A shade's length on the wheel, in the whole numbers a tint adds up.
const UNIT: f64 = 1000.0;
/// How far the modules' own light and dark move a moment's lightness.
const SHADE_LIGHT: f64 = 0.14;
/// How many agreeing shades make a moment's colour full: fewer are drawn
/// quieter.
const FULL_SHADES: f64 = 4.0;
/// Lightness with no sound kept.
const LIGHT: f64 = 0.6;

/// Unheard and unplugged: a moment with nothing but plumbing alive. Grey,
/// never black.
pub const PLAIN: (u8, u8, u8) = (140, 140, 136);

/// How many modules of each family are alive at a moment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Kinds(pub [u16; FAMILIES]);

impl Kinds {
    /// One more module of `kind`'s family, if it has one.
    pub fn add(&mut self, kind: &str) {
        if let Some(f) = family_of(kind) {
            self.0[f] = self.0[f].saturating_add(1);
        }
    }

    /// One fewer module of `kind`'s family; answers false when there was
    /// none to take away.
    pub fn remove(&mut self, kind: &str) -> bool {
        match family_of(kind) {
            Some(f) if self.0[f] > 0 => {
                self.0[f] -= 1;
                true
            }
            Some(_) => false,
            None => true,
        }
    }

    /// The modules counted, every family together.
    #[must_use]
    pub fn total(&self) -> u32 {
        self.0.iter().map(|&n| u32::from(n)).sum()
    }
}

/// Every coloured module alive at a moment, each its shade, added up.
///
/// Each shade is a point on the colour wheel. The sums are whole numbers,
/// so a module added and later taken away leaves exactly what was there
/// before it, across any number of moments.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tint {
    /// The shades' sum across the wheel.
    pub x: i64,
    /// And up it.
    pub y: i64,
    /// Their light and dark, summed.
    pub light: i64,
    /// How many shades are in it.
    pub count: i64,
}

impl Tint {
    /// Add the module at `path`, a `kind`, set as `settings` (`None` when
    /// they could not be read: then it is shaded by its place alone), if
    /// its kind has a family.
    pub fn add(&mut self, kind: &str, path: &str, settings: Option<Settings>) {
        if let Some((x, y, light)) = shade(kind, path, settings) {
            self.x += x;
            self.y += y;
            self.light += light;
            self.count += 1;
        }
    }

    /// Take away the module at `path` as it was when set as `settings`:
    /// exactly what adding it added.
    pub fn remove(&mut self, kind: &str, path: &str, settings: Option<Settings>) {
        if let Some((x, y, light)) = shade(kind, path, settings) {
            self.x -= x;
            self.y -= y;
            self.light -= light;
            self.count -= 1;
        }
    }

    /// Both tints together.
    #[must_use]
    pub const fn plus(&self, other: &Self) -> Self {
        Self {
            x: self.x + other.x,
            y: self.y + other.y,
            light: self.light + other.light,
            count: self.count + other.count,
        }
    }

    /// This tint without `other`'s.
    #[must_use]
    pub const fn minus(&self, other: &Self) -> Self {
        Self {
            x: self.x - other.x,
            y: self.y - other.y,
            light: self.light - other.light,
            count: self.count - other.count,
        }
    }
}

/// Where a module's settings lean its shade: each from -1 to 1, 0 for a
/// module at the middle of every knob with nothing plugged in.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Settings {
    /// Which way, and how far, its hue turns from its place's own.
    pub turn: f64,
    /// How much lighter (or darker) than its place's own.
    pub light: f64,
}

/// A module's settings from its file in the tree (`modules/<id>.toml`).
///
/// They are where each knob sits along its travel and what is plugged
/// into each input, and nothing else: not its name, its birth, its cables' numbers
/// or the seats' dye, which change without the sound changing. Every knob
/// and input leans the shade its own way, by how far it is from the middle
/// (or how much is plugged in), so a small turn is a small move and a knob
/// turned back is the shade it was. `None` when the file is not a module's
/// TOML.
#[must_use]
pub fn settings_of(text: &str) -> Option<Settings> {
    let table: toml::Table = match text.parse() {
        Ok(table) => table,
        Err(_) => return None,
    };
    let kind = table.get("kind")?.as_str()?;
    let spec = kazoo_wall::catalogue::registry()
        .kinds()
        .iter()
        .find(|spec| spec.name == kind);
    let mut lean = Settings::default();
    let mut terms = 0.0_f64;
    if let Some(knobs) = table.get("knobs").and_then(toml::Value::as_table) {
        for (name, value) in knobs {
            let Some(value) = number(value) else {
                continue;
            };
            let at = spec
                .and_then(|spec| spec.knobs.iter().find(|knob| knob.name == name))
                .map_or_else(
                    // A knob this tree does not know: still a share of
                    // its own, the further from zero the further along.
                    || 0.5 + value.atan() / std::f64::consts::PI,
                    |knob| f64::from(knob.normalise(value as f32)),
                );
            lean_by(&mut lean, kind, name, at - 0.5);
            terms += 1.0;
        }
    }
    if let Some(inputs) = table.get("inputs").and_then(toml::Value::as_table) {
        for (jack, input) in inputs {
            let (Some(from), Some(amount)) = (
                input.get("from").and_then(toml::Value::as_str),
                input.get("amount").and_then(number),
            ) else {
                continue;
            };
            lean_by(
                &mut lean,
                kind,
                &format!("{jack}<{from}"),
                amount.clamp(-1.0, 1.0) / 2.0,
            );
            terms += 1.0;
        }
    }
    if terms > 0.0 {
        // Many knobs together wander as far as a few: the sum's spread
        // grows with the square root of how many.
        let scale = SETTINGS_LEAN / terms.sqrt();
        lean.turn = (lean.turn * scale).clamp(-1.0, 1.0);
        lean.light = (lean.light * scale).clamp(-1.0, 1.0);
    }
    Some(lean)
}

/// A TOML number, whole or not.
fn number(value: &toml::Value) -> Option<f64> {
    value
        .as_float()
        .or_else(|| value.as_integer().map(|n| n as f64))
        .filter(|n| n.is_finite())
}

/// Lean `lean` by `by` along the knob or input `name` of `kind`'s own two
/// directions, the same on every machine.
fn lean_by(lean: &mut Settings, kind: &str, name: &str, by: f64) {
    let print = fingerprint(kind, name);
    lean.turn = (unit(print).mul_add(2.0, -1.0)).mul_add(by, lean.turn);
    lean.light = (unit(print >> 32).mul_add(2.0, -1.0)).mul_add(by, lean.light);
}

/// One module's shade: its family's hue turned by up to `SPREAD` either
/// way, and its light or dark, from its place (a fingerprint of its path,
/// fixed for its life) leaned by its `settings`. `None` for a kind with no
/// family.
fn shade(kind: &str, path: &str, settings: Option<Settings>) -> Option<(i64, i64, i64)> {
    let family = family_of(kind)?;
    let print = fingerprint(path, "");
    let lean = settings.unwrap_or_default();
    let turn = unit(print)
        .mul_add(2.0, -1.0)
        .mul_add(PLACE_SHARE, lean.turn * (1.0 - PLACE_SHARE))
        .clamp(-1.0, 1.0);
    let dark = unit(print >> 32)
        .mul_add(2.0, -1.0)
        .mul_add(PLACE_SHARE, lean.light * (1.0 - PLACE_SHARE))
        .clamp(-1.0, 1.0);
    let hue = turn.mul_add(SPREAD, HUES[family]);
    let (sin, cos) = hue.to_radians().sin_cos();
    Some((
        (cos * UNIT).round() as i64,
        (sin * UNIT).round() as i64,
        (dark * UNIT).round() as i64,
    ))
}

/// FNV-1a over `a`, a separator, and `b`: the same words always give the
/// same print, on every machine.
fn fingerprint(a: &str, b: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in a.bytes().chain([0]).chain(b.bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    // Mixed once more, so prints that differ in their last letters spread
    // across the whole range.
    hash ^= hash >> 33;
    hash = hash.wrapping_mul(0xff51_afd7_ed55_8ccd);
    hash ^ (hash >> 33)
}

/// The low 32 bits of `bits` as a share from 0 to 1.
fn unit(bits: u64) -> f64 {
    f64::from((bits & 0xffff_ffff) as u32) / f64::from(u32::MAX)
}

/// A module's kind from its id, as the wall made it: the id without its
/// number (`noiseperc12` is a `noiseperc`, `sampler12_3` a `sampler12`).
#[must_use]
pub fn kind_of(id: &str) -> &str {
    kazoo_wall::patch::split_id(id).map_or_else(
        || id.trim_end_matches(|c: char| c.is_ascii_digit()),
        |(kind, _)| kind,
    )
}

/// The family of a kind of module; `None` for the plumbing and for kinds
/// this tree does not know (a kind added to the wall after it), which
/// colour nothing rather than the wrong thing.
#[must_use]
pub fn family_of(kind: &str) -> Option<usize> {
    Some(match kind {
        "vco" | "digital" | "noise" | "wavetable" | "fm" | "osc" | "drone" | "pluck" => 0,
        "kick" | "hat" | "snare" | "clap" | "noiseperc" | "metal" | "shaker" | "tom" | "perc"
        | "cymbal" | "conga" | "rim" | "cowbell" | "membrane" | "modal" => 1,
        "vcf" | "resonator" | "formant" | "eq" | "filter" => 2,
        "seq" | "euclid" | "clock" | "burst" | "grids" | "arp" | "gate" | "divide" | "prob"
        | "poly" => 3,
        "hall" | "plate" | "reverb" | "spring" | "shimmer" | "delay" | "chorus" | "bbd"
        | "comp" | "phaser" | "flanger" | "trem" | "shift" | "sidestep" => 4,
        "lfo" | "env" | "slew" | "sh" | "math" | "quant" | "pitch" | "random" | "compare"
        | "logic" | "branch" | "switch" | "select" => 5,
        "tape" | "reel" | "speak" | "vocoder" | "radio" | "loop" | "mouth" | "sampler"
        | "sampler12" | "granular" | "vhs" | "cassette" | "genloss" => 6,
        "klon" | "fuzz" | "crush" | "amp" | "vinyl" | "drive" | "fold" | "screamer" | "ringmod" => {
            7
        }
        _ => return None,
    })
}

/// A moment's colour from its `tint`.
///
/// The hue is where its shades point together: vivid when they agree,
/// softer when they spread around the wheel or are few. It is lighter or
/// darker by its modules' own light and, when `sound` was kept, by how
/// loud it was.
#[must_use]
pub fn colour(tint: &Tint, sound: Option<&Sound>) -> (u8, u8, u8) {
    blend(tint, 1.0, sound.map(|sound| f64::from(sound.rms_db)))
}

/// The colour of `moments` moments whose tints add up to `sum`.
///
/// It is the colour of their mean tint, worked in fractions so a few
/// modules among many moments still count, lightened by their mean
/// loudness `rms_db` when any kept their sound.
#[must_use]
pub fn blend(sum: &Tint, moments: f64, rms_db: Option<f64>) -> (u8, u8, u8) {
    if sum.count <= 0 || moments.is_nan() || moments <= 0.0 {
        return PLAIN;
    }
    let (x, y) = (sum.x as f64, sum.y as f64);
    let count = sum.count as f64;
    let length = libm::hypot(x, y);
    let hue = y.atan2(x).to_degrees().rem_euclid(360.0);
    // One family alone is vivid; an even mix of all of them still has a
    // colour, only a softer one; and a moment of one or two modules, whose
    // colour any turn moves a long way, is drawn quieter than a full wall.
    let together = (length / (count * UNIT)).clamp(0.0, 1.0);
    let strength = (length / moments / (FULL_SHADES * UNIT)).clamp(0.0, 1.0);
    let saturation = together.mul_add(0.45, 0.4) * strength.mul_add(0.5, 0.5);
    // The modules' light and dark, as far from even as so many shades
    // drawn at random would wander: a few modules move it as much as many.
    let each = count / moments;
    let shades = (sum.light as f64 / moments / (each.sqrt() * UNIT)).clamp(-1.0, 1.0);
    let light = rms_db.map_or(LIGHT, |rms| {
        let rms = if rms.is_finite() { rms } else { -60.0 };
        ((rms + 50.0) / 40.0).clamp(0.0, 1.0).mul_add(0.3, 0.45)
    });
    hsl(hue, saturation, shades.mul_add(SHADE_LIGHT, light))
}

/// A colour from hue (degrees), saturation and lightness (0 to 1).
#[must_use]
pub fn hsl(hue: f64, saturation: f64, light: f64) -> (u8, u8, u8) {
    let hue = if hue.is_finite() {
        hue.rem_euclid(360.0)
    } else {
        0.0
    };
    let saturation = if saturation.is_finite() {
        saturation.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let light = if light.is_finite() {
        light.clamp(0.0, 1.0)
    } else {
        0.5
    };
    let chroma = (1.0 - 2.0_f64.mul_add(light, -1.0).abs()) * saturation;
    let sector = hue / 60.0;
    let second = chroma * (1.0 - (sector % 2.0 - 1.0).abs());
    let (red, green, blue) = if sector < 1.0 {
        (chroma, second, 0.0)
    } else if sector < 2.0 {
        (second, chroma, 0.0)
    } else if sector < 3.0 {
        (0.0, chroma, second)
    } else if sector < 4.0 {
        (0.0, second, chroma)
    } else if sector < 5.0 {
        (second, 0.0, chroma)
    } else {
        (chroma, 0.0, second)
    };
    let base = light - chroma / 2.0;
    (byte(red + base), byte(green + base), byte(blue + base))
}

/// A share from 0 to 1 as a byte.
fn byte(share: f64) -> u8 {
    (share.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests;
