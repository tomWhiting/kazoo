//! TR-808 hi-hat, cymbal synthesis.
//!
//! Six square-wave oscillators at inharmonic frequencies, summed and
//! bandpass filtered. The inharmonic ratios create metallic timbre.
//! Shared `MetalOscBank` drives closed hat, open hat, and cymbal voices.

use super::Voice;

/// Frequencies of the six metallic oscillators (Hz).
const METAL_FREQS: [f32; 6] = [800.0, 540.0, 523.0, 370.0, 304.0, 205.0];

/// Shared metallic oscillator bank used by hi-hats, cymbal, and cowbell.
///
/// Six square-wave oscillators at inharmonic frequencies. The non-integer
/// ratios create the characteristic metallic timbre of the TR-808.
#[derive(Debug)]
pub struct MetalOscBank {
    phases: [f32; 6],
    increments: [f32; 6],
    tune_ratio: f32,
}

impl MetalOscBank {
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(1.0);
        let mut increments = [0.0_f32; 6];
        for (i, inc) in increments.iter_mut().enumerate() {
            *inc = METAL_FREQS[i] / sr;
        }
        Self {
            phases: [0.0; 6],
            increments,
            tune_ratio: 1.0,
        }
    }

    /// Recompute increments for a new sample rate.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        let sr = sample_rate.max(1.0);
        for (i, inc) in self.increments.iter_mut().enumerate() {
            *inc = METAL_FREQS[i] * self.tune_ratio / sr;
        }
    }

    /// Set tuning ratio (1.0 = default). Requires sample rate to recompute.
    pub fn set_tune(&mut self, ratio: f32, sample_rate: f32) {
        self.tune_ratio = ratio.clamp(0.5, 2.0);
        self.set_sample_rate(sample_rate);
    }

    /// Generate one sample from the mixed square-wave bank.
    pub fn process(&mut self) -> f32 {
        let mut sum = 0.0_f32;
        for (phase, &inc) in self.phases.iter_mut().zip(&self.increments) {
            *phase += inc;
            if *phase >= 1.0 {
                *phase -= 1.0;
            }
            sum += if *phase < 0.5 { 1.0 } else { -1.0 };
        }
        sum / 6.0
    }

    /// Reset all oscillator phases.
    pub const fn reset(&mut self) {
        self.phases = [0.0; 6];
    }
}

// ---------------------------------------------------------------------------
// Utility: exponential decay coefficient
// ---------------------------------------------------------------------------

/// Compute per-sample exponential decay coefficient for -60 dB in `seconds`.
fn decay_coeff(sample_rate: f32, seconds: f32) -> f32 {
    let samples = sample_rate * seconds;
    if samples > 0.0 {
        (-6.9 / samples).exp()
    } else {
        0.0
    }
}

// ---------------------------------------------------------------------------
// Closed Hi-Hat
// ---------------------------------------------------------------------------

/// 808 closed hi-hat voice. Fixed 50 ms decay.
#[derive(Debug)]
pub struct ClosedHiHat {
    osc_bank: MetalOscBank,
    amplitude: f32,
    amp_decay: f32,
    bp_state_1: f32,
    bp_state_2: f32,
    bp_coeff_1: f32,
    bp_coeff_2: f32,
    active: bool,
    sample_rate: f32,
}

impl ClosedHiHat {
    /// Bandpass HP cutoff (Hz).
    const HP_FREQ: f32 = 3440.0;
    /// Bandpass LP cutoff (Hz).
    const LP_FREQ: f32 = 7100.0;
    /// Fixed decay time (seconds).
    const DECAY_TIME: f32 = 0.05;

    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let bp_coeff_1 = (-std::f32::consts::TAU * Self::HP_FREQ / sample_rate).exp();
        let bp_coeff_2 = (-std::f32::consts::TAU * Self::LP_FREQ / sample_rate).exp();
        Self {
            osc_bank: MetalOscBank::new(sample_rate),
            amplitude: 0.0,
            amp_decay: decay_coeff(sample_rate, Self::DECAY_TIME),
            bp_state_1: 0.0,
            bp_state_2: 0.0,
            bp_coeff_1,
            bp_coeff_2,
            active: false,
            sample_rate,
        }
    }

    /// Set tuning ratio for the metallic oscillator bank.
    pub fn set_tune(&mut self, ratio: f32) {
        self.osc_bank.set_tune(ratio, self.sample_rate);
    }
}

