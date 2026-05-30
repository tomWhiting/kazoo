//! One synthesis layer: VCO -> HPF -> LPF -> VCA.
//!
//! The CS-80 has two of these per voice, independently controlled.
//! Each filter is 12 dB/oct state-variable.
//!
//! Signal flow:
//!   VCO (saw/pulse) -> HPF (12dB SVF) -> LPF (12dB SVF) -> VCA
//!   VCO (sine) injected post-filter (sine has no harmonics to filter)

use kazoo_core::sanitize_sample;

use super::envelope::{AdsrEnvelope, FilterEnvelope};
use super::filter::StateVariableFilter;
use super::oscillator::Oscillator;

/// Parameters for a single synthesis layer.
#[derive(Debug, Clone)]
pub struct LayerParams {
    /// HPF cutoff in Hz.
    pub hpf_cutoff: f32,
    /// HPF resonance [0, 0.95].
    pub hpf_resonance: f32,
    /// LPF base cutoff in Hz (before envelope modulation).
    pub lpf_cutoff: f32,
    /// LPF resonance [0, 0.95].
    pub lpf_resonance: f32,
    /// Filter envelope to cutoff modulation depth in Hz.
    /// The envelope output [0,1] is multiplied by this to offset the cutoff.
    pub filter_env_depth: f32,
    /// Layer output level [0, 1].
    pub level: f32,
}

impl Default for LayerParams {
    fn default() -> Self {
        Self {
            hpf_cutoff: 80.0,
            hpf_resonance: 0.2,
            lpf_cutoff: 2400.0,
            lpf_resonance: 0.4,
            filter_env_depth: 4000.0,
            level: 0.7,
        }
    }
}

/// One complete synthesis layer of the CS-80.
///
/// Signal flow: VCO -> HPF -> LPF -> VCA.
/// Sine is injected post-filter.
#[derive(Debug)]
pub struct Layer {
    /// Voltage-controlled oscillator.
    pub oscillator: Oscillator,
    /// High-pass filter (12 dB/oct SVF).
    hpf: StateVariableFilter,
    /// Low-pass filter (12 dB/oct SVF).
    lpf: StateVariableFilter,
    /// VCA amplitude envelope (ADSR).
    pub vca_envelope: AdsrEnvelope,
    /// Filter cutoff envelope (IL/AL).
    pub filter_envelope: FilterEnvelope,
    /// Layer parameters.
    pub params: LayerParams,
    /// Sample rate.
    sample_rate: f32,
    /// Current note velocity [0, 1] — used to scale filter envelope depth at tick time
    /// without mutating the stored param.
    velocity: f32,
}

impl Layer {
    /// Create a new layer at the given sample rate.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let mut hpf = StateVariableFilter::new(sample_rate);
        hpf.set_cutoff(80.0);
        hpf.set_resonance(0.2);

        let mut lpf = StateVariableFilter::new(sample_rate);
        lpf.set_cutoff(2400.0);
        lpf.set_resonance(0.4);

