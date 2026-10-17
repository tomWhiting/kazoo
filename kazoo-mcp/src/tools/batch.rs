//! `wall_batch`'s arguments: a note and the changes, each with the fields
//! of its single tool, turned into the wall's [`Request::Batch`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use kazoo_wall::protocol::Request;

/// `wall_batch`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BatchArgs {
    /// What the changes are for, like a commit message: 1 to 200
    /// characters on one line (no newlines, tabs or other control
    /// characters). The tree shows the batch as one knot with this note.
    pub note: String,
    /// The changes, applied in this order as one change: all of them, or
    /// none if any is refused (the refusal names its step, from 0).
    pub changes: Vec<BatchChange>,
}

/// One change in a batch: `op` and the fields of its single tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum BatchChange {
    /// As `wall_add`.
    Add {
        /// The kind of module, e.g. `lfo`; `wall_catalogue` lists them.
        kind: String,
        /// A display name, 1 to 24 of A-Z a-z 0-9 space _ . -, e.g. `slow
        /// wobble`.
        name: Option<String>,
        /// A label later changes in this batch name the new module by, as
        /// `@label` for the module or `@label.port` for a port (e.g.
        /// `@osc.out`): no spaces, dots or @, unique in the batch. The
        /// answer gives the id beside it.
        #[serde(rename = "as")]
        alias: Option<String>,
        /// Why this step, in a line (1 to 200 characters): shown on its
        /// own moment in the tree.
        #[serde(default, skip_serializing)]
        comment: Option<String>,
    },
    /// As `wall_remove`.
    Remove {
        /// The module's id, e.g. `lfo3`, or `@label` for one added earlier
        /// in this batch. Its cables go with it.
        module: String,
        /// Why this step, in a line (1 to 200 characters): shown on its
        /// own moment in the tree.
        #[serde(default, skip_serializing)]
        comment: Option<String>,
    },
    /// As `wall_turn`.
    Turn {
        /// Beats to hold before the sound starts at the acceptance tempo: 0 to 256, default 0.
        #[serde(default)]
        after_beats: f64,
        /// Shape in knob position: `linear` (default), `ease_in`, `ease_out` or `s_curve`.
        #[serde(default)]
        curve: super::scheduling::GlideCurve,
        /// The module's id, e.g. `vcf1`, or `@label` for one added earlier
        /// in this batch.
        module: String,
        /// The knob's name, e.g. `cutoff`.
        knob: String,
        /// Where to turn it, in the knob's own unit. Held to the knob's
        /// range.
        value: f64,
        /// How long the knob takes to get there, in beats: 0 jumps, 64 is
        /// the longest. Default 2.
        glide_beats: Option<f64>,
        /// Why this step, in a line (1 to 200 characters): shown on its
        /// own moment in the tree.
        #[serde(default, skip_serializing)]
        comment: Option<String>,
    },
    /// As `wall_patch`.
    Patch {
        /// Beats to hold before the sound starts at the acceptance tempo: 0 to 256, default 0.
        #[serde(default)]
        after_beats: f64,
        /// Beats for a linear cable fade after its wait: 0 to 64, default 0.
        #[serde(default)]
        fade_beats: f64,
        /// The output to plug from, as `module.port`, e.g. `lfo1.out` or
        /// `@osc.out`.
        from: String,
        /// The input to plug into, as `module.port`, e.g. `vcf1.cutoff`; a
        /// cable already in it is replaced.
        to: String,
        /// The attenuverter, -1 to 1. Default 1.
        amount: Option<f64>,
        /// Why this step, in a line (1 to 200 characters): shown on its
        /// own moment in the tree.
        #[serde(default, skip_serializing)]
        comment: Option<String>,
    },
    /// As `wall_unpatch`: give `cable` or `to`, not both.
    Unpatch {
        /// The cable's number, as `wall_look` lists it.
        cable: Option<u32>,
        /// The input whose cable comes out, e.g. `vcf1.cutoff` or
        /// `@osc.rate`.
        to: Option<String>,
        /// Why this step, in a line (1 to 200 characters): shown on its
        /// own moment in the tree.
        #[serde(default, skip_serializing)]
        comment: Option<String>,
    },
    /// As `wall_tempo`.
    Tempo {
        /// Beats to hold before the sound starts at the acceptance tempo: 0 to 256, default 0.
        #[serde(default)]
        after_beats: f64,
        /// Beats for the tempo to move linearly after its wait: 0 to 64, default 0.
        #[serde(default)]
        glide_beats: f64,
        /// Beats per minute, 20 to 300.
        bpm: f64,
        /// Why this step, in a line (1 to 200 characters): shown on its
        /// own moment in the tree.
        #[serde(default, skip_serializing)]
        comment: Option<String>,
    },
}

impl BatchArgs {
    /// The wall's request for these arguments.
    ///
    /// # Errors
    ///
    /// When a change does not make a request (it cannot happen for the
    /// fields above; said rather than assumed).
    pub(super) fn request(self) -> Result<Request, serde_json::Error> {
        // Each step's comment goes beside it, not in it: the wall's step
        // is the single tool's request.
        let comments: Vec<Option<String>> = self
            .changes
            .iter()
            .map(|change| change.comment().map(str::to_string))
            .collect();
        let changes = self
            .changes
            .into_iter()
            .map(|change| serde_json::from_value(serde_json::to_value(change)?))
            .collect::<Result<Vec<Request>, _>>()?;
        Ok(Request::Batch {
            note: self.note,
            changes,
            comments: if comments.iter().all(Option::is_none) {
                Vec::new()
            } else {
                comments
            },
        })
    }
}

impl BatchChange {
    /// The step's comment, if it has one.
    fn comment(&self) -> Option<&str> {
        match self {
            Self::Add { comment, .. }
            | Self::Remove { comment, .. }
            | Self::Turn { comment, .. }
            | Self::Patch { comment, .. }
            | Self::Unpatch { comment, .. }
            | Self::Tempo { comment, .. } => comment.as_deref(),
        }
    }
}
