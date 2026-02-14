//! Metronome click generator.
//!
//! The [`Metronome`] generates short sine-burst clicks synchronised to the
//! transport's beat grid. Two click tones are pre-rendered at construction:
//! a higher-pitched downbeat (beat 1) and a lower-pitched normal beat.
//! The [`generate`](Metronome::generate) method detects beat boundaries
//! within each audio block and mixes clicks into the output buffer.

use crate::Db;

/// Duration of a click in seconds (~10 ms).
const CLICK_DURATION_SECS: f32 = 0.01;

/// Frequency of the downbeat click (Hz).
const DOWNBEAT_FREQ: f32 = 1000.0;

/// Frequency of the normal beat click (Hz).
const BEAT_FREQ: f32 = 800.0;

/// Metronome click generator.
///
/// Pre-renders two click buffers at construction and plays them back
/// when beat boundaries are crossed. All methods are O(1) per sample
/// with no allocation, making this safe for the processing thread.
#[derive(Debug)]
pub struct Metronome {
    sample_rate: u32,
    /// Pre-rendered downbeat click (beat 1): sine burst at 1000 Hz.
    downbeat_click: Vec<f32>,
    /// Pre-rendered normal beat click: sine burst at 800 Hz.
    beat_click: Vec<f32>,
    /// Linear gain applied to click output.
    volume: f32,
    /// Current playback position within the active click buffer.
    /// When `>= click length`, no click is playing.
    click_pos: usize,
    /// Which click buffer is currently playing (`true` = downbeat).
    is_downbeat: bool,
    /// The beat number (absolute) of the last triggered click,
    /// used to prevent re-triggering the same beat.
    last_triggered_beat: u64,
}

impl Metronome {
    /// Create a new metronome, pre-rendering both click buffers.
    ///
    /// A `sample_rate` of 0 is treated as 1 to avoid division-by-zero.
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(1);
        Self {
            sample_rate: sr,
            downbeat_click: render_click(sr, DOWNBEAT_FREQ),
            beat_click: render_click(sr, BEAT_FREQ),
            volume: Db::new(-6.0).to_linear(),
            click_pos: usize::MAX,
            is_downbeat: false,
            last_triggered_beat: u64::MAX,
        }
    }

    /// Set metronome volume from a decibel value.
    pub fn set_volume(&mut self, db: Db) {
        self.volume = db.to_linear();
    }

    /// Mix metronome clicks into an interleaved stereo output buffer.
    ///
    /// `position` is the transport sample position at the START of this block.
    /// The method scans each sample in the block for beat boundary crossings
    /// and starts the appropriate click playback.
    ///
    /// # Arguments
    ///
    /// * `output` — Interleaved stereo buffer `[L0, R0, L1, R1, ...]`.
    /// * `position` — Transport position in samples at block start.
    /// * `bpm` — Current tempo in beats per minute.
    /// * `beats_per_bar` — Time signature numerator.
    /// * `num_samples` — Number of mono samples in this block.
    pub fn generate(
        &mut self,
        output: &mut [f32],
        position: u64,
        bpm: f64,
        beats_per_bar: u8,
        num_samples: usize,
    ) {
        if bpm <= 0.0 || !bpm.is_finite() || beats_per_bar == 0 {
            return;
        }

        let samples_per_beat = f64::from(self.sample_rate) * 60.0 / bpm;
        if samples_per_beat <= 0.0 || !samples_per_beat.is_finite() {
            return;
        }

        let click_len = self.downbeat_click.len();

        for i in 0..num_samples {
            let sample_pos = position.saturating_add(i as u64);

            // Determine which beat number this sample falls on.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let beat_number = (sample_pos as f64 / samples_per_beat).floor() as u64;

            // Check if we've crossed into a new beat.
            if beat_number != self.last_triggered_beat {
                // After a reset (last_triggered_beat == MAX), only fire a click
                // if the current position is very close to a beat boundary.
                // This prevents a spurious click when resuming from mid-beat.
                let should_fire = if self.last_triggered_beat == u64::MAX {
                    let samples_into_beat = sample_pos as f64 % samples_per_beat;
                    // Allow a small tolerance (1 sample) for rounding.
                    samples_into_beat < 1.5
                } else {
                    true
                };

                self.last_triggered_beat = beat_number;

                if should_fire {
                    self.click_pos = 0;

                    // Beat 1 (downbeat) is when beat_number % beats_per_bar == 0.
                    #[allow(clippy::cast_possible_truncation)]
                    let beat_in_bar = (beat_number % u64::from(beats_per_bar)) as u8;
                    self.is_downbeat = beat_in_bar == 0;
                }
            }

            // Mix click sample if we're currently playing a click.
            if self.click_pos < click_len {
                let click_buf = if self.is_downbeat {
                    &self.downbeat_click
                } else {
                    &self.beat_click
                };

                let click_sample = click_buf[self.click_pos] * self.volume;
                let stereo_idx = i * 2;

                // Mix into both stereo channels.
                if stereo_idx + 1 < output.len() {
                    output[stereo_idx] += click_sample;
                    output[stereo_idx + 1] += click_sample;
                }

                self.click_pos += 1;
            }
        }
    }

    /// Reset internal playback state.
    ///
    /// Called when the transport stops or seeks to avoid stale click playback.
    pub const fn reset(&mut self) {
        self.click_pos = usize::MAX;
        self.last_triggered_beat = u64::MAX;
    }
}

