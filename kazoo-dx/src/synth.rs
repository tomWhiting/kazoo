//! Four-operator FM synthesis engine.
//!
//! Every sound is phase-modulated sine math: four operators per voice, routed
//! through one of eight classic 4-op algorithms, each with its own ADSR. There
//! is no sample or wavetable playback anywhere in this module.
//!
//! The engine allocates nothing after construction. It is designed to be moved
//! into the cpal output callback and driven there.

use std::f32::consts::TAU;

/// Number of operators per voice.
pub const OPERATORS: usize = 4;
/// Maximum simultaneous voices.
pub const VOICES: usize = 8;
/// Length of the scope ring buffer exposed for display.
pub const SCOPE_LEN: usize = 512;

/// Peak phase-modulation depth, in cycles, for a full-level modulator.
/// Two cycles is a modulation index of 4π, which is the DX7 region.
const MOD_DEPTH_CYCLES: f32 = 2.0;
/// Peak self-feedback depth on operator 4, in cycles.
const FEEDBACK_DEPTH_CYCLES: f32 = 0.35;
/// Vibrato depth at full setting, in cents (a quarter tone).
const MAX_VIBRATO_CENTS: f32 = 50.0;
/// Envelope level treated as silent.
const ENV_FLOOR: f32 = 1.0e-5;
/// Output gain applied to the summed voices before limiting.
const VOICE_MIX_GAIN: f32 = 0.42;

/// One 4-op routing. Operators are indexed 0..4 and shown to the user as 1..4.
///
/// Modulators always have a higher index than the operator they modulate, so
/// evaluating operators from 4 down to 1 in a single pass is always correct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Algorithm {
    /// Bitmask per operator of which operators modulate it.
    pub modulators: [u8; OPERATORS],
    /// Bitmask of operators that are heard (carriers).
    pub carriers: u8,
    /// Short routing sketch for the UI.
    pub diagram: &'static str,
}

/// The eight classic four-operator algorithms (TX81Z/DX21 family).
pub const ALGORITHMS: [Algorithm; 8] = [
    Algorithm {
        modulators: [0b0010, 0b0100, 0b1000, 0],
        carriers: 0b0001,
        diagram: "4>3>2>1",
    },
    Algorithm {
        modulators: [0b0010, 0b1100, 0, 0],
        carriers: 0b0001,
        diagram: "(3+4)>2>1",
    },
    Algorithm {
        modulators: [0b1010, 0b0100, 0, 0],
        carriers: 0b0001,
        diagram: "(3>2 + 4)>1",
    },
    Algorithm {
        modulators: [0b0110, 0, 0b1000, 0],
        carriers: 0b0001,
        diagram: "(2 + 4>3)>1",
    },
    Algorithm {
        modulators: [0b0010, 0, 0b1000, 0],
        carriers: 0b0101,
        diagram: "2>1  4>3",
    },
    Algorithm {
        modulators: [0b1000, 0b1000, 0b1000, 0],
        carriers: 0b0111,
        diagram: "4>(1,2,3)",
    },
    Algorithm {
        modulators: [0, 0, 0b1000, 0],
        carriers: 0b0111,
        diagram: "1  2  4>3",
    },
    Algorithm {
        modulators: [0, 0, 0, 0],
        carriers: 0b1111,
        diagram: "1  2  3  4",
    },
];

/// Editable settings for one operator. All values are normalized or clamped
/// by [`OperatorParams::sanitized`] before the engine uses them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OperatorParams {
    /// Frequency ratio against the note (0.5 to 16).
    pub ratio: f32,
    /// Fine detune in cents (-50 to +50).
    pub detune: f32,
    /// Output level (0 to 1).
    pub level: f32,
    /// Velocity sensitivity (0 to 1).
    pub velocity: f32,
    /// Attack time, normalized (0 to 1, exponential 1 ms to 4 s).
    pub attack: f32,
    /// Decay time, normalized (0 to 1, exponential 5 ms to 8 s).
    pub decay: f32,
    /// Sustain level (0 to 1).
    pub sustain: f32,
    /// Release time, normalized (0 to 1, exponential 5 ms to 8 s).
    pub release: f32,
}

