//! TR-808 hand clap synthesis.
//!
//! Bandpass-filtered white noise through a multi-burst envelope:
//! three 10ms sawtooth pulses, a 20ms release, then a 100ms tail.

use super::Voice;

/// 808 hand clap voice.
#[derive(Debug)]
pub struct Clap {
    /// Bandpass filter states.
    bp_state_lo: f32,
    bp_state_hi: f32,
    bp_coeff_lo: f32,
    bp_coeff_hi: f32,
    /// Envelope phase tracking.
    envelope_pos: u32,
    amplitude: f32,
    active: bool,
    /// Pre-computed envelope segment boundaries in samples.
    burst_len: u32,
    burst_gap: u32,
    sustain_end: u32,
    tail_end: u32,
}

impl Clap {
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        // BPF centered at ~1 kHz: HP at 600 Hz, LP at 2 kHz.
        let bp_coeff_lo = (-std::f32::consts::TAU * 600.0 / sample_rate).exp();
        let bp_coeff_hi = (-std::f32::consts::TAU * 2000.0 / sample_rate).exp();

        let burst_len = seconds_to_samples(sample_rate, 0.010); // 10ms per burst
        let burst_gap = burst_len / 3; // gap between bursts
        let burst_total = (burst_len + burst_gap) * 3; // 3 bursts
        let sustain_end = burst_total + seconds_to_samples(sample_rate, 0.020);
        let tail_end = sustain_end + seconds_to_samples(sample_rate, 0.100);

        Self {
            bp_state_lo: 0.0,
            bp_state_hi: 0.0,
            bp_coeff_lo,
            bp_coeff_hi,
            envelope_pos: 0,
            amplitude: 0.0,
            active: false,
            burst_len,
            burst_gap,
            sustain_end,
            tail_end,
        }
    }

    fn envelope_value(&self) -> f32 {
        let pos = self.envelope_pos;
        let cycle = self.burst_len + self.burst_gap;

        // Three burst phase.
        if pos < cycle * 3 {
            let within_cycle = pos % cycle;
            if within_cycle < self.burst_len {
                // Sawtooth burst: ramp down.
                1.0 - (within_cycle as f32 / self.burst_len as f32)
            } else {
                0.0
            }
        } else if pos < self.sustain_end {
            // Sustain release.
            let progress = (pos - cycle * 3) as f32 / (self.sustain_end - cycle * 3) as f32;
            (1.0 - progress) * 0.6
        } else if pos < self.tail_end {
            // Reverb tail.
            let progress =
                (pos - self.sustain_end) as f32 / (self.tail_end - self.sustain_end) as f32;
            (1.0 - progress) * 0.3
        } else {
            0.0
        }
    }
}

fn seconds_to_samples(sample_rate: f32, seconds: f32) -> u32 {
    if !sample_rate.is_finite() || sample_rate <= 0.0 || !seconds.is_finite() || seconds <= 0.0 {
        return 1;
    }

    (sample_rate * seconds).round().clamp(1.0, u32::MAX as f32) as u32
}

impl Voice for Clap {
    fn trigger(&mut self, velocity: f32) {
        self.active = true;
        self.amplitude = velocity.clamp(0.0, 1.0);
        self.envelope_pos = 0;
        self.bp_state_lo = 0.0;
        self.bp_state_hi = 0.0;
    }

    fn process(&mut self) -> f32 {
        if !self.active {
            return 0.0;
        }

        let env = self.envelope_value();
        if self.envelope_pos >= self.tail_end {
            self.active = false;
            return 0.0;
        }
        self.envelope_pos += 1;

        // White noise.
        let noise = rand::random::<f32>().mul_add(2.0, -1.0);

        // Bandpass: HP then LP.
        let hp = noise - self.bp_state_lo;
        self.bp_state_lo += (1.0 - self.bp_coeff_lo) * (noise - self.bp_state_lo);
        self.bp_state_hi += (1.0 - self.bp_coeff_hi) * (hp - self.bp_state_hi);

        let output = self.bp_state_hi * env * self.amplitude;
        kazoo_core::sanitize_sample(output)
    }

    fn is_active(&self) -> bool {
        self.active
    }
}