        Self {
            oscillator: Oscillator::new(sample_rate),
            hpf,
            lpf,
            vca_envelope: AdsrEnvelope::new(sample_rate),
            filter_envelope: FilterEnvelope::new(sample_rate),
            params: LayerParams::default(),
            sample_rate: sample_rate.max(1.0),
            velocity: 0.0,
        }
    }

    /// Set the oscillator frequency (before drift/LFO modulation).
    pub fn set_frequency(&mut self, freq_hz: f32) {
        self.oscillator.set_frequency(freq_hz);
    }

    /// Update filter parameters from current params + envelope + LFO state.
    ///
    /// The LPF cutoff combines: base + envelope offset + LFO offset.
    /// Both modulations are additive — neither overwrites the other.
    fn update_filters(&mut self, filter_env_value: f32, lfo_filter_mod: f32) {
        // HPF: static cutoff (not modulated by envelope or LFO)
        self.hpf.set_cutoff(self.params.hpf_cutoff);
        self.hpf.set_resonance(self.params.hpf_resonance);

        // LPF: base cutoff + envelope modulation + LFO modulation
        // Velocity scales the filter envelope depth: vel=0 -> 50%, vel=1 -> 100% of nominal depth.
        // Computed at tick time from stored velocity — never mutates params.filter_env_depth.
        let vel_scaled_depth =
            self.params.filter_env_depth * 0.5f32.mul_add(self.velocity.clamp(0.0, 1.0), 0.5);
        let env_offset = filter_env_value * vel_scaled_depth;
        let lfo_offset = if lfo_filter_mod.is_finite() {
            // LFO uses a fixed 2 kHz modulation range, independent of envelope depth
            lfo_filter_mod * 2000.0
        } else {
            0.0
        };
        let lpf_freq = (self.params.lpf_cutoff + env_offset + lfo_offset).clamp(20.0, 20_000.0);
        self.lpf.set_cutoff(lpf_freq);
        self.lpf.set_resonance(self.params.lpf_resonance);
    }

    /// Trigger note-on for this layer.
    pub const fn note_on(&mut self, velocity: f32) {
        self.velocity = velocity.clamp(0.0, 1.0);
        self.vca_envelope.note_on();
        self.filter_envelope.note_on();
    }

    /// Trigger note-off for this layer.
    pub fn note_off(&mut self) {
        self.vca_envelope.note_off();
        self.filter_envelope.note_off();
    }

    /// Set sample rate.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        self.oscillator.set_sample_rate(sample_rate);
        self.hpf.set_sample_rate(sample_rate);
        self.lpf.set_sample_rate(sample_rate);
        self.vca_envelope.set_sample_rate(sample_rate);
        self.filter_envelope.set_sample_rate(sample_rate);
    }

    /// Reset all state.
    pub const fn reset(&mut self) {
        self.oscillator.reset();
        self.hpf.reset();
        self.lpf.reset();
        self.vca_envelope.reset();
        self.filter_envelope.reset();
        self.velocity = 0.0;
    }

    /// Process one sample. Returns the layer output.
    ///
    /// `lfo_filter_mod`: additive filter cutoff modulation from LFO [-1, 1].
    /// `timing_scale`: per-voice drift timing factor (1.0 = nominal).
    #[inline]
    pub fn tick(&mut self, lfo_filter_mod: f32) -> f32 {
        self.tick_with_timing(lfo_filter_mod, 1.0)
    }

    /// Process one sample with per-voice envelope timing jitter.
    ///
    /// `lfo_filter_mod`: additive filter cutoff modulation from LFO [-1, 1].
    /// `timing_scale`: per-voice drift timing factor — values > 1.0 slow envelopes,
    /// < 1.0 speed them up. This IS the CS-80 sound.
    #[inline]
    pub fn tick_with_timing(&mut self, lfo_filter_mod: f32, timing_scale: f32) -> f32 {
        // Envelopes — scaled by per-voice timing jitter
        let filter_env = self.filter_envelope.tick_scaled(timing_scale);
        let vca_env = self.vca_envelope.tick_scaled(timing_scale);

        // Update all filters: HPF static, LPF = base + envelope + LFO combined
        self.update_filters(filter_env, lfo_filter_mod);

        // VCO: saw/pulse goes to filters, sine is post-filter
        let (filtered_signal, sine_signal) = self.oscillator.tick();

        // Signal chain: VCO -> HPF -> LPF
        let (hp_out, _) = self.hpf.tick(filtered_signal);
        let (_, lp_out) = self.lpf.tick(hp_out);

        // Add sine post-filter
        let mixed = lp_out + sine_signal;

        // VCA: apply amplitude envelope and layer level
        let output = mixed * vca_env * self.params.level;

        sanitize_sample(output)
    }

    /// Whether this layer's VCA envelope has finished.
    #[must_use]
    pub const fn is_idle(&self) -> bool {
        self.vca_envelope.is_idle()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::oscillator::Waveform;

    #[test]
    fn layer_produces_output() {
        let mut layer = Layer::new(44100.0);
        layer.oscillator.waveform = Waveform::Sawtooth;
        layer.set_frequency(440.0);
        layer.note_on(1.0);

        let mut has_nonzero = false;
        for _ in 0..4410 {
            let sample = layer.tick(0.0);
            assert!(sample.is_finite());
            if sample.abs() > 0.001 {
                has_nonzero = true;
            }
        }
        assert!(has_nonzero, "layer should produce audible output");
    }

    #[test]
    fn layer_silent_when_idle() {
        let mut layer = Layer::new(44100.0);
        layer.oscillator.waveform = Waveform::Sawtooth;
        layer.set_frequency(440.0);
        // Don't trigger note_on
        for _ in 0..100 {
            let sample = layer.tick(0.0);
            assert!(
                sample.abs() < 0.001,
                "idle layer should be silent, got {sample}"
            );
        }
    }

    #[test]
    fn layer_note_off_decays() {
        let mut layer = Layer::new(44100.0);
        layer.oscillator.waveform = Waveform::Sawtooth;
        layer.set_frequency(440.0);
        layer.vca_envelope.set_attack(0.001);
        layer.vca_envelope.set_release(0.01);
        layer.note_on(1.0);

        // Play for a bit
        for _ in 0..4410 {
            layer.tick(0.0);
        }
        layer.note_off();

        // After release time, should be silent
        for _ in 0..44100 {
            layer.tick(0.0);
        }
        assert!(layer.is_idle(), "layer should be idle after release");
    }

    #[test]
    fn layer_filter_depth_stable_across_notes() {
        let mut layer = Layer::new(44100.0);
        layer.oscillator.waveform = Waveform::Sawtooth;
        layer.set_frequency(440.0);
        layer.params.filter_env_depth = 4000.0;

        // Play multiple notes with varying velocity
        for vel in [0.2, 0.5, 0.8, 1.0, 0.3, 0.9] {
            layer.note_on(vel);
            for _ in 0..4410 {
                layer.tick(0.0);
            }
            layer.note_off();
            for _ in 0..4410 {
                layer.tick(0.0);
            }
        }

        // The stored param must remain exactly what was set
        assert!(
            (layer.params.filter_env_depth - 4000.0).abs() < f32::EPSILON,
            "filter_env_depth should not drift, got {}",
            layer.params.filter_env_depth
        );
    }

    #[test]
    fn layer_output_always_sanitized() {
        let mut layer = Layer::new(44100.0);
        layer.set_frequency(440.0);
        layer.note_on(1.0);

        for _ in 0..44100 {
            let sample = layer.tick(0.0);
            assert!(sample.is_finite(), "layer output must be finite");
            assert!(
                sample.abs() < 10.0,
                "layer output unreasonably large: {sample}"
            );
        }
    }
}
