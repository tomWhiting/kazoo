//! kazoo-string's engine: eight waveguide strings, a few ghost strings that
//! let stolen notes die away instead of clicking, and the body they share.
//!
//! Notes are panned across the keyboard with equal-power gains, so a chord
//! spreads from left to right and the body rings in the centre.
//!
//! Pure physical-model synthesis, no samples. Everything is allocated in
//! [`StringSynth::new`]; [`StringSynth::process`] never allocates, locks or
//! blocks, and never emits NaN or infinity.

use crate::body::Body;
use crate::patch::{PATCHES, Patch};
use crate::waveguide::{StringLine, StringTuning, loop_gains};

/// Polyphony.
pub const VOICES: usize = 8;
/// Stolen strings still fading out at any moment.
const GHOSTS: usize = 4;
/// Samples kept for the scope.
pub const SCOPE_LEN: usize = 512;
/// A stolen string falls 60 dB in this many seconds.
const STEAL_T60: f32 = 0.025;
/// Level of a full-velocity pluck at the strings.
const PLUCK_LEVEL: f32 = 0.3;
/// How far the keyboard's ends sit from the centre: 0 is mono, 1 is hard left/right.
const PAN_SPREAD: f32 = 0.55;
/// Level of the summed strings before the master fader.
const VOICE_MIX_GAIN: f32 = 0.8;

/// Who started a note. A looping phrase only ever releases its own voices, so
/// it never cuts off a note someone is holding on the keyboard or the hub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Computer keyboard or hub note events.
    Player,
    /// The looping `--phrase`.
    Phrase,
}

#[derive(Debug)]
struct Voice {
    line: StringLine,
    note: u8,
    source: Source,
    /// The key is down (or the phrase note has not ended).
    gate: bool,
    started: u64,
    /// Left and right gains of this note's pan position.
    pan: [f32; 2],
}

/// A stolen string fading out, with the pan of the note it was.
#[derive(Debug)]
struct Ghost {
    line: StringLine,
    pan: [f32; 2],
}

impl Voice {
    fn new(sample_rate: f32) -> Self {
        Self {
            line: StringLine::new(sample_rate),
            note: 0,
            source: Source::Player,
            gate: false,
            started: 0,
            pan: pan_gains(60),
        }
    }

    fn is_sounding(&self) -> bool {
        self.gate || self.line.is_sounding()
    }
}

/// The polyphonic plucked-string synth.
#[derive(Debug)]
pub struct StringSynth {
    sample_rate: f32,
    patch: Patch,
    voices: Vec<Voice>,
    ghosts: Vec<Ghost>,
    body: Body,
    clock: u64,
    master: f32,
    scope: [f32; SCOPE_LEN],
    scope_pos: usize,
}