/// Editable per-operator fields, in UI order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatorField {
    Ratio,
    Detune,
    Level,
    Velocity,
    Attack,
    Decay,
    Sustain,
    Release,
}

impl OperatorField {
    pub const ALL: [Self; 8] = [
        Self::Ratio,
        Self::Detune,
        Self::Level,
        Self::Velocity,
        Self::Attack,
        Self::Decay,
        Self::Sustain,
        Self::Release,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ratio => "ratio",
            Self::Detune => "detune",
            Self::Level => "level",
            Self::Velocity => "vel",
            Self::Attack => "attack",
            Self::Decay => "decay",
            Self::Sustain => "sustain",
            Self::Release => "release",
        }
    }

    /// Size of one fine edit step for this field.
    #[must_use]
    pub const fn step(self) -> f32 {
        match self {
            Self::Ratio => 0.5,
            Self::Detune => 1.0,
            _ => 0.02,
        }
    }
}

impl OperatorParams {
    const fn new(
        ratio: f32,
        level: f32,
        attack: f32,
        decay: f32,
        sustain: f32,
        release: f32,
    ) -> Self {
        Self {
            ratio,
            detune: 0.0,
            level,
            velocity: 0.5,
            attack,
            decay,
            sustain,
            release,
        }
    }

    #[must_use]
    pub const fn get(&self, field: OperatorField) -> f32 {
        match field {
            OperatorField::Ratio => self.ratio,
            OperatorField::Detune => self.detune,
            OperatorField::Level => self.level,
            OperatorField::Velocity => self.velocity,
            OperatorField::Attack => self.attack,
            OperatorField::Decay => self.decay,
            OperatorField::Sustain => self.sustain,
            OperatorField::Release => self.release,
        }
    }

    /// Set a field. Non-finite input is ignored; finite input is clamped.
    pub fn set(&mut self, field: OperatorField, value: f32) {
        if !value.is_finite() {
            return;
        }
        match field {
            OperatorField::Ratio => self.ratio = value,
            OperatorField::Detune => self.detune = value,
            OperatorField::Level => self.level = value,
            OperatorField::Velocity => self.velocity = value,
            OperatorField::Attack => self.attack = value,
            OperatorField::Decay => self.decay = value,
            OperatorField::Sustain => self.sustain = value,
            OperatorField::Release => self.release = value,
        }
        *self = self.sanitized();
    }

    /// Clamp every field into range, replacing non-finite values with defaults.
    #[must_use]
    pub fn sanitized(self) -> Self {
        let fix = |v: f32, default: f32, lo: f32, hi: f32| {
            if v.is_finite() {
                v.clamp(lo, hi)
            } else {
                default
            }
        };
        Self {
            ratio: fix(self.ratio, 1.0, 0.5, 16.0),
            detune: fix(self.detune, 0.0, -50.0, 50.0),
            level: fix(self.level, 0.0, 0.0, 1.0),
            velocity: fix(self.velocity, 0.5, 0.0, 1.0),
            attack: fix(self.attack, 0.0, 0.0, 1.0),
            decay: fix(self.decay, 0.5, 0.0, 1.0),
            sustain: fix(self.sustain, 1.0, 0.0, 1.0),
            release: fix(self.release, 0.3, 0.0, 1.0),
        }
    }

    /// Attack time in seconds.
    #[must_use]
    pub fn attack_seconds(&self) -> f32 {
        exp_map(self.attack, 0.001, 4.0)
    }

    /// Decay time in seconds.
    #[must_use]
    pub fn decay_seconds(&self) -> f32 {
        exp_map(self.decay, 0.005, 8.0)
    }

