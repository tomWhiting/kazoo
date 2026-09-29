//! The string's controls and its factory sounds.
//!
//! Every control is a plain 0 to 1 value, so the front panel, the patch
//! table and the engine all speak the same units. The engine maps them to
//! seconds, blends and coefficients.

/// One editable control of a patch, in panel order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamField {
    /// How long the string rings while held.
    Decay,
    /// How quickly treble dies away: high is dark and woody.
    Damping,
    /// Where along the string it is plucked: low is near the bridge (nasal).
    Position,
    /// The pick: low is a soft fingertip, high a hard plectrum.
    Hardness,
    /// Wire stiffness: upper partials run sharp, as in a piano string or bar.
    Stiffness,
    /// How much of the instrument's wooden body rings along.
    Body,
    /// How fast the string stops when the key is let go.
    Release,
}

impl ParamField {
    /// Every control, top to bottom on the panel.
    pub const ALL: [Self; 7] = [
        Self::Decay,
        Self::Damping,
        Self::Position,
        Self::Hardness,
        Self::Stiffness,
        Self::Body,
        Self::Release,
    ];

    /// Panel label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Decay => "decay",
            Self::Damping => "damping",
            Self::Position => "pick position",
            Self::Hardness => "pick hardness",
            Self::Stiffness => "stiffness",
            Self::Body => "body",
            Self::Release => "release",
        }
    }

    /// One-line description shown under the panel while the control is selected.
    #[must_use]
    pub const fn hint(self) -> &'static str {
        match self {
            Self::Decay => "how long the string rings while the key is held",
            Self::Damping => "treble loss per pass: dark and woody when high, glassy when low",
            Self::Position => "near the bridge is nasal, mid-string is hollow and round",
            Self::Hardness => "soft fingertip when low, hard plectrum when high",
            Self::Stiffness => "sharpens upper partials: piano wire, kalimba tine, bell",
            Self::Body => "wooden body resonances (air, top plate, back) ringing with the string",
            Self::Release => "how fast the string is stopped when the key is released",
        }
    }

    /// Size of one `-` / `=` step, the same for every control.
    pub const STEP: f32 = 0.02;
}

/// A complete string voicing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Patch {
    pub name: &'static str,
    pub decay: f32,
    pub damping: f32,
    pub position: f32,
    pub hardness: f32,
    pub stiffness: f32,
    pub body: f32,
    pub release: f32,
}

impl Patch {
    /// The value of one control.
    #[must_use]
    pub const fn get(&self, field: ParamField) -> f32 {
        match field {
            ParamField::Decay => self.decay,
            ParamField::Damping => self.damping,
            ParamField::Position => self.position,
            ParamField::Hardness => self.hardness,
            ParamField::Stiffness => self.stiffness,
            ParamField::Body => self.body,
            ParamField::Release => self.release,
        }
    }

    /// Set one control, clamped to 0..=1. A non-finite value is ignored.
    pub const fn set(&mut self, field: ParamField, value: f32) {
        if !value.is_finite() {
            return;
        }
        let value = value.clamp(0.0, 1.0);
        match field {
            ParamField::Decay => self.decay = value,
            ParamField::Damping => self.damping = value,
            ParamField::Position => self.position = value,
            ParamField::Hardness => self.hardness = value,
            ParamField::Stiffness => self.stiffness = value,
            ParamField::Body => self.body = value,
            ParamField::Release => self.release = value,
        }
    }

    /// A copy with every control finite and inside 0..=1.
    #[must_use]
    pub fn sanitized(mut self) -> Self {
        for field in ParamField::ALL {
            let value = self.get(field);
            self.set(field, if value.is_finite() { value } else { 0.5 });
        }
        self
    }

    /// Ring time to 60 dB down while held, in seconds.
    #[must_use]
    pub fn decay_seconds(&self) -> f32 {
        exp_map(self.decay, 0.15, 25.0)
    }

    /// Ring time to 60 dB down once released, in seconds.
    #[must_use]
    pub fn release_seconds(&self) -> f32 {
        exp_map(self.release, 0.03, 1.5)
    }

    /// Averager blend in the loop, 0 (bright) to 0.5 (dark).
    #[must_use]
    pub fn blend(&self) -> f32 {
        self.damping.clamp(0.0, 1.0) * 0.5
    }

    /// Pick position along the string, 0.02 (bridge) to 0.5 (middle).
    #[must_use]
    pub fn pick_position(&self) -> f32 {
        self.position.clamp(0.0, 1.0).mul_add(0.48, 0.02)
    }

