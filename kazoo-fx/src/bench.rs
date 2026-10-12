//! Shared signals for measuring circuit models and comparing their output.

use std::f64::consts::TAU;

use crate::dsp::sane_rate;

/// The frozen high rate Screamer for interleaved cost measurements.
#[must_use]
pub fn screamer_reference() -> Box<dyn crate::Effect> {
    Box::new(crate::drive::Screamer::reference_model())
}

/// The frozen high rate Amp with both tone stacks for cost measurements.
#[must_use]
pub fn amp_reference() -> Box<dyn crate::Effect> {
    Box::new(crate::drive::Amp::reference_model())
}

/// Two seconds of a logarithmic sweep, a tone burst and seeded noise.
/// The signal has a half scale peak and includes silence after the burst.
#[must_use]
pub fn fixture(rate: f32) -> Vec<f32> {
    let rate = f64::from(sane_rate(rate));
    let length = (2.0 * rate) as usize;
    let mut state = 0x5eed_071a_u32;
    (0..length)
        .map(|frame| {
            let time = frame as f64 / rate;
            if time < 1.0 {
                let growth = (200.0f64).ln();
                let phase = TAU * 40.0 * (growth * time).exp_m1() / growth;
                (0.5 * phase.sin()) as f32
            } else if time < 1.25 {
                (0.5 * (TAU * 220.0 * time).sin()) as f32
            } else if time < 1.5 {
                0.0
            } else {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                0.5 * (2.0 * f64::from(state) / f64::from(u32::MAX) - 1.0) as f32
            }
        })
        .collect()
}