    /// Release time in seconds.
    #[must_use]
    pub fn release_seconds(&self) -> f32 {
        exp_map(self.release, 0.005, 8.0)
    }
}

/// A complete sound: routing, feedback and four operators.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Patch {
    pub name: &'static str,
    /// Index into [`ALGORITHMS`].
    pub algorithm: usize,
    /// Operator 4 self-feedback (0 to 1).
    pub feedback: f32,
    /// LFO speed, normalized (0 to 1, exponential 0.1 Hz to 12 Hz).
    pub lfo_rate: f32,
    /// Vibrato depth (0 to 1, up to a quarter tone either way).
    pub vibrato: f32,
    pub operators: [OperatorParams; OPERATORS],
}

impl Patch {
    /// Clamp all values into range so the engine can trust them.
    #[must_use]
    pub fn sanitized(self) -> Self {
        Self {
            name: self.name,
            algorithm: self.algorithm.min(ALGORITHMS.len() - 1),
            feedback: if self.feedback.is_finite() {
                self.feedback.clamp(0.0, 1.0)
            } else {
                0.0
            },
            lfo_rate: unit_or(self.lfo_rate, 0.5),
            vibrato: unit_or(self.vibrato, 0.0),
            operators: self.operators.map(OperatorParams::sanitized),
        }
    }
}

/// Factory patches. Each one exercises a different algorithm.
pub const PATCHES: [Patch; 6] = [
    Patch {
        name: "Tine Piano",
        algorithm: 4,
        feedback: 0.15,
        lfo_rate: 0.45,
        vibrato: 0.0,
        operators: [
            OperatorParams::new(1.0, 0.95, 0.0, 0.62, 0.0, 0.42),
            OperatorParams::new(1.0, 0.55, 0.0, 0.48, 0.0, 0.35),
            OperatorParams::new(1.0, 0.8, 0.0, 0.6, 0.0, 0.4),
            OperatorParams::new(14.0, 0.42, 0.0, 0.3, 0.0, 0.2),
        ],
    },
    Patch {
        name: "Solid Bass",
        algorithm: 0,
        feedback: 0.55,
        lfo_rate: 0.4,
        vibrato: 0.0,
        operators: [
            OperatorParams::new(0.5, 1.0, 0.0, 0.55, 0.55, 0.12),
            OperatorParams::new(1.0, 0.6, 0.0, 0.4, 0.3, 0.12),
            OperatorParams::new(1.0, 0.45, 0.0, 0.3, 0.2, 0.12),
            OperatorParams::new(3.0, 0.3, 0.0, 0.25, 0.0, 0.1),
        ],
    },
    Patch {
        name: "Glass Bell",
        algorithm: 4,
        feedback: 0.0,
        lfo_rate: 0.35,
        vibrato: 0.04,
        operators: [
            OperatorParams::new(1.0, 0.9, 0.0, 0.78, 0.0, 0.7),
            OperatorParams::new(3.5, 0.62, 0.0, 0.7, 0.0, 0.6),
            OperatorParams::new(2.0, 0.6, 0.0, 0.74, 0.0, 0.65),
            OperatorParams::new(7.0, 0.5, 0.0, 0.55, 0.0, 0.5),
        ],
    },
    Patch {
        name: "Brass Section",
        algorithm: 1,
        feedback: 0.4,
        lfo_rate: 0.42,
        vibrato: 0.12,
        operators: [
            OperatorParams::new(1.0, 0.95, 0.35, 0.5, 0.8, 0.3),
            OperatorParams::new(1.0, 0.7, 0.42, 0.5, 0.65, 0.3),
            OperatorParams::new(1.0, 0.35, 0.3, 0.4, 0.5, 0.3),
            OperatorParams::new(2.0, 0.25, 0.3, 0.4, 0.5, 0.3),
        ],
    },
    Patch {
        name: "Drawbar Organ",
        algorithm: 7,
        feedback: 0.2,
        lfo_rate: 0.6,
        vibrato: 0.06,
        operators: [
            OperatorParams::new(0.5, 0.8, 0.02, 0.5, 1.0, 0.1),
            OperatorParams::new(1.0, 0.85, 0.02, 0.5, 1.0, 0.1),
            OperatorParams::new(2.0, 0.65, 0.02, 0.5, 1.0, 0.1),
            OperatorParams::new(3.0, 0.55, 0.02, 0.5, 1.0, 0.1),
        ],
    },
    Patch {
        name: "Terminal Lead",
        algorithm: 3,
        feedback: 0.7,
        lfo_rate: 0.55,
        vibrato: 0.22,
        operators: [
            OperatorParams::new(1.0, 0.9, 0.05, 0.5, 0.75, 0.25),
            OperatorParams::new(2.0, 0.5, 0.05, 0.45, 0.4, 0.25),
            OperatorParams::new(1.0, 0.55, 0.05, 0.5, 0.6, 0.25),
            OperatorParams::new(1.0, 0.45, 0.05, 0.5, 0.6, 0.25),
        ],
    },
];