    /// Exciter brightness for a note of the given velocity (0 to 1): a
    /// harder blow is brighter.
    #[must_use]
    pub fn pick_brightness(&self, velocity: f32) -> f32 {
        let hardness = self.hardness.clamp(0.0, 1.0);
        let force = if velocity.is_finite() {
            velocity.clamp(0.0, 1.0)
        } else {
            0.5
        };
        (0.94 * hardness)
            .mul_add(0.55_f32.mul_add(force, 0.45), 0.06)
            .clamp(0.02, 1.0)
    }
}

/// Map 0..=1 exponentially onto `min..=max`.
#[must_use]
pub fn exp_map(value: f32, min: f32, max: f32) -> f32 {
    let value = if value.is_finite() { value } else { 0.0 };
    min * (max / min).powf(value.clamp(0.0, 1.0))
}

const fn patch(
    name: &'static str,
    [decay, damping, position, hardness, stiffness, body, release]: [f32; 7],
) -> Patch {
    Patch {
        name,
        decay,
        damping,
        position,
        hardness,
        stiffness,
        body,
        release,
    }
}

/// Factory sounds. Tab and 1-6 load them.
pub const PATCHES: [Patch; 6] = [
    patch("Nylon", [0.55, 0.55, 0.30, 0.35, 0.05, 0.70, 0.35]),
    patch("Steel", [0.65, 0.30, 0.22, 0.75, 0.10, 0.50, 0.35]),
    patch("Harp", [0.70, 0.40, 0.15, 0.55, 0.00, 0.25, 0.50]),
    patch("Pizzicato", [0.40, 0.65, 0.45, 0.30, 0.00, 0.80, 0.15]),
    patch("Kalimba", [0.45, 0.25, 0.50, 0.90, 0.90, 0.20, 0.30]),
    patch("Dulcimer", [0.85, 0.15, 0.10, 0.90, 0.25, 0.35, 0.60]),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_patches_are_in_range_and_distinct() {
        for (i, p) in PATCHES.iter().enumerate() {
            for field in ParamField::ALL {
                assert!((0.0..=1.0).contains(&p.get(field)), "{} {field:?}", p.name);
            }
            for q in &PATCHES[i + 1..] {
                assert_ne!(p.name, q.name);
                assert_ne!(p, q);
            }
        }
    }

    #[test]
    fn set_clamps_and_ignores_non_finite() {
        let mut p = PATCHES[0];
        p.set(ParamField::Decay, 7.0);
        assert!((p.decay - 1.0).abs() < f32::EPSILON);
        p.set(ParamField::Decay, -3.0);
        assert!(p.decay.abs() < f32::EPSILON);
        p.set(ParamField::Decay, 0.4);
        p.set(ParamField::Decay, f32::NAN);
        assert!((p.decay - 0.4).abs() < f32::EPSILON);
    }

    #[test]
    fn sanitized_repairs_hostile_patches() {
        let mut p = PATCHES[1];
        p.decay = f32::NAN;
        p.damping = f32::INFINITY;
        p.body = -9.0;
        let clean = p.sanitized();
        for field in ParamField::ALL {
            assert!((0.0..=1.0).contains(&clean.get(field)), "{field:?}");
        }
        assert!((clean.decay - 0.5).abs() < f32::EPSILON);
        assert!(clean.body.abs() < f32::EPSILON);
    }

    #[test]
    fn mappings_hit_their_ends() {
        let mut p = PATCHES[0];
        p.decay = 0.0;
        assert!((p.decay_seconds() - 0.15).abs() < 1.0e-4);
        p.decay = 1.0;
        assert!((p.decay_seconds() - 25.0).abs() < 1.0e-2);
        p.release = 0.0;
        assert!((p.release_seconds() - 0.03).abs() < 1.0e-5);
        p.damping = 1.0;
        assert!((p.blend() - 0.5).abs() < f32::EPSILON);
        p.position = 0.0;
        assert!((p.pick_position() - 0.02).abs() < f32::EPSILON);
        p.position = 1.0;
        assert!((p.pick_position() - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn harder_blows_are_brighter() {
        let p = PATCHES[1];
        assert!(p.pick_brightness(1.0) > p.pick_brightness(0.2));
        assert!(p.pick_brightness(f32::NAN) >= 0.02);
    }

    #[test]
    fn labels_and_hints_are_filled_in() {
        for field in ParamField::ALL {
            assert!(!field.label().is_empty());
            assert!(field.hint().len() > 10);
        }
    }
}