impl StringSynth {
    /// Create an engine at the given sample rate with the first factory patch.
    ///
    /// Non-finite or non-positive sample rates fall back to 48 kHz.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let sample_rate = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };
        Self {
            sample_rate,
            patch: PATCHES[0],
            voices: (0..VOICES).map(|_| Voice::new(sample_rate)).collect(),
            ghosts: (0..GHOSTS)
                .map(|_| Ghost {
                    line: StringLine::new(sample_rate),
                    pan: pan_gains(60),
                })
                .collect(),
            body: Body::new(sample_rate),
            clock: 0,
            master: 0.8,
            scope: [0.0; SCOPE_LEN],
            scope_pos: 0,
        }
    }

    /// Change the voicing. New notes use it in full; strings already ringing
    /// take the new decay and release times but keep their pitch and pluck.
    pub fn set_patch(&mut self, patch: Patch) {
        self.patch = patch.sanitized();
        let (decay, release) = (self.patch.decay_seconds(), self.patch.release_seconds());
        let rate = self.sample_rate;
        for voice in self.voices.iter_mut().filter(|v| v.line.is_sounding()) {
            let tuning = *voice.line.tuning();
            let (gain, release_gain) = loop_gains(rate, tuning.hz, tuning.blend, decay, release);
            voice.line.set_gains(gain, release_gain, voice.gate);
        }
    }

    /// Set the master level, 0 to 1. A non-finite value is ignored.
    pub const fn set_master(&mut self, value: f32) {
        if value.is_finite() {
            self.master = value.clamp(0.0, 1.0);
        }
    }

    /// Pluck a note for the keyboard or hub.
    pub fn note_on(&mut self, note: u8, velocity: u8) {
        self.note_on_from(Source::Player, note, velocity);
    }

    /// Pluck a note on behalf of a source. Velocity 0 is a note-off.
    pub fn note_on_from(&mut self, source: Source, note: u8, velocity: u8) {
        if velocity == 0 {
            self.note_off_from(source, note);
            return;
        }
        let note = note.min(127);
        let velocity = f32::from(velocity.min(127)) / 127.0;
        let slot = self.pick_voice(source, note);
        self.clock = self.clock.wrapping_add(1);

        let reuse = {
            let voice = &self.voices[slot];
            voice.line.is_sounding() && voice.note == note && voice.source == source
        };
        if !reuse && self.voices[slot].line.is_sounding() {
            self.retire(slot);
        }

        let hz = midi_to_hz(note);
        let tuning = StringTuning::solve(
            self.sample_rate,
            hz,
            self.patch.blend(),
            self.patch.stiffness,
            self.patch.decay_seconds(),
            self.patch.release_seconds(),
        );
        let amplitude = PLUCK_LEVEL * velocity.powf(1.3);
        let brightness = self.patch.pick_brightness(velocity);
        let position = self.patch.pick_position();
        let clock = self.clock;
        let voice = &mut self.voices[slot];
        voice.note = note;
        voice.source = source;
        voice.gate = true;
        voice.started = clock;
        voice.pan = pan_gains(note);
        voice.line.pluck(tuning, amplitude, brightness, position);
    }

    /// Hand a sounding voice's string to a ghost so it can fade out while the
    /// voice is plucked afresh. With every ghost busy the quietest is cut.
    fn retire(&mut self, slot: usize) {
        let ghost = self
            .ghosts
            .iter()
            .position(|g| !g.line.is_sounding())
            .or_else(|| {
                self.ghosts
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| a.line.level().total_cmp(&b.line.level()))
                    .map(|(i, _)| i)
            });
        if let Some(ghost) = ghost {
            std::mem::swap(&mut self.voices[slot].line, &mut self.ghosts[ghost].line);
            self.ghosts[ghost].pan = self.voices[slot].pan;
            self.ghosts[ghost].line.mute(STEAL_T60);
            // Whatever the ghost held before is gone; the voice's string is clean.
            self.voices[slot].line.clear();
        }
    }

    /// Release every player voice holding this note.
    pub fn note_off(&mut self, note: u8) {
        self.note_off_from(Source::Player, note);
    }

    /// Release every voice this source started on this note.
    pub fn note_off_from(&mut self, source: Source, note: u8) {
        for voice in self
            .voices
            .iter_mut()
            .filter(|v| v.gate && v.note == note && v.source == source)
        {
            voice.gate = false;
            voice.line.damp();
        }
    }

    /// Release every held voice one source started.
    pub fn release_source(&mut self, source: Source) {
        for voice in self
            .voices
            .iter_mut()
            .filter(|v| v.gate && v.source == source)
        {
            voice.gate = false;
            voice.line.damp();
        }
    }

    /// Release every voice.
    pub fn all_notes_off(&mut self) {
        for voice in &mut self.voices {
            if voice.gate {
                voice.gate = false;
                voice.line.damp();
            }
        }
    }

    /// Choose a voice: same note first, then a free voice, then the oldest
    /// released voice, then the oldest held voice.
    fn pick_voice(&self, source: Source, note: u8) -> usize {
        if let Some(i) = self
            .voices
            .iter()
            .position(|v| v.note == note && v.source == source && v.is_sounding())
        {
            return i;
        }
        if let Some(i) = self.voices.iter().position(|v| !v.is_sounding()) {
            return i;
        }
        let oldest = |held: bool| {
            self.voices
                .iter()
                .enumerate()
                .filter(|(_, v)| v.gate == held)
                .min_by_key(|(_, v)| v.started)
                .map(|(i, _)| i)
        };
        oldest(false).or_else(|| oldest(true)).unwrap_or(0)
    }

    /// Notes currently held or ringing, with gate state, for display.
    #[must_use]
    pub fn voice_states(&self) -> [Option<(u8, bool)>; VOICES] {
        let mut states = [None; VOICES];
        for (state, voice) in states.iter_mut().zip(&self.voices) {
            if voice.is_sounding() {
                *state = Some((voice.note, voice.gate));
            }
        }
        states
    }

    /// Most recent output samples, oldest first starting at `scope_pos`.
    #[must_use]
    pub const fn scope(&self) -> (&[f32; SCOPE_LEN], usize) {
        (&self.scope, self.scope_pos)
    }

    /// Render one stereo frame `[left, right]`, limited and NaN-safe.
    pub fn process(&mut self) -> [f32; 2] {
        let mut dry = [0.0_f32; 2];
        for voice in &mut self.voices {
            if voice.line.is_sounding() {
                let sample = voice.line.process();
                dry[0] = sample.mul_add(voice.pan[0], dry[0]);
                dry[1] = sample.mul_add(voice.pan[1], dry[1]);
            }
        }
        for ghost in &mut self.ghosts {
            if ghost.line.is_sounding() {
                let sample = ghost.line.process();
                dry[0] = sample.mul_add(ghost.pan[0], dry[0]);
                dry[1] = sample.mul_add(ghost.pan[1], dry[1]);
            }
        }
        // The body is one wooden box: it hears the centre of the mix and rings
        // equally in both channels.
        let mid = 0.5 * (dry[0] + dry[1]) * VOICE_MIX_GAIN;
        let wet = self.body.process(mid, self.patch.body) - mid;
        let out = dry.map(|d| {
            let mixed = d.mul_add(VOICE_MIX_GAIN, wet) * self.master;
            kazoo_core::sanitize_sample(kazoo_core::soft_limit(mixed))
        });
        self.scope[self.scope_pos] = 0.5 * (out[0] + out[1]);
        self.scope_pos = (self.scope_pos + 1) % SCOPE_LEN;
        out
    }
}

