use serde::{Deserialize, Serialize};

/// A loop's recorder or pending reel fetch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopState {
    #[default]
    Blank,
    Armed,
    Rolling,
    PostRoll,
    Holding,
    NoTape,
    Loading,
}

/// Tape observations built on the control thread.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoopView {
    pub state: LoopState,
    pub beats: f64,
    pub frames: u64,
    pub rate: u32,
    pub anchor: f64,
    pub pass: i64,
    pub written: Vec<bool>,
    pub ages: Vec<u32>,
    pub bins: Vec<u8>,
    pub peak_db: f64,
    pub from: Option<String>,
    pub take: Option<u64>,
    pub reel: Option<String>,
    pub shortened: bool,
    pub notes: Vec<String>,
}

impl Default for LoopView {
    fn default() -> Self {
        Self {
            state: LoopState::Blank,
            beats: 0.0,
            frames: 0,
            rate: 0,
            anchor: 0.0,
            pass: 0,
            written: Vec::new(),
            ages: Vec::new(),
            bins: Vec::new(),
            peak_db: -120.0,
            from: None,
            take: None,
            reel: None,
            shortened: false,
            notes: Vec::new(),
        }
    }
}

/// The common seam period, and the next seam shared by every tape.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LoopsMeet {
    pub bars: Option<u64>,
    pub seconds: Option<f64>,
    pub next_bar: Option<u64>,
}
