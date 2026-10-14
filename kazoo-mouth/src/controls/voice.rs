use super::{Settings, step};

#[derive(Debug, Clone, Copy)]
pub(super) enum Dial {
    Voice,
    Bands,
    Sibilance,
}

impl Settings {
    pub(super) fn adjust_vocoder(&mut self, dial: Dial, right: bool, shift: bool) {
        let direction = if right { 1.0 } else { -1.0 };
        let amount = if shift { 0.25 } else { 0.05 };
        match dial {
            Dial::Voice => self.synth.voice = step(self.synth.voice, direction, amount, 1.0),
            Dial::Bands => {
                self.synth.bands = if right {
                    (self.synth.bands + 8).min(24)
                } else {
                    self.synth.bands.saturating_sub(8).max(8)
                };
            }
            Dial::Sibilance => {
                self.synth.sibilance = step(self.synth.sibilance, direction, amount, 1.0);
            }
        }
    }
}
