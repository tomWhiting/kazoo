//! Minimoog synthesis engine.
//!
//! Monophonic. Three VCOs -> Mixer -> Ladder Filter -> VCA.
//! Lowest-note priority. Rate-based glide. Cross-modulation.

pub mod envelope;
pub mod glide;
pub mod ladder;
pub mod mixer;
pub mod oscillator;
pub mod xmod;

use kazoo_core::{sanitize_sample, soft_limit};

use self::envelope::AdsrEnvelope;
use self::glide::Glide;
use self::ladder::MoogLadder;
use self::mixer::OscMixer;
use self::oscillator::{OctaveRange, Oscillator, Waveform};
use self::xmod::CrossMod;

// ---------------------------------------------------------------------------
// Note stack (lowest-note priority)
// ---------------------------------------------------------------------------

/// Maximum number of simultaneously held notes.
const MAX_HELD_NOTES: usize = 16;

/// A monophonic note stack with lowest-note priority.
///
/// When multiple notes are held, the lowest pitch plays. On note-off,
/// the voice reassigns to the next-lowest remaining held note.
#[derive(Debug)]
struct NoteStack {
    /// Currently held MIDI note numbers, in insertion order.
    held: [u8; MAX_HELD_NOTES],
    /// Number of currently held notes.
    count: usize,
}

impl NoteStack {
    const fn new() -> Self {
        Self {
            held: [0; MAX_HELD_NOTES],
            count: 0,
        }
    }

    /// Add a note to the stack. Returns the new active (lowest) note.
    fn note_on(&mut self, note: u8) -> Option<u8> {
        // Don't add duplicates
        if self.held[..self.count].contains(&note) {
            return self.active_note();
        }
        if self.count < MAX_HELD_NOTES {
            self.held[self.count] = note;
            self.count += 1;
        }
        self.active_note()
    }

    /// Remove a note from the stack. Returns the new active note (or None if empty).
    fn note_off(&mut self, note: u8) -> Option<u8> {
        if let Some(pos) = self.held[..self.count].iter().position(|&n| n == note) {
            // Remove by shifting
            for i in pos..self.count.saturating_sub(1) {
                self.held[i] = self.held[i + 1];
            }
            self.count = self.count.saturating_sub(1);
        }
        self.active_note()
    }

    /// The currently active note (lowest held note).
    fn active_note(&self) -> Option<u8> {
        if self.count == 0 {
            return None;
        }
        let mut lowest = self.held[0];
        for &note in &self.held[1..self.count] {
            if note < lowest {
                lowest = note;
            }
        }
        Some(lowest)
    }

    /// Whether any notes are held.
    const fn is_active(&self) -> bool {
        self.count > 0
    }

    /// Clear all held notes.
    const fn clear(&mut self) {
        self.count = 0;
    }
}

// ---------------------------------------------------------------------------
// MiniVoice — complete monophonic voice engine
// ---------------------------------------------------------------------------

/// The complete Minimoog voice engine.
///
/// Monophonic voice with three VCOs, oscillator mixer, Moog ladder filter,
/// filter and amplitude ADSR envelopes, rate-based glide, and cross-modulation.
///
/// This is the audio processing core. The TUI owns one of these and calls
/// `process_block()` from the audio output callback.
#[derive(Debug)]
pub struct MiniVoice {
    // Oscillators
    pub osc1: Oscillator,
    pub osc2: Oscillator,
    pub osc3: Oscillator,

    // Mixer
    pub mixer: OscMixer,

    // Filter
    pub filter: MoogLadder,
    /// Filter envelope amount (0.0 to 1.0): how much the filter ADSR
    /// modulates cutoff.
    pub filter_env_amount: f32,

    // Envelopes
    pub filter_env: AdsrEnvelope,
    pub amp_env: AdsrEnvelope,

    // Performance
    pub glide: Glide,
    pub xmod: CrossMod,
    /// Legato mode: when true, overlapping notes don't retrigger envelopes.
    pub legato: bool,
    /// Whether retrigger mode is active (opposite of legato for envelopes).
    pub retrigger: bool,

    // Note management
    note_stack: NoteStack,
    /// Current MIDI note number (for display).
    current_note: Option<u8>,

    sample_rate: f32,

