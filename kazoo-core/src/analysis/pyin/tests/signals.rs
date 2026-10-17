//! Test signals for the pyin parity fixtures: the generator and kazoo's test share this file.

pub const RATE: u32 = 48_000;
pub const LEN: usize = 24_000;

/// Every signal by name, `LEN` samples at `RATE`.
pub fn all() -> Vec<(&'static str, Vec<f64>)> {
    vec![
        (
            "sine220",
            (0..LEN).map(|i| 0.5 * phase(220.0, i).sin()).collect(),
        ),
        ("glide", glide()),
        ("noise", noise()),
        ("silence", vec![0.0; LEN]),
        (
            "sine3000",
            (0..LEN).map(|i| 0.4 * phase(3000.0, i).sin()).collect(),
        ),
        ("vibrato82", vibrato()),
        ("gapped", gapped()),
    ]
}

fn phase(hz: f64, i: usize) -> f64 {
    2.0 * std::f64::consts::PI * hz * i as f64 / f64::from(RATE)
}

/// 110 Hz to 880 Hz, exponential, with four harmonics.
fn glide() -> Vec<f64> {
    let mut theta = 0.0_f64;
    (0..LEN)
        .map(|i| {
            let hz = 110.0 * 8.0_f64.powf(i as f64 / LEN as f64);
            theta += 2.0 * std::f64::consts::PI * hz / f64::from(RATE);
            (1..=4)
                .map(|k| (theta * f64::from(k)).sin() / f64::from(k))
                .sum::<f64>()
                * 0.3
        })
        .collect()
}

fn noise() -> Vec<f64> {
    let mut state = 0x2545_F491_4F6C_DD1D_u64;
    (0..LEN)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((state >> 11) as f64 / (1_u64 << 53) as f64 - 0.5) * 0.6
        })
        .collect()
}

/// A bright 82 Hz tone with 5 Hz vibrato of half a semitone.
fn vibrato() -> Vec<f64> {
    let mut theta = 0.0_f64;
    (0..LEN)
        .map(|i| {
            let t = i as f64 / f64::from(RATE);
            let hz = 82.0 * (0.5 / 12.0 * (2.0 * std::f64::consts::PI * 5.0 * t).sin()).exp2();
            theta += 2.0 * std::f64::consts::PI * hz / f64::from(RATE);
            let cycle = (theta / (2.0 * std::f64::consts::PI)).fract();
            let saw = cycle + cycle - 1.0;
            0.4 * saw
        })
        .collect()
}

/// 330 Hz, silent through the middle third.
fn gapped() -> Vec<f64> {
    (0..LEN)
        .map(|i| {
            if (LEN / 3..2 * LEN / 3).contains(&i) {
                0.0
            } else {
                0.5 * phase(330.0, i).sin()
            }
        })
        .collect()
}