impl Voice for ClosedHiHat {
    fn trigger(&mut self, velocity: f32) {
        self.active = true;
        self.amplitude = velocity.clamp(0.0, 1.0);
    }

    fn process(&mut self) -> f32 {
        if !self.active {
            return 0.0;
        }

        let raw = self.osc_bank.process();

        // HP then LP to approximate bandpass.
        let hp = raw - self.bp_state_1;
        self.bp_state_1 += (1.0 - self.bp_coeff_1) * (raw - self.bp_state_1);
        self.bp_state_2 += (1.0 - self.bp_coeff_2) * (hp - self.bp_state_2);

        let output = self.bp_state_2 * self.amplitude;

        self.amplitude *= self.amp_decay;
        if self.amplitude < 1e-6 {
            self.active = false;
        }

        kazoo_core::sanitize_sample(output)
    }

    fn is_active(&self) -> bool {
        self.active
    }
}

// ---------------------------------------------------------------------------
// Open Hi-Hat
// ---------------------------------------------------------------------------

/// 808 open hi-hat voice. Adjustable decay (90-600 ms).
#[derive(Debug)]
pub struct OpenHiHat {
    osc_bank: MetalOscBank,
    amplitude: f32,
    amp_decay: f32,
    bp_state_1: f32,
    bp_state_2: f32,
    bp_coeff_1: f32,
    bp_coeff_2: f32,
    active: bool,
    sample_rate: f32,
    decay_time: f32,
}

impl OpenHiHat {
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let bp_coeff_1 = (-std::f32::consts::TAU * 3440.0 / sample_rate).exp();
        let bp_coeff_2 = (-std::f32::consts::TAU * 7100.0 / sample_rate).exp();
        let decay_time = 0.3;
        Self {
            osc_bank: MetalOscBank::new(sample_rate),
            amplitude: 0.0,
            amp_decay: decay_coeff(sample_rate, decay_time),
            bp_state_1: 0.0,
            bp_state_2: 0.0,
            bp_coeff_1,
            bp_coeff_2,
            active: false,
            sample_rate,
            decay_time,
        }
    }

    /// Set decay time in seconds (0.09-0.6).
    pub fn set_decay(&mut self, seconds: f32) {
        self.decay_time = seconds.clamp(0.09, 0.6);
        self.amp_decay = decay_coeff(self.sample_rate, self.decay_time);
    }

    /// Set tuning ratio for the metallic oscillator bank.
    pub fn set_tune(&mut self, ratio: f32) {
        self.osc_bank.set_tune(ratio, self.sample_rate);
    }

    /// Choke the open hat (called when closed hat triggers).
    pub const fn choke(&mut self) {
        self.active = false;
        self.amplitude = 0.0;
    }
}

impl Voice for OpenHiHat {
    fn trigger(&mut self, velocity: f32) {
        self.active = true;
        self.amplitude = velocity.clamp(0.0, 1.0);
    }

    fn process(&mut self) -> f32 {
        if !self.active {
            return 0.0;
        }

        let raw = self.osc_bank.process();
        let hp = raw - self.bp_state_1;
        self.bp_state_1 += (1.0 - self.bp_coeff_1) * (raw - self.bp_state_1);
        self.bp_state_2 += (1.0 - self.bp_coeff_2) * (hp - self.bp_state_2);

        let output = self.bp_state_2 * self.amplitude;
        self.amplitude *= self.amp_decay;
        if self.amplitude < 1e-6 {
            self.active = false;
        }

        kazoo_core::sanitize_sample(output)
    }

    fn is_active(&self) -> bool {
        self.active
    }
}

// ---------------------------------------------------------------------------
// Cymbal
// ---------------------------------------------------------------------------

/// 808 cymbal voice.
///
/// Same six-oscillator metallic source as hi-hats with longer decay
/// (350-1200ms adjustable) and different filter balance emphasizing
/// lower metallic frequencies.
#[derive(Debug)]
pub struct Cymbal {
    osc_bank: MetalOscBank,
    amplitude: f32,
    amp_decay: f32,
    bp_state_1: f32,
    bp_state_2: f32,
    bp_coeff_1: f32,
    bp_coeff_2: f32,
    active: bool,
    sample_rate: f32,
    decay_time: f32,
}