    /// Pre-allocated scratch buffer for output waveform display.
    /// Stores the last block's output for the TUI waveform monitor.
    display_buffer: Vec<f32>,
    display_write_pos: usize,
}

/// Display buffer size (enough for ~1 frame at 60fps at 44.1kHz).
const DISPLAY_BUFFER_SIZE: usize = 1024;

impl MiniVoice {
    /// Create a new voice engine at the given sample rate.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(1.0);

        let mut osc1 = Oscillator::new(sr);
        osc1.waveform = Waveform::Saw;
        osc1.octave = OctaveRange::Footage8;
        osc1.level = 0.8;

        let mut osc2 = Oscillator::new(sr);
        osc2.waveform = Waveform::Saw;
        osc2.octave = OctaveRange::Footage8;
        osc2.fine_tune_cents = 2.0; // slight detune for fatness
        osc2.level = 0.75;

        let mut osc3 = Oscillator::new(sr);
        osc3.waveform = Waveform::Triangle;
        osc3.octave = OctaveRange::Footage8;
        osc3.fine_tune_cents = -1.0;
        osc3.level = 0.0;

        let filter = MoogLadder::new(sr);

        let mut filter_env = AdsrEnvelope::new(sr);
        filter_env.attack = 0.005;
        filter_env.decay = 0.2;
        filter_env.sustain = 0.3;
        filter_env.release = 0.15;
        filter_env.recompute_coefficients();

        let mut amp_env = AdsrEnvelope::new(sr);
        amp_env.attack = 0.002;
        amp_env.decay = 0.3;
        amp_env.sustain = 0.6;
        amp_env.release = 0.1;
        amp_env.recompute_coefficients();

