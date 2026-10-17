//! When a change's sound starts and how a knob moves (KZ-T12): the curve
//! a turn glides along, and the constructors that make untimed requests.

use serde::{Deserialize, Serialize};

/// The shape a knob glides along, over t from 0 to 1 of its glide, in knob
/// position (the 0 to 1 travel [`crate::catalogue::KnobSpec::normalise`]
/// gives).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GlideCurve {
    /// t: a straight line, as every glide before KZ-T12.
    #[default]
    Linear,
    /// t³: slow, then fast (a swell).
    EaseIn,
    /// 1 − (1 − t)³: fast, then settling in.
    EaseOut,
    /// 3t² − 2t³: slow at both ends.
    SCurve,
}

impl GlideCurve {
    /// Whether this is the default, linear: records leave it out then.
    #[must_use]
    pub const fn is_linear(&self) -> bool {
        matches!(self, Self::Linear)
    }

    /// Where along the glide's travel (0 to 1) it is at `t` (0 to 1) of
    /// its time; 0 at 0 and 1 at 1 for every curve.
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

/// Whether a beat count is 0, the default: records leave it out then.
#[must_use]
pub const fn is_zero(value: &f64) -> bool {
    *value == 0.0
}

impl super::Request {
    /// A turn of `knob` on `module` to `value`, gliding over `glide_beats`
    /// (default 2), at once and linear.
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

    /// A cable from `from` into `to` at `amount` (default 1), at once.
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

    /// A tempo of `bpm`, at once.
    #[must_use]
    pub const fn tempo(bpm: f64) -> Self {
        Self::Tempo {
            bpm,
            after_beats: 0.0,
            glide_beats: 0.0,
        }
    }

    /// An add of a `kind` module, optionally named, placed and labelled.
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

    /// A removal of `module`.
    #[must_use]
    pub const fn remove(module: String) -> Self {
        Self::Remove {
            module,
            after_beats: None,
        }
    }

    /// An unpatch of `cable`, or of whatever is plugged into `to`.
    #[must_use]
    pub const fn unpatch(cable: Option<u32>, to: Option<String>) -> Self {
        Self::Unpatch {
            cable,
            to,
            after_beats: None,
        }
    }
}
