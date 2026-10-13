//! Deterministic musical signals and a crossing-based measurement oracle.

use std::f64::consts::{PI, TAU};

pub fn glide_fourth(rate: f32) -> Vec<f32> {
    phase_signal(rate, |t| 220.0 * (293.66_f64 / 220.0).powf(t / 2.0))
}

pub fn vibrato_30c_6hz(rate: f32) -> Vec<f32> {
    phase_signal(rate, |t| {
        220.0 * (30.0 * (TAU * 6.0 * t).sin() / 1200.0).exp2()
    })
}

fn phase_signal(rate: f32, frequency: impl Fn(f64) -> f64) -> Vec<f32> {
    let mut phase: f64 = 0.0;
    (0..(rate * 2.0) as usize)
        .map(|n| {
            let out = phase.sin() as f32 * 0.25;
            phase = (phase + TAU * frequency(n as f64 / f64::from(rate)) / f64::from(rate)) % TAU;
            out
        })
        .collect()
}

pub fn chord(rate: f32, frequencies: &[f32]) -> Vec<f32> {
    (0..(rate * 2.0) as usize)
        .map(|n| {
            frequencies
                .iter()
                .map(|f| {
                    (1..=6)
                        .map(|h| {
                            (std::f32::consts::TAU * f * h as f32 * n as f32 / rate).sin()
                                / h as f32
                        })
                        .sum::<f32>()
                })
                .sum::<f32>()
                * 0.2
                / frequencies.len() as f32
        })
        .collect()
}

pub fn pluck(rate: f32, frequency: f32, seconds: f32) -> Vec<f32> {
    let delay = rate / frequency - 0.5;
    let length = delay.floor() as usize;
    let fraction = f64::from(delay.fract());
    let coefficient = (1.0 - fraction) / (1.0 + fraction);
    let mut seed = 0x1234_5678_u32;
    let mut ring: Vec<f64> = (0..length)
        .map(|n| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            0.25 * (TAU * n as f64 / f64::from(delay)).sin()
                + 0.025 * (f64::from(seed) / f64::from(u32::MAX) * 2.0 - 1.0)
        })
        .collect();
    let mean = ring.iter().sum::<f64>() / length as f64;
    for x in &mut ring {
        *x -= mean;
    }
    let mut index = 0;
    let mut last_in = 0.0;
    let mut last_out = 0.0;
    let mut previous = 0.0;
    (0..(rate * seconds) as usize)
        .map(|_| {
            let input = ring[index];
            let out = if fraction < 1e-6 {
                input
            } else {
                coefficient.mul_add(input - last_out, last_in)
            };
            last_in = input;
            last_out = out;
            ring[index] = 0.996 * 0.5 * (out + previous);
            previous = out;
            index = (index + 1) % length;
            out as f32
        })
        .collect()
}

pub fn crossings(signal: &[f32]) -> Vec<f64> {
    signal
        .windows(2)
        .enumerate()
        .filter_map(|(n, pair)| {
            if pair[0] <= 0.0 && pair[1] > 0.0 {
                Some(n as f64 + f64::from(-pair[0] / (pair[1] - pair[0])))
            } else {
                None
            }
        })
        .collect()
}

/// Band-pass around the median crossing frequency, then interpolate periods
/// at window centres. Adjacent windows can share a cycle at low frequencies.
pub fn window_pitch(signal: &[f32], rate: f32, ms: f32) -> Vec<(f64, f64)> {
    let raw = crossings(signal);
    let mut periods: Vec<f64> = raw.windows(2).map(|p| p[1] - p[0]).collect();
    assert!(!periods.is_empty(), "signal has no cycles");
    periods.sort_by(f64::total_cmp);
    let centre = f64::from(rate) / periods[periods.len() / 2];
    let g = (PI * centre / f64::from(rate)).tan();
    let a = 1.0 / (1.0 + g * (g + 2.0));
    let mut state = [0.0; 2];
    let band: Vec<f32> = signal
        .iter()
        .map(|x| {
            let v3 = f64::from(*x) - state[1];
            let v1 = a * (state[0] + g * v3);
            let v2 = state[1] + g * v1;
            state = [2.0 * v1 - state[0], 2.0 * v2 - state[1]];
            v1 as f32
        })
        .collect();
    let edges = crossings(&band);
    let measured: Vec<(f64, f64)> = edges
        .windows(2)
        .map(|p| ((p[0] + p[1]) * 0.5, f64::from(rate) / (p[1] - p[0])))
        .collect();
    let stride = f64::from(rate * ms / 1000.0);
    (0..(signal.len() as f64 / stride) as usize)
        .filter_map(|n| {
            let time = (n as f64 + 0.5) * stride;
            let i = measured.partition_point(|p| p.0 < time);
            if i == 0 || i >= measured.len() {
                return None;
            }
            let (a, b) = (measured[i - 1], measured[i]);
            let t = (time - a.0) / (b.0 - a.0);
            Some((time / f64::from(rate), (b.1 - a.1).mul_add(t, a.1)))
        })
        .collect()
}

pub fn magnitude(signal: &[f32], rate: f32, frequency: f64) -> f64 {
    let step = TAU * frequency / f64::from(rate);
    let rotation = (step.cos(), step.sin());
    let mut phase = (1.0, 0.0);
    let mut sum = (0.0, 0.0);
    for (n, sample) in signal.iter().enumerate() {
        let window = 0.5 - 0.5 * (TAU * n as f64 / signal.len() as f64).cos();
        sum.0 += f64::from(*sample) * window * phase.0;
        sum.1 += f64::from(*sample) * window * phase.1;
        phase = (
            phase.0 * rotation.0 - phase.1 * rotation.1,
            phase.0 * rotation.1 + phase.1 * rotation.0,
        );
    }
    sum.0.hypot(sum.1) / signal.len() as f64
}

/// Hann-windowed radix-two power spectrum, positive frequencies only.
pub fn spectrum(signal: &[f32]) -> Vec<f64> {
    let length = signal.len();
    assert!(length.is_power_of_two());
    let mut data: Vec<(f64, f64)> = signal
        .iter()
        .enumerate()
        .map(|(n, x)| {
            (
                f64::from(*x) * (0.5 - 0.5 * (TAU * n as f64 / length as f64).cos()),
                0.0,
            )
        })
        .collect();
    let bits = length.trailing_zeros();
    for index in 0..length {
        let other = index.reverse_bits() >> (usize::BITS - bits);
        if other > index {
            data.swap(index, other);
        }
    }
    let mut width = 2;
    while width <= length {
        let step = -TAU / width as f64;
        let rotation = (step.cos(), step.sin());
        for chunk in data.chunks_mut(width) {
            let mut phase = (1.0, 0.0);
            for n in 0..width / 2 {
                let a = chunk[n];
                let b = chunk[n + width / 2];
                let rotated = (b.0 * phase.0 - b.1 * phase.1, b.0 * phase.1 + b.1 * phase.0);
                chunk[n] = (a.0 + rotated.0, a.1 + rotated.1);
                chunk[n + width / 2] = (a.0 - rotated.0, a.1 - rotated.1);
                phase = (
                    phase.0 * rotation.0 - phase.1 * rotation.1,
                    phase.0 * rotation.1 + phase.1 * rotation.0,
                );
            }
        }
        width *= 2;
    }
    data[..length / 2]
        .iter()
        .map(|(real, imag)| real * real + imag * imag)
        .collect()
}