        Self {
            osc1,
            osc2,
            osc3,
            mixer: OscMixer::new(),
            filter,
            filter_env_amount: 0.7,
            filter_env,
            amp_env,
            glide: Glide::new(sr),
            xmod: CrossMod::new(),
            legato: true,
            retrigger: false,
            note_stack: NoteStack::new(),
            current_note: None,
            sample_rate: sr,
            display_buffer: vec![0.0; DISPLAY_BUFFER_SIZE],
            display_write_pos: 0,
        }
    }

    /// MIDI note on.
    pub fn note_on(&mut self, note: u8) {
        let was_active = self.note_stack.is_active();
        let active = self.note_stack.note_on(note);

        if let Some(active_note) = active {
            let freq = kazoo_core::midi_note_to_frequency(active_note);
            let is_legato = was_active && self.legato;

            self.glide.set_target(freq, is_legato);
            self.current_note = Some(active_note);

            // Trigger envelopes (respecting legato mode)
            let should_retrigger = self.retrigger || !is_legato;
            self.filter_env.gate_on(should_retrigger);
            self.amp_env.gate_on(should_retrigger);
        }
    }

    /// MIDI note off.
    pub fn note_off(&mut self, note: u8) {
        let active = self.note_stack.note_off(note);

        if let Some(new_note) = active {
            // Reassign to next-lowest note
            let freq = kazoo_core::midi_note_to_frequency(new_note);
            self.glide.set_target(freq, true); // always glide on reassignment
            self.current_note = Some(new_note);
        } else {
            // No more notes held — release envelopes
            self.filter_env.gate_off();
            self.amp_env.gate_off();
            self.current_note = None;
        }
    }

    /// Process a block of audio, writing mono output to `output`.
    ///
    /// This is called from the audio output callback. No allocations,
    /// no locks, no I/O.
    pub fn process_block(&mut self, output: &mut [f32]) {
        for sample in output.iter_mut() {
            *sample = self.process_sample();
        }
    }

    /// Process a single sample through the complete voice chain.
    #[inline]
    fn process_sample(&mut self) -> f32 {
        // 1. Glide: get current pitch frequency
        let pitch_freq = self.glide.tick();
        if pitch_freq <= 0.0 {
            // No active note
            let env = self.amp_env.tick();
            let _ = self.filter_env.tick();
            if env < 1e-6 {
                self.store_display_sample(0.0);
                return 0.0;
            }
        }

        // 2. Compute oscillator frequencies with cross-mod

        // Osc 3: may be in LFO mode (disconnected from keyboard tracking)
        let osc3_freq = if self.osc3.lfo_mode {
            // Fixed LFO frequency based on fine tune knob
            // Map fine tune -50..+50 to 0.1..20 Hz
            let base_lfo = 2.0; // 2 Hz base rate
            self.osc3.effective_frequency(base_lfo)
        } else {
            self.osc3.effective_frequency(pitch_freq)
        };

        // Generate Osc 3 first (needed for cross-mod)
        let osc3_out = self.osc3.tick(osc3_freq);

        // Mod wheel modulation from Osc 3 (when in LFO mode)
        let pitch_mod = if self.osc3.lfo_mode {
            self.xmod.mod_wheel_pitch_multiplier(osc3_out)
        } else {
            1.0
        };

        // Osc 1: base frequency with mod wheel vibrato
        let osc1_freq = self.osc1.effective_frequency(pitch_freq) * pitch_mod;
        let osc1_out = self.osc1.tick(osc1_freq);

        // Osc 2: with Osc3->Osc2 FM and mod wheel vibrato
        let fm_mult = self.xmod.osc2_fm_multiplier(osc3_out);
        let osc2_freq = self.osc2.effective_frequency(pitch_freq) * pitch_mod * fm_mult;
        let osc2_out = self.osc2.tick(osc2_freq);

        // 3. Mix oscillators
        let mixed = self.mixer.mix(osc1_out, osc2_out, osc3_out, 0.0);

        // 4. Filter envelope -> cutoff modulation
        let filter_env_val = self.filter_env.tick();
        let base_cutoff = self.filter.base_cutoff;

        // Envelope modulates cutoff: env * amount * (max - base) + base
        let env_cutoff_offset =
            filter_env_val * self.filter_env_amount * (MoogLadder::MAX_CUTOFF - base_cutoff);
        let mut effective_cutoff = base_cutoff + env_cutoff_offset;

        // Osc 2 -> filter cutoff modulation
        effective_cutoff += self.xmod.filter_mod_hz(osc2_out, base_cutoff);

        // Mod wheel -> filter (when Osc 3 is LFO)
        if self.osc3.lfo_mode {
            effective_cutoff += self.xmod.mod_wheel_filter_hz(osc3_out, base_cutoff);
        }

        // Keyboard tracking
        if self.filter.key_track > 0.0 && pitch_freq > 0.0 {
            let ratio = pitch_freq / 261.63;
            let tracking_semitones = ratio.log2() * 12.0 * self.filter.key_track;
            let multiplier = (tracking_semitones / 12.0).exp2();
            effective_cutoff *= multiplier;
        }

        self.filter
            .set_cutoff(effective_cutoff.clamp(MoogLadder::MIN_CUTOFF, MoogLadder::MAX_CUTOFF));

        // 5. Filter
        let filtered = self.filter.process_sample(mixed);

        // 6. Amplitude envelope (VCA)
        let amp_env_val = self.amp_env.tick();
        let output = filtered * amp_env_val;

        // 7. Soft limit and sanitize
        let output = soft_limit(sanitize_sample(output));

        self.store_display_sample(output);
        output
    }

    /// Store a sample in the circular display buffer.
    #[inline]
    fn store_display_sample(&mut self, sample: f32) {
        if !self.display_buffer.is_empty() {
            self.display_buffer[self.display_write_pos] = sample;
            self.display_write_pos = (self.display_write_pos + 1) % self.display_buffer.len();
        }
    }

    /// Get a snapshot of the display buffer for the TUI waveform monitor.
    ///
    /// In the command-channel architecture, the audio-thread voice and
    /// UI-thread voice are separate instances. The audio callback copies
    /// this buffer into a `DisplaySnapshot` sent via crossbeam channel.
    #[must_use]
    pub fn display_samples(&self) -> &[f32] {
        &self.display_buffer
    }

    /// Get the current write position in the circular display buffer.
    ///
    /// Used by the TUI to linearize the ring buffer before rendering.
    #[must_use]
    pub const fn display_write_pos(&self) -> usize {
        self.display_write_pos
    }

    /// Set the display write position (for snapshot copies from the audio thread).
    pub const fn set_display_write_pos(&mut self, pos: usize) {
        self.display_write_pos = pos;
    }

    /// Current active MIDI note (for display).
    #[must_use]
    pub const fn current_note(&self) -> Option<u8> {
        self.current_note
    }

    /// Get a mutable reference to the display buffer (for snapshot copies).
    pub fn display_samples_mut(&mut self) -> &mut [f32] {
        &mut self.display_buffer
    }

    /// Set the display note (for snapshot copies from the audio thread).
    pub const fn set_display_note(&mut self, note: Option<u8>) {
        self.current_note = note;
    }

    /// Set sample rate on all sub-components.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        let sr = sample_rate.max(1.0);
        self.sample_rate = sr;
        self.osc1.set_sample_rate(sr);
        self.osc2.set_sample_rate(sr);
        self.osc3.set_sample_rate(sr);
        self.filter.set_sample_rate(sr);
        self.filter_env.set_sample_rate(sr);
        self.amp_env.set_sample_rate(sr);
        self.glide.set_sample_rate(sr);
    }

    /// Reset all state.
    pub fn reset(&mut self) {
        self.osc1.reset();
        self.osc2.reset();
        self.osc3.reset();
        self.filter.reset();
        self.filter_env.reset();
        self.amp_env.reset();
        self.glide.reset();
        self.mixer.reset();
        self.note_stack.clear();
        self.current_note = None;
        self.display_buffer.fill(0.0);
        self.display_write_pos = 0;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_stack_lowest_priority() {
        let mut stack = NoteStack::new();
        stack.note_on(60); // C4
        stack.note_on(64); // E4
        stack.note_on(55); // G3

        assert_eq!(stack.active_note(), Some(55), "lowest should be G3 (55)");
    }

    #[test]
    fn note_stack_reassignment() {
        let mut stack = NoteStack::new();
        stack.note_on(60);
        stack.note_on(55);
        assert_eq!(stack.active_note(), Some(55));

        stack.note_off(55);
        assert_eq!(
            stack.active_note(),
            Some(60),
            "should reassign to C4 after G3 released"
        );
    }

    #[test]
    fn note_stack_empty() {
        let mut stack = NoteStack::new();
        stack.note_on(60);
        stack.note_off(60);
        assert_eq!(stack.active_note(), None);
        assert!(!stack.is_active());
    }

    #[test]
    fn note_stack_no_duplicates() {
        let mut stack = NoteStack::new();
        stack.note_on(60);
        stack.note_on(60);
        assert_eq!(stack.count, 1);
    }

    #[test]
    fn voice_produces_output() {
        let mut voice = MiniVoice::new(44100.0);
        voice.note_on(60); // C4

        let mut output = vec![0.0; 4410]; // 100ms
        voice.process_block(&mut output);

        let max_abs = output.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
        assert!(
            max_abs > 0.01,
            "voice should produce audible output, got max={max_abs}"
        );
    }

    #[test]
    fn voice_silence_after_note_off() {
        let mut voice = MiniVoice::new(44100.0);
        voice.note_on(60);

        // Process some audio
        let mut buf = vec![0.0; 4410];
        voice.process_block(&mut buf);

        // Note off
        voice.note_off(60);

        // Process enough for release to finish
        let mut buf = vec![0.0; 44100];
        voice.process_block(&mut buf);

        // Tail should be silence
        let tail = &buf[buf.len() - 4410..];
        let max_abs = tail.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
        assert!(
            max_abs < 0.01,
            "should be silent after release, got max={max_abs}"
        );
    }

    #[test]
    fn voice_output_always_finite() {
        let mut voice = MiniVoice::new(44100.0);
        voice.note_on(60);

        let mut output = vec![0.0; 44100];
        voice.process_block(&mut output);

        for (i, &s) in output.iter().enumerate() {
            assert!(
                s.is_finite() && s >= -1.0 && s <= 1.0,
                "sample {i} out of range: {s}"
            );
        }
    }

    #[test]
    fn voice_legato_glides() {
        let mut voice = MiniVoice::new(44100.0);
        voice.legato = true;
        voice.glide.enabled = true;
        voice.glide.rate = 60.0;

        voice.note_on(60); // C4
        let mut buf = vec![0.0; 1000];
        voice.process_block(&mut buf);

        voice.note_on(48); // C3 — lower than C4, so lowest-note priority changes pitch
        // The glide should be active (pitch changed from C4 to C3)
        assert!(voice.glide.is_gliding());
    }
}