/// Equal-power `[left, right]` gains for a note: low notes sit left, high
/// notes right, and middle C is dead centre at unity gain in both channels.
#[must_use]
pub fn pan_gains(note: u8) -> [f32; 2] {
    let pan = ((f32::from(note.min(127)) - 60.0) / 36.0).clamp(-1.0, 1.0) * PAN_SPREAD;
    let angle = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
    [
        angle.cos() * std::f32::consts::SQRT_2,
        angle.sin() * std::f32::consts::SQRT_2,
    ]
}

/// MIDI note number to frequency in hertz (A4 = 440 Hz).
#[must_use]
pub fn midi_to_hz(note: u8) -> f32 {
    440.0 * ((f32::from(note) - 69.0) / 12.0).exp2()
}

/// MIDI note number to a name such as `C#4`.
#[must_use]
pub fn note_name(note: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let octave = i16::from(note) / 12 - 1;
    format!("{}{octave}", NAMES[usize::from(note % 12)])
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;

    /// The centre of the stereo image.
    fn run(synth: &mut StringSynth, samples: usize) -> Vec<f32> {
        (0..samples)
            .map(|_| {
                let [l, r] = synth.process();
                0.5 * (l + r)
            })
            .collect()
    }

    /// Both channels, left then right.
    fn run_stereo(synth: &mut StringSynth, samples: usize) -> [Vec<f32>; 2] {
        let mut out = [Vec::with_capacity(samples), Vec::with_capacity(samples)];
        for _ in 0..samples {
            let [l, r] = synth.process();
            out[0].push(l);
            out[1].push(r);
        }
        out
    }

    fn peak(signal: &[f32]) -> f32 {
        signal.iter().fold(0.0_f32, |m, s| m.max(s.abs()))
    }

    fn rms(signal: &[f32]) -> f32 {
        (signal.iter().map(|s| s * s).sum::<f32>() / signal.len().max(1) as f32).sqrt()
    }

    fn db(x: f32) -> f32 {
        20.0 * x.log10()
    }

    #[test]
    fn silent_without_notes() {
        let mut synth = StringSynth::new(RATE);
        assert!(
            run(&mut synth, 2_000)
                .iter()
                .all(|s| s.abs() < f32::EPSILON)
        );
        assert!(synth.voice_states().iter().all(Option::is_none));
    }

    #[test]
    fn every_patch_makes_finite_sound_at_a_sane_level() {
        for patch in PATCHES {
            for note in [36_u8, 48, 60, 72, 84] {
                let mut synth = StringSynth::new(RATE);
                synth.set_patch(patch);
                synth.note_on(note, 100);
                let signal = run(&mut synth, 24_000);
                assert!(signal.iter().all(|s| s.is_finite()), "{}", patch.name);
                let level = db(peak(&signal));
                assert!(
                    (-24.0..=-3.0).contains(&level),
                    "{} note {note}: peak {level:.1} dBFS",
                    patch.name
                );
            }
        }
    }

    #[test]
    fn full_chords_never_clip() {
        for patch in PATCHES {
            let mut synth = StringSynth::new(RATE);
            synth.set_patch(patch);
            synth.set_master(1.0);
            for note in [36_u8, 48, 55, 60, 64, 67, 72, 76] {
                synth.note_on(note, 127);
            }
            for channel in run_stereo(&mut synth, 24_000) {
                assert!(peak(&channel) <= 1.0, "{}", patch.name);
            }
        }
    }

    #[test]
    fn released_note_falls_silent_and_frees_its_voice() {
        let mut synth = StringSynth::new(RATE);
        synth.note_on(60, 100);
        run(&mut synth, 4_800);
        assert_eq!(synth.voice_states()[0], Some((60, true)));
        synth.note_off(60);
        assert_eq!(synth.voice_states()[0], Some((60, false)));
        run(&mut synth, RATE as usize * 3);
        assert!(synth.voice_states().iter().all(Option::is_none));
    }

    #[test]
    fn holding_rings_longer_than_releasing() {
        let level_after = |release: bool| {
            let mut synth = StringSynth::new(RATE);
            synth.note_on(55, 100);
            run(&mut synth, 4_800);
            if release {
                synth.note_off(55);
            }
            run(&mut synth, 24_000);
            rms(&run(&mut synth, 4_800))
        };
        assert!(level_after(true) < level_after(false) * 0.2);
    }

    #[test]
    fn steals_the_oldest_released_voice_then_the_oldest_held() {
        let mut synth = StringSynth::new(RATE);
        for note in 60..68 {
            synth.note_on(note, 100);
        }
        synth.note_off(63);
        synth.note_on(70, 100);
        let states = synth.voice_states();
        assert!(states.iter().flatten().all(|&(n, _)| n != 63));
        assert!(states.contains(&Some((70, true))));
        synth.note_on(71, 100);
        let states = synth.voice_states();
        assert!(states.iter().flatten().all(|&(n, _)| n != 60));
        assert_eq!(states.iter().flatten().count(), VOICES);
    }

    #[test]
    fn stealing_does_not_click() {
        let mut synth = StringSynth::new(RATE);
        synth.set_patch(PATCHES[0]);
        for note in 48..56 {
            synth.note_on(note, 100);
        }
        run(&mut synth, 2_400);
        let before = run(&mut synth, 480);
        let slope = before
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0_f32, f32::max);
        // Steal a loud string; the stolen sound fades instead of being cut.
        synth.note_on(90, 20);
        let after = run(&mut synth, 480);
        let mut last = before[before.len() - 1];
        let mut worst = 0.0_f32;
        for &s in &after {
            worst = worst.max((s - last).abs());
            last = s;
        }
        assert!(
            worst < slope * 1.5 + 0.01,
            "step {worst} against a normal slope of {slope}"
        );
        assert!(after.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn repeated_note_reuses_its_voice() {
        let mut synth = StringSynth::new(RATE);
        synth.note_on(60, 100);
        run(&mut synth, 2_400);
        synth.note_on(60, 100);
        assert_eq!(synth.voice_states().iter().flatten().count(), 1);
    }

    #[test]
    fn zero_velocity_is_note_off() {
        let mut synth = StringSynth::new(RATE);
        synth.note_on(60, 100);
        synth.note_on(60, 0);
        assert_eq!(synth.voice_states()[0], Some((60, false)));
    }

    #[test]
    fn velocity_scales_loudness_and_brightness() {
        let play = |velocity: u8| {
            let mut synth = StringSynth::new(RATE);
            synth.set_patch(PATCHES[1]);
            synth.note_on(60, velocity);
            rms(&run(&mut synth, 9_600))
        };
        assert!(play(127) > play(30) * 1.5);
    }

    #[test]
    fn hostile_values_are_sanitized() {
        let mut synth = StringSynth::new(f32::NAN);
        let mut patch = PATCHES[0];
        patch.decay = f32::NAN;
        patch.damping = f32::INFINITY;
        patch.stiffness = -4.0;
        patch.body = 99.0;
        synth.set_patch(patch);
        synth.set_master(f32::NAN);
        for note in [0_u8, 127, 255] {
            synth.note_on(note, 255);
        }
        let signal = run(&mut synth, 9_600);
        assert!(signal.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
    }

    #[test]
    fn master_scales_output() {
        let play = |master: f32| {
            let mut synth = StringSynth::new(RATE);
            synth.set_master(master);
            synth.note_on(60, 100);
            peak(&run(&mut synth, 4_800))
        };
        assert!(play(0.0) < f32::EPSILON);
        assert!(play(0.4) < play(0.8));
    }

    #[test]
    fn live_decay_edit_changes_the_ring_but_not_the_pitch() {
        let ring = |decay: f32| {
            let mut synth = StringSynth::new(RATE);
            let mut patch = PATCHES[2];
            patch.decay = 0.9;
            synth.set_patch(patch);
            synth.note_on(57, 100);
            run(&mut synth, 2_400);
            patch.decay = decay;
            synth.set_patch(patch);
            run(&mut synth, 48_000);
            rms(&run(&mut synth, 4_800))
        };
        assert!(ring(0.1) < ring(0.9) * 0.2);
    }

    #[test]
    fn phrase_release_leaves_player_notes_alone() {
        let mut synth = StringSynth::new(RATE);
        synth.note_on(60, 100);
        synth.note_on_from(Source::Phrase, 64, 100);
        synth.release_source(Source::Phrase);
        let states = synth.voice_states();
        assert!(states.contains(&Some((60, true))));
        assert!(states.contains(&Some((64, false))));
        synth.note_off_from(Source::Phrase, 60);
        assert!(synth.voice_states().contains(&Some((60, true))));
    }

    #[test]
    fn all_notes_off_releases_everything() {
        let mut synth = StringSynth::new(RATE);
        for note in 60..64 {
            synth.note_on(note, 100);
        }
        synth.all_notes_off();
        assert!(
            synth
                .voice_states()
                .iter()
                .flatten()
                .all(|&(_, held)| !held)
        );
    }

    #[test]
    fn the_scope_follows_the_output() {
        let mut synth = StringSynth::new(RATE);
        synth.note_on(60, 100);
        run(&mut synth, SCOPE_LEN * 2);
        let (ring, _) = synth.scope();
        assert!(peak(ring) > 0.01);
    }

    #[test]
    fn pan_is_equal_power_and_centred_on_middle_c() {
        let [l, r] = pan_gains(60);
        assert!((l - 1.0).abs() < 1.0e-5 && (r - 1.0).abs() < 1.0e-5);
        let [low_l, low_r] = pan_gains(24);
        let [high_l, high_r] = pan_gains(96);
        assert!(low_l > low_r && high_r > high_l);
        assert!((low_l - high_r).abs() < 1.0e-5, "mirror image");
        for note in 0..=255_u8 {
            let [l, r] = pan_gains(note);
            let power = r.mul_add(r, l * l);
            assert!((power - 2.0).abs() < 1.0e-4, "note {note}: {power}");
        }
    }

    #[test]
    fn low_notes_sound_left_and_high_notes_right() {
        let side = |note: u8| {
            let mut synth = StringSynth::new(RATE);
            synth.note_on(note, 100);
            let [l, r] = run_stereo(&mut synth, 9_600);
            (rms(&l), rms(&r))
        };
        let (l, r) = side(36);
        assert!(l > r * 1.1, "{l} vs {r}");
        let (l, r) = side(84);
        assert!(r > l * 1.1, "{l} vs {r}");
        let (l, r) = side(60);
        assert!((l - r).abs() < l * 0.02);
    }

    #[test]
    fn names_and_pitch() {
        assert_eq!(note_name(60), "C4");
        assert_eq!(note_name(69), "A4");
        assert_eq!(note_name(0), "C-1");
        assert!((midi_to_hz(69) - 440.0).abs() < 1.0e-3);
        assert!((midi_to_hz(57) - 220.0).abs() < 1.0e-3);
    }
}
