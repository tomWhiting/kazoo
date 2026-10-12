//! A voice's playback direction and loop progress.

const REVERSE: u8 = 1;
const FORWARD: u8 = 2;
const WRAPPED: u8 = 4;

#[derive(Debug, Clone, Copy)]
pub struct Playback(u8);

impl Playback {
    pub const fn new(reverse: bool) -> Self {
        Self(if reverse { REVERSE | FORWARD } else { FORWARD })
    }

    pub const fn is_reverse(self) -> bool {
        self.0 & REVERSE != 0
    }

    pub const fn is_forward(self) -> bool {
        self.0 & FORWARD != 0
    }

    pub const fn has_wrapped(self) -> bool {
        self.0 & WRAPPED != 0
    }

    pub const fn wrap(&mut self) {
        self.0 |= WRAPPED;
    }

    pub const fn turn_back(&mut self) {
        self.0 &= !FORWARD;
    }

    pub const fn turn_forward(&mut self) {
        self.0 |= FORWARD;
    }
}
