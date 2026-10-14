use std::time::Duration;

use kazoo_core::ipc::follow::beats_in;

/// Transport position between snapshots, using the shared beat conversion.
#[must_use]
pub fn beat_now(snapshot_beat: f64, tempo: f64, since: Duration) -> f64 {
    let beat = if snapshot_beat.is_finite() {
        snapshot_beat
    } else {
        0.0
    };
    if !tempo.is_finite() || tempo <= 0.0 {
        return beat;
    }
    let position = beat + beats_in(since.as_secs_f64(), tempo, 1.0);
    if position.is_finite() { position } else { beat }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beat_now_moves_with_the_tempo() {
        assert!((beat_now(10.0, 120.0, Duration::from_millis(250)) - 10.5).abs() < f64::EPSILON);
        assert!((beat_now(10.0, 60.0, Duration::from_secs(2)) - 12.0).abs() < f64::EPSILON);
        assert!((beat_now(10.0, f64::NAN, Duration::from_secs(2)) - 10.0).abs() < f64::EPSILON);
        assert!(beat_now(f64::INFINITY, 120.0, Duration::ZERO).is_finite());
    }
}
