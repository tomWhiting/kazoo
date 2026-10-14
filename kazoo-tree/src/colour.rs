//! A limb's colour, from the wall's own settings.
//!
//! Every kind of module belongs to a family, and every family has its own
//! hue: voices amber, drums yellow, filters green, rhythm cyan, space blue,
//! modulation violet, tape and speech pink, drive red. A moment is the
//! blend of the families alive in it, so a wall of oscillators and filters
//! reads amber-green and one gone to reverb and delay reads blue, and the
//! blend leans toward what was being played with (the families whose
//! modules were turned or added) along the step so far. The plumbing (out,
//! mix, vca, scope, readout) has no hue of its own. Mixed families make a
//! softer colour, one family a vivid one. How loud it was, when the moment
//! kept its sound, makes it lighter or darker. The same settings always
//! give the same colour.

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

/// How much what was played with counts against what is alive: at most
/// this share of the alive modules' weight.
const PLAYED: f64 = 0.6;
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

/// A moment's colour: the blend of the families `alive`, leaning toward
/// those `played` with, lighter when `sound` says it was louder.
#[must_use]
pub fn colour(alive: &Kinds, played: &Kinds, sound: Option<&Sound>) -> (u8, u8, u8) {
    let alive_weight = f64::from(alive.total());
    let played_total = f64::from(played.total());
    let played_scale = if played_total > 0.0 {
        PLAYED * alive_weight.max(1.0) / played_total
    } else {
        0.0
    };
    let (mut x, mut y, mut weight) = (0.0_f64, 0.0_f64, 0.0_f64);
    for ((hue, alive), played) in HUES.iter().zip(alive.0).zip(played.0) {
        let w = f64::from(played).mul_add(played_scale, f64::from(alive));
        if w > 0.0 {
            let (sin, cos) = hue.to_radians().sin_cos();
            x = cos.mul_add(w, x);
            y = sin.mul_add(w, y);
            weight += w;
        }
    }
    if weight <= 0.0 {
        return PLAIN;
    }
    let hue = y.atan2(x).to_degrees().rem_euclid(360.0);
    // One family alone is vivid; an even mix of all of them still has a
    // colour, only a softer one.
    let together = (libm::hypot(x, y) / weight).clamp(0.0, 1.0);
    let saturation = together.mul_add(0.4, 0.45);
    let light = sound.map_or(LIGHT, |sound| {
        let rms = if sound.rms_db.is_finite() {
            f64::from(sound.rms_db)
        } else {
            -60.0
        };
        ((rms + 50.0) / 40.0).clamp(0.0, 1.0).mul_add(0.3, 0.45)
    });
    hsl(hue, saturation, light)
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
