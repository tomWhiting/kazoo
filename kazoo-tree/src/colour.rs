//! A limb's colour, from the wall's own settings.
//!
//! Every kind of module belongs to a family, and every family has its own
//! hue: voices amber, drums yellow, filters green, rhythm cyan, space blue,
//! modulation violet, tape and speech pink, drive red. Each module alive
//! is its own shade of its family's hue, turned to one side or the other
//! and lightened or darkened by a fingerprint of its settings (its file in
//! the tree), and a moment is every module's shade together. So a wall of
//! oscillators and filters reads amber-green and one gone to reverb and
//! delay reads blue; one knob turned moves one module's shade of the
//! hundred or so, so the colour drifts from moment to moment rather than
//! jumping, and a wall put up again from its settings comes back to
//! exactly its colour. The plumbing (out, mix, vca, scope, readout) has no
//! hue of its own. Mixed families make a softer colour, one family a vivid
//! one. How loud it was, when the moment kept its sound, makes it lighter
//! or darker.

use crate::history::Sound;

/// The families, in hue order.
pub const FAMILIES: usize = 8;

/// Each family's hue, degrees: voices, drums, filters, rhythm, space,
/// modulation, tape and speech, drive.
const HUES: [f64; FAMILIES] = [32.0, 58.0, 125.0, 178.0, 215.0, 270.0, 318.0, 356.0];

/// The families' names, as the help and the details say them.
pub const NAMES: [&str; FAMILIES] = [
    "voices",
    "drums",
    "filters",
    "rhythm",
    "space",
    "modulation",
    "tape and speech",
    "drive",
];

/// How far a module's shade turns from its family's hue, either way,
/// degrees: wide enough that neighbouring families' shades meet.
const SPREAD: f64 = 55.0;
/// A shade's length on the wheel, in the whole numbers a tint adds up.
const UNIT: f64 = 1000.0;
/// How far the modules' own light and dark move a moment's lightness.
const SHADE_LIGHT: f64 = 0.14;
/// Lightness with no sound kept.
const LIGHT: f64 = 0.6;

/// Unheard and unplugged: a moment with nothing but plumbing alive. Grey,
/// never black.
pub const PLAIN: (u8, u8, u8) = (140, 140, 136);

/// How many modules of each family: alive at a moment, or touched by it.
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

    /// Both counts added, family by family.
    #[must_use]
    pub fn plus(&self, other: &Self) -> Self {
        let mut sum = *self;
        for (a, b) in sum.0.iter_mut().zip(other.0) {
            *a = a.saturating_add(b);
        }
        sum
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
    /// Add the module at `path`, a `kind`, whose settings' fingerprint is
    /// `blob` (git's id of its file), if its kind has a family.
    pub fn add(&mut self, kind: &str, path: &str, blob: &str) {
        if let Some((x, y, light)) = shade(kind, path, blob) {
            self.x += x;
            self.y += y;
            self.light += light;
            self.count += 1;
        }
    }

    /// Take away the module at `path` as it was when `blob` was its
    /// settings: exactly what adding it added.
    pub fn remove(&mut self, kind: &str, path: &str, blob: &str) {
        if let Some((x, y, light)) = shade(kind, path, blob) {
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

    /// The mean of `moments` tints added up into this one, each sum
    /// divided and rounded to the nearest whole number; this tint as it is
    /// for none or one.
    #[must_use]
    pub fn mean(&self, moments: i64) -> Self {
        if moments <= 1 {
            return *self;
        }
        let share = |sum: i64| {
            let half = moments / 2;
            if sum >= 0 {
                (sum + half) / moments
            } else {
                (sum - half) / moments
            }
        };
        Self {
            x: share(self.x),
            y: share(self.y),
            light: share(self.light),
            count: share(self.count),
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

/// One module's shade: its family's hue turned by up to `SPREAD` either
/// way, and its light or dark, both from a fingerprint of where it is and
/// what its settings are. `None` for a kind with no family.
fn shade(kind: &str, path: &str, blob: &str) -> Option<(i64, i64, i64)> {
    let family = family_of(kind)?;
    let print = fingerprint(path, blob);
    let turn = unit(print);
    let dark = unit(print >> 32);
    let hue = turn.mul_add(2.0 * SPREAD, HUES[family] - SPREAD);
    let (sin, cos) = hue.to_radians().sin_cos();
    Some((
        (cos * UNIT).round() as i64,
        (sin * UNIT).round() as i64,
        (dark.mul_add(2.0, -1.0) * UNIT).round() as i64,
    ))
}

/// FNV-1a over `path`, a separator, and `blob`: the same settings at the
/// same place always give the same print, on every machine.
fn fingerprint(path: &str, blob: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.bytes().chain([0]).chain(blob.bytes()) {
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
/// softer when they spread around the wheel. It is lighter or darker by
/// its modules' own light and, when `sound` was kept, by how loud it was.
#[must_use]
pub fn colour(tint: &Tint, sound: Option<&Sound>) -> (u8, u8, u8) {
    if tint.count <= 0 {
        return PLAIN;
    }
    let (x, y) = (tint.x as f64, tint.y as f64);
    let count = tint.count as f64;
    let hue = y.atan2(x).to_degrees().rem_euclid(360.0);
    // One family alone is vivid; an even mix of all of them still has a
    // colour, only a softer one.
    let together = (libm::hypot(x, y) / (count * UNIT)).clamp(0.0, 1.0);
    let saturation = together.mul_add(0.45, 0.4);
    // The modules' light and dark, as far from even as so many shades
    // drawn at random would wander: a few modules move it as much as many.
    let shades = (tint.light as f64 / (count.sqrt() * UNIT)).clamp(-1.0, 1.0);
    let light = sound.map_or(LIGHT, |sound| {
        let rms = if sound.rms_db.is_finite() {
            f64::from(sound.rms_db)
        } else {
            -60.0
        };
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