/// Render a click buffer: a sine wave with exponential decay envelope.
///
/// Duration is [`CLICK_DURATION_SECS`] at the given sample rate.
fn render_click(sample_rate: u32, frequency: f32) -> Vec<f32> {
    let sr = sample_rate.max(1) as f32;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let num_samples = (sr * CLICK_DURATION_SECS) as usize;
    let num_samples = num_samples.max(1);

    let mut buf = Vec::with_capacity(num_samples);
    let two_pi_f = 2.0 * std::f32::consts::PI * frequency;

    for i in 0..num_samples {
        let t = i as f32 / sample_rate as f32;
        // Sine oscillator.
        let sine = (two_pi_f * t).sin();
        // Exponential decay envelope: starts at 1.0, decays to ~0.007 at end.
        let envelope = (-5.0 * t / CLICK_DURATION_SECS).exp();
        buf.push(sine * envelope * 0.5); // 0.5 peak amplitude
    }

    buf
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_metronome_has_pre_rendered_clicks() {
        let met = Metronome::new(44_100);
        assert!(!met.downbeat_click.is_empty());
        assert!(!met.beat_click.is_empty());
        // ~10ms at 44.1kHz = ~441 samples.
        assert!(met.downbeat_click.len() > 400);
        assert!(met.downbeat_click.len() < 500);
    }

    #[test]
    fn click_samples_are_finite() {
        let met = Metronome::new(44_100);
        for s in &met.downbeat_click {
            assert!(s.is_finite());
        }
        for s in &met.beat_click {
            assert!(s.is_finite());
        }
    }

    #[test]
    fn click_samples_bounded() {
        let met = Metronome::new(44_100);
        for s in &met.downbeat_click {
            assert!(s.abs() <= 1.0);
        }
        for s in &met.beat_click {
            assert!(s.abs() <= 1.0);
        }
    }

    #[test]
    fn generate_produces_output_at_beat_zero() {
        let mut met = Metronome::new(44_100);
        met.volume = 1.0;
        let mut output = vec![0.0_f32; 1024]; // 512 stereo samples

        // Generate starting at position 0 (beat boundary).
        met.generate(&mut output, 0, 120.0, 4, 512);

        // The first few stereo samples should be non-zero (click playing).
        let has_nonzero = output.iter().any(|&s| s.abs() > 0.001);
        assert!(has_nonzero, "Expected click output at beat 0");
    }

    #[test]
    fn generate_no_output_when_volume_zero() {
        let mut met = Metronome::new(44_100);
        met.set_volume(Db::SILENCE);
        let mut output = vec![0.0_f32; 1024];

        met.generate(&mut output, 0, 120.0, 4, 512);

        let all_zero = output.iter().all(|&s| s.abs() < f32::EPSILON);
        assert!(all_zero, "Expected silence with zero volume");
    }

    #[test]
    fn generate_invalid_bpm_is_safe() {
        let mut met = Metronome::new(44_100);
        let mut output = vec![0.0_f32; 256];

        // These should not panic.
        met.generate(&mut output, 0, 0.0, 4, 128);
        met.generate(&mut output, 0, -10.0, 4, 128);
        met.generate(&mut output, 0, f64::NAN, 4, 128);
        met.generate(&mut output, 0, f64::INFINITY, 4, 128);
    }

    #[test]
    fn generate_zero_beats_per_bar_is_safe() {
        let mut met = Metronome::new(44_100);
        let mut output = vec![0.0_f32; 256];
        met.generate(&mut output, 0, 120.0, 0, 128);
    }

    #[test]
    fn reset_clears_playback() {
        let mut met = Metronome::new(44_100);
        met.volume = 1.0;
        let mut output = vec![0.0_f32; 512];

        // Start a click.
        met.generate(&mut output, 0, 120.0, 4, 256);
        assert!(met.click_pos < met.downbeat_click.len());

        // Reset.
        met.reset();
        assert_eq!(met.click_pos, usize::MAX);
        assert_eq!(met.last_triggered_beat, u64::MAX);
    }

    #[test]
    fn downbeat_detected_at_bar_boundary() {
        let mut met = Metronome::new(44_100);
        met.volume = 1.0;

        // At 120 BPM, samples_per_beat = 44100 * 60 / 120 = 22050.
        // Beat 0 is at sample 0 (downbeat), beat 1 at 22050, beat 2 at 44100,
        // beat 3 at 66150, beat 4 at 88200 (next downbeat in 4/4).
        let mut output = vec![0.0_f32; 512];

        // Generate at position 0 — should trigger downbeat.
        met.generate(&mut output, 0, 120.0, 4, 256);
        assert!(met.is_downbeat, "Beat 0 should be downbeat");

        // Generate at position 22050 — beat 1 (not downbeat).
        met.reset();
        output.fill(0.0);
        met.generate(&mut output, 22050, 120.0, 4, 256);
        assert!(!met.is_downbeat, "Beat 1 should not be downbeat");
    }

    #[test]
    fn render_click_produces_correct_length() {
        let buf = render_click(44_100, 1000.0);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let expected = (44_100.0_f32 * CLICK_DURATION_SECS) as usize;
        assert_eq!(buf.len(), expected);
    }

    #[test]
    fn render_click_zero_sample_rate() {
        // Should produce at least 1 sample.
        let buf = render_click(0, 1000.0);
        assert!(!buf.is_empty());
    }

    #[test]
    fn new_metronome_zero_sample_rate() {
        let met = Metronome::new(0);
        assert_eq!(met.sample_rate, 1);
        assert!(!met.downbeat_click.is_empty());
    }

    #[test]
    fn generate_output_is_always_finite() {
        let mut met = Metronome::new(44_100);
        met.volume = 1.0;
        let mut output = vec![0.0_f32; 1024];
        met.generate(&mut output, 0, 120.0, 4, 512);
        for s in &output {
            assert!(s.is_finite(), "output sample must be finite");
        }
    }

    #[test]
    fn generate_with_nan_in_output_preserves_nan() {
        // The metronome mixes additively: NaN + finite = NaN.
        // This verifies the metronome does not introduce additional NaN
        // on its own (only inherits pre-existing NaN from the buffer).
        // The processing pipeline sanitizes after metronome mixing.
        let mut met = Metronome::new(44_100);
        met.volume = 1.0;
        let mut output = vec![f32::NAN; 1024];
        met.generate(&mut output, 0, 120.0, 4, 512);
        // All samples should still be NaN (since NaN + anything = NaN)
        // or possibly NaN-free in regions where no click was added.
        // This test just verifies no panic occurs.
    }

    #[test]
    fn generate_extreme_volume_stays_finite() {
        let mut met = Metronome::new(44_100);
        // Maximum possible volume from Db(+24.0).
        met.volume = crate::Db::new(24.0).to_linear();
        let mut output = vec![0.0_f32; 1024];
        met.generate(&mut output, 0, 120.0, 4, 512);
        for s in &output {
            assert!(s.is_finite(), "output must be finite even at max volume");
        }
    }

    #[test]
    fn no_spurious_click_on_resume_from_mid_beat() {
        let mut met = Metronome::new(44_100);
        met.volume = 1.0;
        met.reset();

        // At 120 BPM, samples_per_beat = 22050.
        // Resume from position 11025 (exactly mid-beat).
        let mut output = vec![0.0_f32; 256];
        met.generate(&mut output, 11025, 120.0, 4, 128);

        // No click should fire when resuming mid-beat.
        let all_zero = output.iter().all(|&s| s.abs() < f32::EPSILON);
        assert!(all_zero, "should not fire click when resuming mid-beat");
    }
}
