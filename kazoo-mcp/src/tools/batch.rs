use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use kazoo_wall::protocol::Request;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BatchArgs {
    pub note: String,
    pub changes: Vec<BatchChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum BatchChange {
    Add {
        kind: String,
        name: Option<String>,
        #[serde(rename = "as")]
        alias: Option<String>,
    },
    Remove {
        module: String,
    },
    Turn {
        module: String,
        knob: String,
        value: f64,
        glide_beats: Option<f64>,
    },
    Patch {
        from: String,
        to: String,
        amount: Option<f64>,
    },
    Unpatch {
        cable: Option<u32>,
        to: Option<String>,
    },
    Tempo {
        bpm: f64,
    },
}

impl BatchArgs {
    pub(super) fn request(self) -> Result<Request, serde_json::Error> {
        let changes = self
            .changes
            .into_iter()
            .map(|change| serde_json::from_value(serde_json::to_value(change)?))
            .collect::<Result<Vec<Request>, _>>()?;
        Ok(Request::Batch {
            note: self.note,
            changes,
        })
    }
}
