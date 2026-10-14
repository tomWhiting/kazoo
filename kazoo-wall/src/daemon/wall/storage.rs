use super::{Change, EngineControl, HubLink, Patch, Store, Wall};

impl Wall {
    /// A wall playing `patch` on `engine`, remembering `history` (oldest
    /// first) and saving to `store`.
    #[must_use]
    pub fn new(
        patch: Patch,
        engine: EngineControl,
        desk: Option<HubLink>,
        history: Vec<Change>,
        store: Option<Store>,
    ) -> Self {
        Self::new_with_reels(patch, engine, desk, history, store, None)
    }
}
