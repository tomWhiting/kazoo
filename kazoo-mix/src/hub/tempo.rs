use crate::shared::SharedState;
use crate::song::SongAnchor;

pub fn initial_song(shared: &SharedState) -> SongAnchor {
    SongAnchor::stopped(f64::from(shared.transport().bpm))
}
