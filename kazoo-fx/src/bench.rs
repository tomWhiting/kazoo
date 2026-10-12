//! Shared deterministic input for circuit model measurements.

/// A block of a minus twelve decibel, 110 Hz tone at `rate`.
#[must_use]
pub fn fixture(rate: f32) -> Vec<f32> {
    assert!(
        rate.is_finite() && rate > 0.0,
        "fixture rate must be finite and positive"
    );
    (0..512)
        .map(|frame| {
            (std::f32::consts::TAU * 110.0 * frame as f32 / rate).sin()
                * 10.0_f32.powf(-12.0 / 20.0)
        })
        .collect()
}
