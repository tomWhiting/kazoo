use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GlideCurve {
    #[default]
    Linear,
    EaseIn,
    EaseOut,
    SCurve,
}

impl GlideCurve {
    #[must_use]
    pub const fn is_linear(&self) -> bool {
        matches!(self, Self::Linear)
    }

    #[must_use]
    pub fn position(self, t: f32) -> f32 {
        match self {
            Self::Linear => t,
            Self::EaseIn => t * t * t,
            Self::EaseOut => 1.0 - (1.0 - t).powi(3),
            Self::SCurve => t * t * (2.0_f32.mul_add(-t, 3.0)),
        }
    }
}

#[must_use]
pub const fn is_zero(value: &f64) -> bool {
    *value == 0.0
}

impl super::Request {
    #[must_use]
    pub const fn turn(module: String, knob: String, value: f64, glide_beats: Option<f64>) -> Self {
        Self::Turn {
            module,
            knob,
            value,
            glide_beats,
            after_beats: 0.0,
            curve: GlideCurve::Linear,
        }
    }

    #[must_use]
    pub const fn patch(from: String, to: String, amount: Option<f64>) -> Self {
        Self::Patch {
            from,
            to,
            amount,
            after_beats: 0.0,
            fade_beats: 0.0,
            curve: None,
        }
    }

    #[must_use]
    pub const fn tempo(bpm: f64) -> Self {
        Self::Tempo {
            bpm,
            after_beats: 0.0,
            glide_beats: 0.0,
        }
    }

    #[must_use]
    pub const fn add(
        kind: String,
        name: Option<String>,
        place: Option<super::Place>,
        alias: Option<String>,
    ) -> Self {
        Self::Add {
            kind,
            name,
            place,
            alias,
            after_beats: None,
        }
    }

    #[must_use]
    pub const fn remove(module: String) -> Self {
        Self::Remove {
            module,
            after_beats: None,
        }
    }

    #[must_use]
    pub const fn unpatch(cable: Option<u32>, to: Option<String>) -> Self {
        Self::Unpatch {
            cable,
            to,
            after_beats: None,
        }
    }
}
