use super::{Engine, SUB_BLOCK, Wall};

pub(super) fn land(wall: &Wall, engine: &mut Engine) {
    let shared = wall.engine.shared();
    let mut buffer = [0.0; SUB_BLOCK * 2];
    while shared.unhushed() < wall.swaps_sent {
        engine.render(&mut buffer, 2);
    }
}