impl Cymbal {
    /// HP cutoff for cymbal (lower than hats to emphasize metallic body).
    const HP_FREQ: f32 = 1500.0;
    /// LP cutoff for cymbal (wider than hats).
    const LP_FREQ: f32 = 8000.0;

    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let bp_coeff_1 = (-std::f32::consts::TAU * Self::HP_FREQ / sample_rate).exp();
        let bp_coeff_2 = (-std::f32::consts::TAU * Self::LP_FREQ / sample_rate).exp();
        let decay_time = 0.6;
        Self {
            osc_bank: MetalOscBank::new(sample_rate),
            amplitude: 0.0,
            amp_decay: decay_coeff(sample_rate, decay_time),
            bp_state_1: 0.0,
            bp_state_2: 0.0,
            bp_coeff_1,
            bp_coeff_2,
            active: false,
            sample_rate,
            decay_time,
        }
    }

    /// Set decay time in seconds (0.35-1.2).
    pub fn set_decay(&mut self, seconds: f32) {
        self.decay_time = seconds.clamp(0.35, 1.2);
        self.amp_decay = decay_coeff(self.sample_rate, self.decay_time);
    }

    /// Set tuning ratio for the metallic oscillator bank.
    pub fn set_tune(&mut self, ratio: f32) {
        self.osc_bank.set_tune(ratio, self.sample_rate);
    }
}

impl Voice for Cymbal {
    fn trigger(&mut self, velocity: f32) {
        self.active = true;
        self.amplitude = velocity.clamp(0.0, 1.0);
    }

    fn process(&mut self) -> f32 {
        if !self.active {
            return 0.0;
        }

        let raw = self.osc_bank.process();

        let hp = raw - self.bp_state_1;
        self.bp_state_1 += (1.0 - self.bp_coeff_1) * (raw - self.bp_state_1);
        self.bp_state_2 += (1.0 - self.bp_coeff_2) * (hp - self.bp_state_2);

        let output = self.bp_state_2 * self.amplitude;
        self.amplitude *= self.amp_decay;
        if self.amplitude < 1e-6 {
            self.active = false;
        }

        kazoo_core::sanitize_sample(output)
    }

    fn is_active(&self) -> bool {
        self.active
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metal_osc_bank_produces_finite_output() {
        let mut bank = MetalOscBank::new(44100.0);
        for _ in 0..1000 {
            let s = bank.process();
            assert!(s.is_finite(), "MetalOscBank output must be finite");
            assert!(s.abs() <= 1.0, "MetalOscBank output must be <= 1.0");
        }
    }

    #[test]
    fn closed_hihat_decays_to_silence() {
        let mut ch = ClosedHiHat::new(44100.0);
        ch.trigger(1.0);
        assert!(ch.is_active());
        // Run for 200ms (well past 50ms decay).
        for _ in 0..8820 {
            ch.process();
        }
        assert!(!ch.is_active(), "closed hat should decay to silence");
    }

    #[test]
    fn open_hihat_choke() {
        let mut oh = OpenHiHat::new(44100.0);
        oh.trigger(1.0);
        assert!(oh.is_active());
        oh.choke();
        assert!(!oh.is_active());
    }

    #[test]
    fn cymbal_longer_than_closed_hat() {
        let mut cym = Cymbal::new(44100.0);
        cym.trigger(1.0);
        // After 100ms the cymbal should still be active.
        for _ in 0..4410 {
            cym.process();
        }
        assert!(cym.is_active(), "cymbal should still ring at 100ms");
    }

    #[test]
    fn all_metallic_voices_sanitize_output() {
        let mut ch = ClosedHiHat::new(44100.0);
        let mut oh = OpenHiHat::new(44100.0);
        let mut cym = Cymbal::new(44100.0);

        ch.trigger(1.0);
        oh.trigger(1.0);
        cym.trigger(1.0);

        for _ in 0..4410 {
            let s1 = ch.process();
            let s2 = oh.process();
            let s3 = cym.process();
            assert!(s1.is_finite());
            assert!(s2.is_finite());
            assert!(s3.is_finite());
        }
    }
}
