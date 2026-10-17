use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GlideCurve {
    #[default]
    Linear,
    EaseIn,
    EaseOut,
    SCurve,
}

impl From<GlideCurve> for kazoo_wall::protocol::GlideCurve {
    fn from(curve: GlideCurve) -> Self {
        match curve {
            GlideCurve::Linear => Self::Linear,
            GlideCurve::EaseIn => Self::EaseIn,
            GlideCurve::EaseOut => Self::EaseOut,
            GlideCurve::SCurve => Self::SCurve,
        }
    }
}
