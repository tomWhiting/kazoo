//! Places and immutable names in the wall's durable tree.

use serde::{Deserialize, Serialize};

/// A place in the tree: a commit, and the line and mark it was reached by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreePlace {
    pub commit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<u64>,
}

/// One moment's identity and human-readable commit metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MomentView {
    pub commit: String,
    pub short: String,
    pub change: Option<u64>,
    pub seat: String,
    pub at: String,
    pub summary: String,
}

/// A line and the moment from which it grew.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchView {
    pub name: String,
    pub tip: MomentView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grew_from: Option<String>,
}

/// Which kind of immutable name a tag supplies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkKind {
    Mark,
    Save,
}

/// An immutable name and the moment it names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkView {
    pub name: String,
    pub kind: MarkKind,
    pub tag: String,
    pub moment: MomentView,
}

/// The name just given to the wall's current moment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkResult {
    pub name: String,
    pub tag: String,
    pub at: TreePlace,
}

/// A bounded listing with counts for the names beyond the screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeResult {
    pub here: Option<TreePlace>,
    pub through: u64,
    #[serde(default)]
    pub revision: u64,
    pub branches: Vec<BranchView>,
    pub branch_count: usize,
    pub marks: Vec<MarkView>,
    pub mark_count: usize,
    pub recent: Vec<MomentView>,
}

/// Twenty recent moments fill one screen.
pub const DEFAULT_RECENT: u32 = 20;
/// Two hundred moments bound a walk on the thread that also writes commits;
/// a git log of this size takes a few milliseconds.
pub const MAX_RECENT: u32 = 200;
/// List at most two hundred branches or marks; counts disclose the rest.
pub const MAX_LISTED: usize = 200;
/// Moment names fit in 128 bytes; even a bud's name is under ninety.
pub const MAX_MOMENT: usize = 128;
/// More than a year of daily marking by every seat; beyond this a list
/// of marks is no longer read. Console marks remain unlimited.
pub const MAX_MARKS: usize = 1_000;
