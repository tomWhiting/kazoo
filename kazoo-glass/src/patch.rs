//! Normalized front-panel controls and six factory voicings.

/// One editable patch control, in panel order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamField {
    Decay,
    Damping,
    Material,
    Brightness,
    Position,
    Width,
    Release,
}

impl ParamField {
    pub const ALL: [Self; 7] = [
        Self::Decay,
        Self::Damping,
        Self::Material,
        Self::Brightness,
        Self::Position,
        Self::Width,
        Self::Release,
    ];
    pub const STEP: f32 = 0.02;

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Decay => "ring time",
            Self::Damping => "treble damping",
            Self::Material => "metal / glass",
            Self::Brightness => "mallet hardness",
            Self::Position => "strike position",
            Self::Width => "stereo width",
            Self::Release => "key damping",
        }
    }

    #[must_use]
    pub const fn hint(self) -> &'static str {
        match self {
            Self::Decay => "how long the fundamental rings (time to -60 dB)",
            Self::Damping => "upper modes die faster as damping increases",
            Self::Material => "harmonic tine at zero; inharmonic glass/bell modes at one",
            Self::Brightness => "a harder mallet excites more high-frequency modes",
            Self::Position => "where the mallet hits: changes which modes are excited",
            Self::Width => "spreads resonances across stereo; zero is mono",
            Self::Release => "ring time after key release (never longer than held decay)",
        }
    }
}

/// A complete voicing. Numeric controls are normalized to 0..=1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Patch {
    pub name: &'static str,
    pub decay: f32,
    pub damping: f32,
    pub material: f32,
    pub brightness: f32,
    pub position: f32,
    pub width: f32,
    pub release: f32,
}

impl Patch {
    #[must_use]
    pub const fn get(&self, field: ParamField) -> f32 {
        match field {
            ParamField::Decay => self.decay,
            ParamField::Damping => self.damping,
            ParamField::Material => self.material,
            ParamField::Brightness => self.brightness,
            ParamField::Position => self.position,
            ParamField::Width => self.width,
            ParamField::Release => self.release,
        }
    }

    /// Ignore non-finite edits; clamp other edits to the panel range.
    pub const fn set(&mut self, field: ParamField, value: f32) {
        if !value.is_finite() {
            return;
        }
        let value = value.clamp(0.0, 1.0);
        match field {
            ParamField::Decay => self.decay = value,
            ParamField::Damping => self.damping = value,
            ParamField::Material => self.material = value,
            ParamField::Brightness => self.brightness = value,
            ParamField::Position => self.position = value,
            ParamField::Width => self.width = value,
            ParamField::Release => self.release = value,
        }
    }

    #[must_use]
    pub fn sanitized(mut self) -> Self {
        for field in ParamField::ALL {
            let value = self.get(field);
            self.set(field, if value.is_finite() { value } else { 0.5 });
        }
        self
    }

    #[must_use]
    pub fn decay_seconds(&self) -> f32 {
        exp_map(self.decay, 0.2, 16.0)
    }

    #[must_use]
    pub fn release_seconds(&self) -> f32 {
        exp_map(self.release, 0.04, 8.0)
    }
}

fn exp_map(value: f32, min: f32, max: f32) -> f32 {
    min * (max / min).powf(value.clamp(0.0, 1.0))
}

const fn patch(name: &'static str, p: [f32; 7]) -> Patch {
    Patch {
        name,
        decay: p[0],
        damping: p[1],
        material: p[2],
        brightness: p[3],
        position: p[4],
        width: p[5],
        release: p[6],
    }
}

/// Tab or 1-6 loads a sound. No samples are used.
pub const PATCHES: [Patch; 6] = [
    patch("Wine Glass", [0.72, 0.40, 0.87, 0.42, 0.30, 0.65, 0.86]),
    patch("Celesta", [0.40, 0.65, 0.12, 0.50, 0.40, 0.30, 0.30]),
    patch("Temple Bell", [0.90, 0.20, 1.00, 0.80, 0.20, 0.80, 1.00]),
    patch("Soft Tines", [0.55, 0.80, 0.00, 0.20, 0.55, 0.25, 0.55]),
    patch("Ice Chimes", [0.65, 0.12, 0.65, 1.00, 0.08, 1.00, 0.85]),
    patch("Muted Bronze", [0.30, 0.72, 1.00, 0.60, 0.72, 0.40, 0.10]),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_patch_values_are_repaired() {
        let mut patch = PATCHES[0];
        patch.decay = f32::NAN;
        patch.material = f32::INFINITY;
        patch.width = -3.0;
        let patch = patch.sanitized();
        for field in ParamField::ALL {
            assert!((0.0..=1.0).contains(&patch.get(field)));
        }
        assert!(patch.width.abs() < f32::EPSILON);
    }

    #[test]
    fn factory_controls_are_valid() {
        for patch in PATCHES {
            for field in ParamField::ALL {
                assert!((0.0..=1.0).contains(&patch.get(field)));
            }
            assert!(patch.decay_seconds() > 0.0 && patch.release_seconds() > 0.0);
        }
    }
}