/// Who started a note. A looping phrase only ever releases its own voices, so
/// it never cuts off a note someone is holding on the keyboard or the hub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Computer keyboard or hub note events.
    Player,
    /// The looping `--phrase`.
    Phrase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Release,
}

/// Per-operator values derived from a patch and the sample rate.
#[derive(Debug, Clone, Copy, Default)]
struct OperatorCoeffs {
    attack_inc: f32,
    decay_coeff: f32,
    release_coeff: f32,
    sustain: f32,
    amplitude: f32,
    velocity: f32,
    freq_mult: f32,
}

impl OperatorCoeffs {
    fn from_params(p: &OperatorParams, sample_rate: f32) -> Self {
        // Time constants chosen so the segment is about -43 dB after its time.
        let segment = |seconds: f32| (-5.0 / (seconds * sample_rate)).exp();
        Self {
            attack_inc: 1.0 / (p.attack_seconds() * sample_rate).max(1.0),
            decay_coeff: segment(p.decay_seconds()),
            release_coeff: segment(p.release_seconds()),
            sustain: p.sustain,
            amplitude: p.level * p.level,
            velocity: p.velocity,
            freq_mult: p.ratio * (p.detune / 1200.0).exp2(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Envelope {
    stage: Stage,
    level: f32,
}

impl Envelope {
    const IDLE: Self = Self {
        stage: Stage::Idle,
        level: 0.0,
    };

    fn next(&mut self, c: &OperatorCoeffs) -> f32 {
        match self.stage {
            Stage::Idle => self.level = 0.0,
            Stage::Attack => {
                self.level += c.attack_inc;
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                self.level = (self.level - c.sustain).mul_add(c.decay_coeff, c.sustain);
                // Snap once inaudibly close, so a zero sustain lands on exactly
                // zero instead of creeping into denormal floats.
                if (self.level - c.sustain).abs() < ENV_FLOOR {
                    self.level = c.sustain;
                }
            }
            Stage::Release => {
                self.level *= c.release_coeff;
                if self.level < ENV_FLOOR {
                    self.level = 0.0;
                    self.stage = Stage::Idle;
                }
            }
        }
        self.level
    }
}

#[derive(Debug, Clone, Copy)]
struct Voice {
    note: u8,
    source: Source,
    gate: bool,
    velocity: f32,
    note_hz: f32,
    /// Monotonic start stamp used for oldest-voice stealing.
    started: u64,
    phase: [f32; OPERATORS],
    envelope: [Envelope; OPERATORS],
    feedback_history: [f32; 2],
}

impl Voice {
    const SILENT: Self = Self {
        note: 0,
        source: Source::Player,
        gate: false,
        velocity: 0.0,
        note_hz: 0.0,
        started: 0,
        phase: [0.0; OPERATORS],
        envelope: [Envelope::IDLE; OPERATORS],
        feedback_history: [0.0; 2],
    };

    /// A voice is alive while any operator envelope is running. Tracking all
    /// operators, not just current carriers, means changing algorithm while a
    /// note rings never frees that voice mid-sound.
    fn is_sounding(&self) -> bool {
        self.envelope.iter().any(|env| env.stage != Stage::Idle)
    }
}

/// Eight-voice, four-operator FM synth.
#[derive(Debug)]
pub struct FmSynth {
    sample_rate: f32,
    patch: Patch,
    coeffs: [OperatorCoeffs; OPERATORS],
    voices: [Voice; VOICES],
    clock: u64,
    master: f32,
    lfo_phase: f32,
    scope: [f32; SCOPE_LEN],
    scope_pos: usize,
}

impl FmSynth {
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
        let mut synth = Self {
            sample_rate,
            patch: PATCHES[0],
            coeffs: [OperatorCoeffs::default(); OPERATORS],
            voices: [Voice::SILENT; VOICES],
            clock: 0,
            master: 0.8,
            lfo_phase: 0.0,
            scope: [0.0; SCOPE_LEN],
            scope_pos: 0,
        };
        synth.set_patch(PATCHES[0]);
        synth
    }

    #[cfg(test)]
    #[must_use]
    pub const fn patch(&self) -> &Patch {
        &self.patch
    }

    /// Replace the sound. Sounding voices keep playing with the new values.
    pub fn set_patch(&mut self, patch: Patch) {
        self.patch = patch.sanitized();
        for (coeff, params) in self.coeffs.iter_mut().zip(self.patch.operators.iter()) {
            *coeff = OperatorCoeffs::from_params(params, self.sample_rate);
        }
    }

    /// Master output volume (0 to 1). Non-finite input is ignored.
    pub const fn set_master(&mut self, value: f32) {
        if value.is_finite() {
            self.master = value.clamp(0.0, 1.0);
        }
    }

    /// Start a note. Velocity is 0 to 127; zero velocity is treated as note-off.
    pub fn note_on(&mut self, note: u8, velocity: u8) {
        self.note_on_from(Source::Player, note, velocity);
    }

    /// Start a note on behalf of a source.
    pub fn note_on_from(&mut self, source: Source, note: u8, velocity: u8) {
        if velocity == 0 {
            self.note_off_from(source, note);
            return;
        }
        let note = note.min(127);
        let slot = self.pick_voice(source, note);
        self.clock = self.clock.wrapping_add(1);
        let voice = &mut self.voices[slot];
        voice.note = note;
        voice.source = source;
        voice.gate = true;
        voice.velocity = f32::from(velocity.min(127)) / 127.0;
        voice.note_hz = midi_to_hz(note);
        voice.started = self.clock;
        // Envelopes restart from their current level so a stolen voice does
        // not click down to zero first.
        for env in &mut voice.envelope {
            env.stage = Stage::Attack;
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
            for env in &mut voice.envelope {
                if env.stage != Stage::Idle {
                    env.stage = Stage::Release;
                }
            }
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
            for env in &mut voice.envelope {
                if env.stage != Stage::Idle {
                    env.stage = Stage::Release;
                }
            }
        }
    }

    /// Release every voice.
    pub fn all_notes_off(&mut self) {
        for voice in &mut self.voices {
            voice.gate = false;
            for env in &mut voice.envelope {
                if env.stage != Stage::Idle {
                    env.stage = Stage::Release;
                }
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
    pub fn voice_states(&self) -> [Option<(u8, bool)>; VOICES] {
        self.voices
            .map(|v| v.is_sounding().then_some((v.note, v.gate)))
    }

    /// Most recent output samples, oldest first starting at `scope_pos`.
    #[must_use]
    pub const fn scope(&self) -> (&[f32; SCOPE_LEN], usize) {
        (&self.scope, self.scope_pos)
    }

    /// Render one mono sample, limited and NaN-safe.
    pub fn process(&mut self) -> f32 {
        let algorithm = ALGORITHMS[self.patch.algorithm];
        let feedback = self.patch.feedback * FEEDBACK_DEPTH_CYCLES;
        let inv_sr = 1.0 / self.sample_rate;

        // One shared LFO, as on the DX7: a sine that bends every voice's pitch.
        let lfo_hz = exp_map(self.patch.lfo_rate, 0.1, 12.0);
        self.lfo_phase = lfo_hz.mul_add(inv_sr, self.lfo_phase).fract();
        let vibrato_cents = self.patch.vibrato * MAX_VIBRATO_CENTS * (TAU * self.lfo_phase).sin();
        let pitch_mult = (vibrato_cents / 1200.0).exp2();
        let step_scale = pitch_mult * inv_sr;

        // Carriers are summed at equal power, so additive algorithms (organ) are
        // not quieter than single-carrier stacks.
        let carrier_norm = 1.0 / (algorithm.carriers.count_ones().max(1) as f32).sqrt();
        let mut mix = 0.0;

        for voice in &mut self.voices {
            if !voice.is_sounding() {
                continue;
            }
            let mut out = [0.0_f32; OPERATORS];
            for op in (0..OPERATORS).rev() {
                let c = &self.coeffs[op];
                let env = voice.envelope[op].next(c);
                let vel_gain = c.velocity.mul_add(voice.velocity - 1.0, 1.0);
                let amp = c.amplitude * env * vel_gain;

                let mut modulation = 0.0;
                for (src, &value) in out.iter().enumerate() {
                    if algorithm.modulators[op] & (1 << src) != 0 {
                        modulation += value;
                    }
                }
                let mut offset = modulation * MOD_DEPTH_CYCLES;
                if op == OPERATORS - 1 {
                    let [a, b] = voice.feedback_history;
                    offset = ((a + b) * 0.5).mul_add(feedback, offset);
                }

                let sample = (TAU * (voice.phase[op] + offset)).sin() * amp;
                out[op] = sample;
                if op == OPERATORS - 1 {
                    voice.feedback_history = [voice.feedback_history[1], sample];
                }

                voice.phase[op] = voice
                    .note_hz
                    .mul_add(c.freq_mult * step_scale, voice.phase[op])
                    .fract();
            }

            let carriers: f32 = (0..OPERATORS)
                .filter(|op| algorithm.carriers & (1 << op) != 0)
                .map(|op| out[op])
                .sum();
            mix = carriers.mul_add(carrier_norm, mix);

            if !voice
                .phase
                .iter()
                .chain(voice.feedback_history.iter())
                .all(|v| v.is_finite())
            {
                *voice = Voice::SILENT;
            }
        }

        let sample =
            kazoo_core::sanitize_sample(kazoo_core::soft_limit(mix * VOICE_MIX_GAIN * self.master));
        self.scope[self.scope_pos] = sample;
        self.scope_pos = (self.scope_pos + 1) % SCOPE_LEN;
        sample
    }
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

const fn unit_or(value: f32, default: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        default
    }
}

fn exp_map(value: f32, min: f32, max: f32) -> f32 {
    min * (max / min).powf(value.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn energy(synth: &mut FmSynth, samples: usize) -> f32 {
        (0..samples).map(|_| synth.process().abs()).sum()
    }

    #[test]
    fn modulators_always_have_higher_index() {
        for (n, alg) in ALGORITHMS.iter().enumerate() {
            for (op, &mask) in alg.modulators.iter().enumerate() {
                let lower_or_self = (1u8 << (op + 1)) - 1;
                assert_eq!(mask & lower_or_self, 0, "algorithm {} op {}", n + 1, op + 1);
            }
            assert_ne!(alg.carriers, 0, "algorithm {} has no carrier", n + 1);
        }
    }

    #[test]
    fn silent_without_notes() {
        let mut synth = FmSynth::new(SR);
        assert!(energy(&mut synth, 4_096) < 1.0e-6);
    }

    #[test]
    fn every_patch_makes_finite_bounded_sound() {
        for patch in PATCHES {
            let mut synth = FmSynth::new(SR);
            synth.set_patch(patch);
            synth.note_on(60, 110);
            synth.note_on(64, 90);
            let mut total = 0.0;
            for _ in 0..8_192 {
                let s = synth.process();
                assert!(
                    s.is_finite() && s.abs() <= 1.0,
                    "{} produced {s}",
                    patch.name
                );
                total += s.abs();
            }
            assert!(total > 1.0, "{} is silent", patch.name);
        }
    }

    #[test]
    fn every_algorithm_sounds() {
        for alg in 0..ALGORITHMS.len() {
            let mut synth = FmSynth::new(SR);
            let mut patch = PATCHES[4];
            patch.algorithm = alg;
            synth.set_patch(patch);
            synth.note_on(57, 100);
            assert!(
                energy(&mut synth, 4_096) > 1.0,
                "algorithm {} is silent",
                alg + 1
            );
        }
    }

    #[test]
    fn note_off_decays_to_silence_and_frees_voice() {
        let mut synth = FmSynth::new(SR);
        synth.set_patch(PATCHES[4]);
        synth.note_on(60, 100);
        energy(&mut synth, 2_000);
        synth.note_off(60);
        energy(&mut synth, SR as usize * 2);
        assert!(energy(&mut synth, 1_000) < 1.0e-3);
        assert!(synth.voice_states().iter().all(Option::is_none));
    }

    #[test]
    fn steals_when_more_than_eight_notes() {
        let mut synth = FmSynth::new(SR);
        synth.set_patch(PATCHES[4]);
        for note in 48..60 {
            synth.note_on(note, 100);
            synth.process();
        }
        let states = synth.voice_states();
        assert_eq!(states.iter().flatten().count(), VOICES);
        // The four oldest notes were stolen; the newest are all present.
        for note in 52..60 {
            assert!(
                states.iter().flatten().any(|&(n, _)| n == note),
                "missing {note}"
            );
        }
    }

    #[test]
    fn repeated_note_reuses_its_voice() {
        let mut synth = FmSynth::new(SR);
        synth.note_on(60, 100);
        synth.note_on(60, 100);
        assert_eq!(synth.voice_states().iter().flatten().count(), 1);
    }

    #[test]
    fn zero_velocity_is_note_off() {
        let mut synth = FmSynth::new(SR);
        synth.note_on(60, 100);
        synth.note_on(60, 0);
        assert!(
            synth
                .voice_states()
                .iter()
                .flatten()
                .all(|&(_, gate)| !gate)
        );
    }

    #[test]
    fn hostile_values_are_sanitized() {
        let mut patch = PATCHES[0];
        patch.algorithm = 99;
        patch.feedback = f32::NAN;
        patch.operators[0].ratio = f32::INFINITY;
        patch.operators[1].level = -3.0;
        patch.operators[2].attack = f32::NAN;
        let mut synth = FmSynth::new(f32::NAN);
        synth.set_patch(patch);
        synth.set_master(f32::NAN);
        synth.note_on(200, 255);
        for _ in 0..4_096 {
            assert!(synth.process().is_finite());
        }
        assert_eq!(synth.patch().algorithm, ALGORITHMS.len() - 1);
    }

    #[test]
    fn max_feedback_stays_stable() {
        let mut synth = FmSynth::new(SR);
        let mut patch = PATCHES[1];
        patch.feedback = 1.0;
        synth.set_patch(patch);
        synth.note_on(40, 127);
        for _ in 0..SR as usize {
            let s = synth.process();
            assert!(s.is_finite() && s.abs() <= 1.0);
        }
    }

    #[test]
    fn operator_field_set_clamps() {
        let mut op = PATCHES[0].operators[0];
        op.set(OperatorField::Ratio, 100.0);
        assert!((op.ratio - 16.0).abs() < f32::EPSILON);
        op.set(OperatorField::Level, f32::NAN);
        assert!(op.level.is_finite());
    }

    #[test]
    fn levels_have_headroom() {
        for patch in PATCHES {
            for (notes, lo, hi) in [(&[60_u8][..], 0.15, 0.6), (&[48, 55, 60, 64][..], 0.4, 1.0)] {
                let mut synth = FmSynth::new(SR);
                synth.set_patch(patch);
                for &n in notes {
                    synth.note_on(n, 100);
                }
                let peak = (0..24_000).fold(0.0_f32, |m, _| m.max(synth.process().abs()));
                assert!(
                    (lo..=hi).contains(&peak),
                    "{} with {} notes peaks at {peak}",
                    patch.name,
                    notes.len()
                );
            }
        }
    }

    #[test]
    fn vibrato_moves_pitch() {
        // Count zero crossings over one second with and without vibrato: the
        // average pitch is unchanged, so compare the spread across windows.
        let crossings = |vibrato: f32| {
            let mut synth = FmSynth::new(SR);
            let mut patch = PATCHES[4];
            patch.algorithm = 7;
            patch.vibrato = vibrato;
            patch.lfo_rate = 0.6;
            for op in &mut patch.operators[1..] {
                op.level = 0.0;
            }
            synth.set_patch(patch);
            synth.note_on(69, 100);
            let mut windows = Vec::new();
            let mut last = 0.0_f32;
            for _ in 0..8 {
                let mut count = 0;
                for _ in 0..6_000 {
                    let s = synth.process();
                    if last <= 0.0 && s > 0.0 {
                        count += 1;
                    }
                    last = s;
                }
                windows.push(count);
            }
            windows.iter().max().unwrap() - windows.iter().min().unwrap()
        };
        assert!(crossings(1.0) > crossings(0.0));
    }

    #[test]
    fn algorithm_change_keeps_ringing_voice() {
        let mut synth = FmSynth::new(SR);
        let mut patch = PATCHES[4];
        patch.algorithm = 7;
        synth.set_patch(patch);
        synth.note_on(60, 100);
        energy(&mut synth, 1_000);
        // Operator 1 goes quiet while 2 to 4 ring, then operator 1 becomes the only carrier.
        synth.voices[0].envelope[0] = Envelope::IDLE;
        patch.algorithm = 0;
        synth.set_patch(patch);
        synth.process();
        assert_eq!(synth.voice_states().iter().flatten().count(), 1);
    }

    #[test]
    fn zero_sustain_settles_to_exact_zero() {
        let mut env = Envelope {
            stage: Stage::Decay,
            level: 1.0,
        };
        let coeffs = OperatorCoeffs::from_params(&PATCHES[0].operators[3], SR);
        for _ in 0..SR as usize * 2 {
            env.next(&coeffs);
        }
        assert!(env.level == 0.0 || env.level.is_normal());
        assert!(env.level.abs() < f32::EPSILON);
    }

    #[test]
    fn phrase_release_leaves_player_notes_alone() {
        let mut synth = FmSynth::new(SR);
        synth.note_on(60, 100);
        synth.note_on_from(Source::Phrase, 60, 100);
        synth.release_source(Source::Phrase);
        synth.note_off_from(Source::Phrase, 60);
        let held: Vec<_> = synth
            .voice_states()
            .iter()
            .flatten()
            .filter(|v| v.1)
            .copied()
            .collect();
        assert_eq!(held, vec![(60, true)]);
    }

    #[test]
    fn names_and_pitch() {
        assert!((midi_to_hz(69) - 440.0).abs() < 1.0e-3);
        assert_eq!(note_name(60), "C4");
        assert_eq!(note_name(61), "C#4");
    }
}
