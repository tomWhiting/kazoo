use super::gate::{GATE_OFF_DB, GATE_ON_DB};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GateSettings {
    pub open_db: f32,
    pub close_db: f32,
    pub hold_ms: f32,
}

impl Default for GateSettings {
    fn default() -> Self {
        Self {
            open_db: GATE_ON_DB,
            close_db: GATE_OFF_DB,
            hold_ms: 40.0,
        }
    }
}

impl GateSettings {
    #[must_use]
    pub fn held(self) -> Self {
        let finite = |value: f32, default: f32| if value.is_finite() { value } else { default };
        let open_db = finite(self.open_db, GATE_ON_DB).clamp(-70.0, -10.0);
        Self {
            open_db,
            close_db: finite(self.close_db, GATE_OFF_DB)
                .clamp(-80.0, -15.0)
                .min(open_db - 3.0),
            hold_ms: finite(self.hold_ms, 40.0).clamp(0.0, 300.0),
        }
    }
}

#[cfg(test)]
mod tests;
