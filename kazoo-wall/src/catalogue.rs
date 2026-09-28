//! The module catalogue: every kind of module the wall holds, with its knobs,
//! its ports and how to build one.
//!
//! The catalogue is a registry, built once: the wall's own synth modules
//! first, then one kind for every effect in [`kazoo_fx::catalogue`] (its id
//! is the kind's name). A [`Kind`] is a small handle into it; every kind
//! carries a [`Builder`], so adding a family of modules (percussion, a
//! sampler, speech) is adding its kinds to the registry, not a match arm in
//! every file.
//!
//! Knob and port order is fixed: the DSP addresses knobs and ports by their
//! index in these tables, and the engine carries knob values in the same
//! order. Values are always clamped to a knob's range; the ranges marked as
//! safety caps in the design (VCF resonance, out level, and every effect's
//! own caps) are the only enforced limits on taste.
//!
//! Every knob is also a jack: a CV input with the knob's name. A cable into
//! it adds `cv × (max − min) / 2 × amount` to the knob's gliding value, held
//! to the knob's range, so modulation never crosses a safety cap (the
//! filter's cutoff jack is the one exception: it moves the cutoff five
//! octaves per 1.0, as on the hardware). Knob names and input names share
//! one namespace per kind, so `vcf1.cutoff` always means one thing.

use std::fmt;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::dsp::Module;
use crate::{MAX_INPUTS, MAX_KNOBS, MAX_OUTPUTS};

/// Middle C (C4), the pitch 0.0 stands for: 1.0 per octave above or below.
pub const C4_HZ: f32 = 261.625_58;

/// A kind of module: a handle into the [`registry`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Kind(u16);

impl Kind {
    /// Voltage-controlled oscillator.
    pub const VCO: Self = Self(0);
    /// Low-frequency oscillator.
    pub const LFO: Self = Self(1);
    /// Noise source.
    pub const NOISE: Self = Self(2);
    /// Voltage-controlled filter.
    pub const VCF: Self = Self(3);
    /// Voltage-controlled amplifier.
    pub const VCA: Self = Self(4);
    /// ADSR envelope.
    pub const ENV: Self = Self(5);
    /// Clock on the wall's beat.
    pub const CLOCK: Self = Self(6);
    /// Step sequencer.
    pub const SEQ: Self = Self(7);
    /// Sample and hold.
    pub const SH: Self = Self(8);
    /// Pitch quantiser.
    pub const QUANT: Self = Self(9);
    /// Slew limiter.
    pub const SLEW: Self = Self(10);
    /// Four-input mixer.
    pub const MIX: Self = Self(11);
    /// Output to the master bus.
    pub const OUT: Self = Self(12);

    /// The kind's catalogue entry.
    #[must_use]
    pub fn spec(self) -> &'static KindSpec {
        &registry().kinds[usize::from(self.0)]
    }

    /// The kind's name on the wire and in module ids (`vco`, `plate`, ...).
    #[must_use]
    pub fn name(self) -> &'static str {
        self.spec().name
    }

    /// The kind with this name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        registry()
            .kinds
            .iter()
            .find(|spec| spec.name == name)
            .map(|spec| spec.kind)
    }

    /// Every kind, in catalogue order: the wall's own, then the effects.
    pub fn all() -> impl Iterator<Item = Self> {
        registry().kinds.iter().map(|spec| spec.kind)
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl fmt::Debug for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Kind({})", self.name())
    }
}

/// What a knob's number means, for display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    /// A plain number.
    None,
    /// Frequency: shown in Hz or kHz.
    Hz,
    /// Time: shown in ms or s.
    Seconds,
    /// Semitones.
    Semitones,
    /// Whole octaves.
    Octaves,
    /// A count of steps.
    Steps,
    /// Any other unit, shown after the number (`dB`, `%`).
    Other(&'static str),
}

impl Unit {
    /// The unit's name on the wire.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Hz => "Hz",
            Self::Seconds => "s",
            Self::Semitones => "st",
            Self::Octaves => "oct",
            Self::Steps => "steps",
            Self::Other(unit) => unit,
        }
    }

    /// The unit for an effect parameter's unit text.
    #[must_use]
    pub const fn from_text(text: &'static str) -> Self {
        match text.as_bytes() {
            b"" => Self::None,
            b"Hz" => Self::Hz,
            b"s" => Self::Seconds,
            b"st" => Self::Semitones,
            _ => Self::Other(text),
        }
    }
}

/// How a knob's travel maps to its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Curve {
    /// Even steps across the range.
    Linear,
    /// Even ratios across the range: every part of the travel is the same
    /// musical distance. A range starting at zero is logarithmic from a
    /// thousandth of its top, with zero at the very bottom.
    Log,
}

/// How a cable into a knob's jack moves the knob.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JackLaw {
    /// `cv × (max − min) / 2` is added to the knob.
    Range,
    /// The knob is multiplied by `2^(cv × octaves)`.
    Octaves(f32),
}

/// One knob of a module kind.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KnobSpec {
    /// Name, unique within the kind's knobs and inputs.
    pub name: &'static str,
    /// Lowest value.
    pub min: f32,
    /// Highest value.
    pub max: f32,
    /// Value on a new module.
    pub default: f32,
    /// What the number means.
    pub unit: Unit,
    /// How the travel maps to the value.
    pub curve: Curve,
    /// Whole numbers only.
    pub stepped: bool,
    /// How its jack moves it.
    pub jack: JackLaw,
    /// Names for the whole-number positions from `min` up: every position
    /// of a stepped knob (divisions, scales), or the named points of a
    /// morph (sine, triangle, saw...). Empty for plain numbers.
    pub labels: &'static [&'static str],
}

impl KnobSpec {
    const fn linear(name: &'static str, min: f32, max: f32, default: f32, unit: Unit) -> Self {
        Self {
            name,
            min,
            max,
            default,
            unit,
            curve: Curve::Linear,
            stepped: false,
            jack: JackLaw::Range,
            labels: &[],
        }
    }

    const fn log(name: &'static str, min: f32, max: f32, default: f32, unit: Unit) -> Self {
        Self {
            curve: Curve::Log,
            ..Self::linear(name, min, max, default, unit)
        }
    }

    const fn stepped(name: &'static str, min: f32, max: f32, default: f32, unit: Unit) -> Self {
        Self {
            stepped: true,
            ..Self::linear(name, min, max, default, unit)
        }
    }

    /// The same knob with names for its positions.
    const fn named(self, labels: &'static [&'static str]) -> Self {
        Self { labels, ..self }
    }

    /// The same knob with its jack moving it `octaves` per 1.0.
    const fn octave_jack(self, octaves: f32) -> Self {
        Self {
            jack: JackLaw::Octaves(octaves),
            ..self
        }
    }

    /// `value` held to the knob's range (and to whole steps on a stepped
    /// knob). A value that is not a number gives the default.
    #[must_use]
    pub fn clamp(&self, value: f32) -> f32 {
        if !value.is_finite() {
            return self.default;
        }
        let value = value.clamp(self.min, self.max);
        if self.stepped { value.round() } else { value }
    }

    /// Where `value` sits along the knob's travel, 0 to 1.
    #[must_use]
    pub fn normalise(&self, value: f32) -> f32 {
        let value = self.clamp(value);
        let span = self.max - self.min;
        if span <= 0.0 {
            return 0.0;
        }
        match self.curve {
            Curve::Linear => (value - self.min) / span,
            Curve::Log => {
                let floor = self.log_floor();
                if value <= floor {
                    // Only a zero-based range has values below the floor.
                    return 0.0;
                }
                (value / floor).log(self.max / floor).clamp(0.0, 1.0)
            }
        }
    }

    /// The value at `position` (0 to 1) along the knob's travel.
    #[must_use]
    pub fn denormalise(&self, position: f32) -> f32 {
        let position = if position.is_finite() {
            position.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let value = match self.curve {
            Curve::Linear => (self.max - self.min).mul_add(position, self.min),
            Curve::Log => {
                if position <= 0.0 {
                    self.min
                } else {
                    let floor = self.log_floor();
                    floor * (self.max / floor).powf(position)
                }
            }
        };
        self.clamp(value)
    }

    /// The knob moved by a control voltage arriving at its jack (already
    /// scaled by the cable's amount), held to the knob's range.
    #[must_use]
    pub fn modulate(&self, knob: f32, cv: f32) -> f32 {
        if cv == 0.0 || !cv.is_finite() {
            return self.clamp(knob);
        }
        let moved = match self.jack {
            JackLaw::Range => ((self.max - self.min) * 0.5).mul_add(cv, self.clamp(knob)),
            JackLaw::Octaves(octaves) => self.clamp(knob) * (cv * octaves).exp2(),
        };
        self.clamp(moved)
    }

    /// Bottom of the logarithmic part of the range.
    fn log_floor(&self) -> f32 {
        if self.min > 0.0 {
            self.min
        } else {
            self.max * 1.0e-3
        }
    }
}

/// What a port carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Signal {
    /// Sound, nominally ±1.
    Audio,
    /// Gates and triggers: high above 0.5.
    Gate,
    /// Control voltage, nominally ±1; pitch is 1.0 per octave.
    Cv,
}

/// The groups module kinds are sorted into, in order: the console's
/// picker lists kinds by them, and a rack's first rows are laid out by
/// them.
pub const GROUPS: [&str; 5] = [
    "sound sources",
    "modulation",
    "filters & dynamics",
    "effects",
    "utilities",
];

/// The group (an index into [`GROUPS`]) of kind `kind` of `family`.
///
/// Effects go by family; the wall's own modules by what they are; anything
/// else by what its ports carry (whether any input and any output carries
/// audio, and whether it has outputs at all).
#[must_use]
pub fn group(family: &str, kind: &str, audio_in: bool, audio_out: bool, outputs: bool) -> usize {
    if family == "fx" {
        return 3;
    }
    if family == "synth" {
        match kind {
            "vco" | "noise" => return 0,
            "lfo" | "env" | "clock" | "seq" | "sh" | "slew" | "quant" => return 1,
            "vcf" | "vca" => return 2,
            "delay" | "reverb" => return 3,
            "mix" | "out" => return 4,
            _ => {}
        }
    }
    match (audio_in, audio_out, outputs) {
        (true, true, _) => 3,
        (false, true, _) => 0,
        (_, false, true) => 1,
        (_, false, false) => 4,
    }
}

/// One port of a module kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortSpec {
    /// Name, unique within the kind's inputs, outputs and knobs.
    pub name: &'static str,
    /// What it carries.
    pub signal: Signal,
    /// What it is for, in a few words.
    pub about: &'static str,
}

/// An input a cable can plug into: a listed input or a knob's jack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Jack {
    /// The kind's input at this index.
    Input(usize),
    /// The jack of the kind's knob at this index.
    Knob(usize),
}

/// How to build a module of a kind. Building happens on the control side
/// and may allocate; the result must then never allocate while it runs.
#[derive(Debug, Clone, Copy)]
pub enum Builder {
    /// One of the wall's own modules, for a stream at the given rate.
    Native(fn(f32) -> Box<dyn Module>),
    /// A `kazoo-fx` effect, adapted (see [`crate::dsp::effect`]).
    Effect(&'static kazoo_fx::EffectKind),
    /// Something from another of the studio's crates (see
    /// [`crate::adapters`]).
    Adapted(crate::adapters::Adapter),
}

/// A module kind offered to the registry, from its parameters: it becomes
/// a [`KindSpec`] if it fits the engine.
#[derive(Debug)]
pub struct Candidate {
    /// Its name (see [`valid_kind_name`]).
    pub name: &'static str,
    /// Its family: `fx`, `perc`, `rhythm`, `speech`.
    pub family: &'static str,
    /// What it is, in a few words.
    pub about: String,
    /// Its parameters, which become its knobs, in order.
    pub params: &'static [kazoo_fx::ParamSpec],
    /// Inputs other than the knob jacks.
    pub inputs: Vec<PortSpec>,
    /// Outputs.
    pub outputs: Vec<PortSpec>,
    /// How to build one.
    pub build: Builder,
}

/// A module kind's catalogue entry.
#[derive(Debug)]
pub struct KindSpec {
    /// The kind.
    pub kind: Kind,
    /// Its name (see [`valid_kind_name`]); a module id is the name and a
    /// number (see [`crate::patch::make_id`]).
    pub name: &'static str,
    /// The family it belongs to: `synth` for the wall's own, `fx` for
    /// effects, `perc` for drum voices, `rhythm` for rhythm generators,
    /// `speech` for the vocoder and speaker.
    pub family: &'static str,
    /// What it is, in a few words.
    pub about: String,
    /// Knobs, in engine order.
    pub knobs: Vec<KnobSpec>,
    /// Inputs other than the knob jacks, in engine order.
    pub inputs: Vec<PortSpec>,
    /// Outputs, in engine order.
    pub outputs: Vec<PortSpec>,
    /// How to build one.
    pub build: Builder,
}

impl KindSpec {
    /// The index of the knob called `name`.
    #[must_use]
    pub fn knob_index(&self, name: &str) -> Option<usize> {
        self.knobs.iter().position(|knob| knob.name == name)
    }

    /// The index of the input called `name`.
    #[must_use]
    pub fn input_index(&self, name: &str) -> Option<usize> {
        self.inputs.iter().position(|port| port.name == name)
    }

    /// The input or knob jack called `name`.
    #[must_use]
    pub fn jack(&self, name: &str) -> Option<Jack> {
        self.input_index(name)
            .map(Jack::Input)
            .or_else(|| self.knob_index(name).map(Jack::Knob))
    }

    /// Every name a cable can plug into: inputs, then knob jacks.
    #[must_use]
    pub fn jack_names(&self) -> Vec<&'static str> {
        self.inputs
            .iter()
            .map(|port| port.name)
            .chain(self.knobs.iter().map(|knob| knob.name))
            .collect()
    }

    /// The index of the output called `name`.
    #[must_use]
    pub fn output_index(&self, name: &str) -> Option<usize> {
        self.outputs.iter().position(|port| port.name == name)
    }

    /// Every knob at its default, in engine order.
    #[must_use]
    pub fn defaults(&self) -> Vec<f32> {
        self.knobs.iter().map(|knob| knob.default).collect()
    }

    /// Its group, an index into [`GROUPS`] (see [`group`]).
    #[must_use]
    pub fn group(&self) -> usize {
        let audio = |ports: &[PortSpec]| ports.iter().any(|port| port.signal == Signal::Audio);
        group(
            self.family,
            self.name,
            audio(&self.inputs),
            audio(&self.outputs),
            !self.outputs.is_empty(),
        )
    }
}

// ---------------------------------------------------------------------------
// The registry
// ---------------------------------------------------------------------------

/// Every module kind the wall knows.
#[derive(Debug)]
pub struct Registry {
    kinds: Vec<KindSpec>,
    skipped: Vec<String>,
}

impl Registry {
    /// The wall's own kinds, then one for each of `effects` that fits the
    /// engine; those that do not are left out, each with a sentence in
    /// [`Self::skipped`].
    #[must_use]
    pub fn build(effects: impl IntoIterator<Item = &'static kazoo_fx::EffectKind>) -> Self {
        Self::assemble(effects.into_iter().map(effect_candidate).collect())
    }

    /// The wall's own kinds, then each of `candidates` that fits the
    /// engine; those that do not are left out, each with a sentence in
    /// [`Self::skipped`].
    #[must_use]
    pub fn assemble(candidates: Vec<Candidate>) -> Self {
        let mut kinds = native_kinds();
        let mut skipped = Vec::new();
        for candidate in candidates {
            let (family, name) = (candidate.family, candidate.name);
            match admit(candidate, &kinds) {
                Ok(spec) => kinds.push(spec),
                Err(why) => skipped.push(format!("{family} '{name}' left out: {why}")),
            }
        }
        for (index, spec) in kinds.iter_mut().enumerate() {
            // At most a few hundred kinds: fits.
            spec.kind = Kind(index as u16);
        }
        Self { kinds, skipped }
    }

    /// Every kind, in catalogue order.
    #[must_use]
    pub fn kinds(&self) -> &[KindSpec] {
        &self.kinds
    }

    /// Effects left out, and why.
    #[must_use]
    pub fn skipped(&self) -> &[String] {
        &self.skipped
    }
}

/// The wall's registry of every module kind.
///
/// It holds the wall's own kinds, every `kazoo-fx` effect (and, in this
/// crate's tests, a test-only effect too), every `kazoo-perc` voice and
/// rhythm generator, and `kazoo-speech`'s vocoder and speaker.
#[must_use]
pub fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let effects = kazoo_fx::catalogue();
        #[cfg(test)]
        let effects = effects.chain(crate::dsp::effect::testing::KINDS.iter());
        let mut candidates: Vec<Candidate> = effects.map(effect_candidate).collect();
        candidates.extend(crate::adapters::candidates());
        let registry = Registry::assemble(candidates);
        for why in &registry.skipped {
            eprintln!("kazoo-wall: {why}");
        }
        registry
    })
}

/// Whether `name` can name a kind: `[a-z][a-z0-9_]*`, at most 24
/// characters, not ending in `_` (module ids add a number, with a `_`
/// between when the name ends in a digit).
#[must_use]
pub fn valid_kind_name(name: &str) -> bool {
    (1..=24).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && !name.ends_with('_')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// An effect as a candidate kind: stereo in and out.
fn effect_candidate(effect: &'static kazoo_fx::EffectKind) -> Candidate {
    Candidate {
        name: effect.id,
        family: "fx",
        about: format!("{}: {}", effect.name, effect.description),
        params: effect.params,
        inputs: vec![
            audio("left", "sound, left or mono"),
            audio("right", "sound, right; the left input when unplugged"),
        ],
        outputs: vec![audio("left", "sound, left"), audio("right", "sound, right")],
        build: Builder::Effect(effect),
    }
}

/// A candidate as a kind, if it fits: a name that makes good module ids
/// and is not taken, ports that fit the engine, and knobs that fit it
/// without clashing with each other or the inputs.
fn admit(candidate: Candidate, taken: &[KindSpec]) -> Result<KindSpec, String> {
    let name = candidate.name;
    if !valid_kind_name(name) {
        return Err(
            "its id must be a lower-case letter, then lower-case letters, digits and _, \
             not ending in _"
                .to_string(),
        );
    }
    if taken.iter().any(|spec| spec.name == name) {
        return Err("another module kind has that name".to_string());
    }
    if candidate.params.len() > MAX_KNOBS {
        return Err(format!(
            "{} parameters; a module holds {MAX_KNOBS}",
            candidate.params.len()
        ));
    }
    if candidate.inputs.len() > MAX_INPUTS || candidate.outputs.len() > MAX_OUTPUTS {
        return Err(format!(
            "{} inputs and {} outputs; a module holds {MAX_INPUTS} and {MAX_OUTPUTS}",
            candidate.inputs.len(),
            candidate.outputs.len()
        ));
    }
    for ports in [&candidate.inputs, &candidate.outputs] {
        for (index, port) in ports.iter().enumerate() {
            if ports[..index]
                .iter()
                .any(|earlier| earlier.name == port.name)
            {
                return Err(format!("two ports are named '{}'", port.name));
            }
        }
    }
    let mut knobs = Vec::with_capacity(candidate.params.len());
    for param in candidate.params {
        if candidate.inputs.iter().any(|port| port.name == param.name)
            || knobs.iter().any(|k: &KnobSpec| k.name == param.name)
        {
            return Err(format!("parameter name '{}' is taken", param.name));
        }
        if !(param.min.is_finite() && param.max.is_finite() && param.min < param.max) {
            return Err(format!("parameter '{}' has no range", param.name));
        }
        let unit = Unit::from_text(param.unit);
        let knob = match param.curve {
            kazoo_fx::Curve::Linear => {
                KnobSpec::linear(param.name, param.min, param.max, param.default, unit)
            }
            kazoo_fx::Curve::Log => {
                KnobSpec::log(param.name, param.min, param.max, param.default, unit)
            }
            kazoo_fx::Curve::Stepped { labels } => {
                KnobSpec::stepped(param.name, param.min, param.max, param.default, unit)
                    .named(labels)
            }
        };
        knobs.push(KnobSpec {
            default: knob.clamp(param.default),
            ..knob
        });
    }
    Ok(KindSpec {
        kind: Kind(0),
        name,
        family: candidate.family,
        about: candidate.about,
        knobs,
        inputs: candidate.inputs,
        outputs: candidate.outputs,
        build: candidate.build,
    })
}

// ---------------------------------------------------------------------------
// Tables the wall's own modules use
// ---------------------------------------------------------------------------

/// Clock divisions, in beats: 1/16, 1/8, 1/4, 1/2, 1 bar, 2 bars, 4 bars.
pub const CLOCK_DIVISIONS: [f64; 7] = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0];

/// The clock division knob's positions.
pub const CLOCK_LABELS: [&str; 7] = ["1/16", "1/8", "1/4", "1/2", "1 bar", "2 bars", "4 bars"];

/// LFO sync divisions, in beats; knob value 0 means free-running, value `n`
/// means entry `n - 1`.
pub const SYNC_DIVISIONS: [f64; 9] = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0];

/// The LFO sync knob's positions.
pub const SYNC_LABELS: [&str; 10] = [
    "free", "1/16", "1/8", "1/4", "1/2", "1 bar", "2 bars", "4 bars", "8 bars", "16 bars",
];

/// Quantiser scales: the semitones above the root each holds.
pub const SCALES: [&[u8]; 10] = [
    &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
    &[0, 2, 4, 5, 7, 9, 11],
    &[0, 2, 3, 5, 7, 8, 10],
    &[0, 2, 3, 5, 7, 9, 10],
    &[0, 1, 3, 5, 7, 8, 10],
    &[0, 2, 4, 6, 7, 9, 11],
    &[0, 2, 4, 5, 7, 9, 10],
    &[0, 2, 4, 7, 9],
    &[0, 3, 5, 7, 10],
    &[0, 2, 4, 6, 8, 10],
];

/// The scales' names.
pub const SCALE_NAMES: [&str; 10] = [
    "chromatic",
    "major",
    "minor",
    "dorian",
    "phrygian",
    "lydian",
    "mixolydian",
    "pentatonic major",
    "pentatonic minor",
    "whole tone",
];

/// Pitch-class names, 0 = C.
pub const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

const VCO_SHAPES: [&str; 4] = ["sine", "triangle", "saw", "square"];
const LFO_SHAPES: [&str; 6] = [
    "sine",
    "triangle",
    "saw",
    "square",
    "random steps",
    "smooth random",
];
const NOISE_COLOURS: [&str; 3] = ["white", "pink", "brown"];
const FILTER_MODES: [&str; 3] = ["low-pass", "band-pass", "high-pass"];

/// The beats a sync knob stands for: `None` when free-running (0).
#[must_use]
pub fn sync_beats(table: &[f64], knob: f32) -> Option<f64> {
    let index = knob.round();
    if !index.is_finite() || index < 1.0 {
        return None;
    }
    // Rounded, positive and finite: the cast is exact.
    table
        .get(index as usize - 1)
        .or_else(|| table.last())
        .copied()
}

const fn audio(name: &'static str, about: &'static str) -> PortSpec {
    PortSpec {
        name,
        signal: Signal::Audio,
        about,
    }
}

const fn gate(name: &'static str, about: &'static str) -> PortSpec {
    PortSpec {
        name,
        signal: Signal::Gate,
        about,
    }
}

const fn cv(name: &'static str, about: &'static str) -> PortSpec {
    PortSpec {
        name,
        signal: Signal::Cv,
        about,
    }
}

/// A native kind's entry (its `kind` is set when the registry is built).
fn native(
    name: &'static str,
    about: &str,
    knobs: Vec<KnobSpec>,
    inputs: Vec<PortSpec>,
    outputs: Vec<PortSpec>,
    build: fn(f32) -> Box<dyn Module>,
) -> KindSpec {
    KindSpec {
        kind: Kind(0),
        name,
        family: "synth",
        about: about.to_string(),
        knobs,
        inputs,
        outputs,
        build: Builder::Native(build),
    }
}

/// The wall's own kinds, in the order of the [`Kind`] constants.
fn native_kinds() -> Vec<KindSpec> {
    let mut kinds = sound_kinds();
    kinds.extend(shaping_kinds());
    kinds.extend(control_kinds());
    kinds
}

/// `vco`, `lfo`, `noise`, `vcf`.
fn sound_kinds() -> Vec<KindSpec> {
    use crate::dsp::native as dsp;
    vec![
        native(
            "vco",
            "oscillator: sine, triangle, saw and square, morphing",
            vec![
                KnobSpec::stepped("octave", -4.0, 4.0, 0.0, Unit::Octaves),
                KnobSpec::linear("tune", -12.0, 12.0, 0.0, Unit::Semitones),
                KnobSpec::linear("shape", 0.0, 3.0, 2.0, Unit::None).named(&VCO_SHAPES),
                KnobSpec::linear("width", 0.05, 0.95, 0.5, Unit::None),
                KnobSpec::linear("level", 0.0, 1.0, 0.8, Unit::None),
                KnobSpec::linear("fm_depth", 0.0, 1.0, 0.25, Unit::None),
            ],
            vec![
                cv("pitch", "pitch, 1.0 per octave above C4"),
                audio("fm", "linear frequency modulation, depth set by fm_depth"),
            ],
            vec![audio("out", "the oscillator")],
            dsp::vco,
        ),
        native(
            "lfo",
            "slow oscillator: sine, triangle, saw, square, random steps, smooth random",
            vec![
                KnobSpec::log("rate", 0.01, 30.0, 0.5, Unit::Hz),
                KnobSpec::linear("shape", 0.0, 5.0, 0.0, Unit::None).named(&LFO_SHAPES),
                KnobSpec::linear("depth", 0.0, 1.0, 1.0, Unit::None),
                KnobSpec::linear("offset", -1.0, 1.0, 0.0, Unit::None),
                KnobSpec::stepped("sync", 0.0, 9.0, 0.0, Unit::None).named(&SYNC_LABELS),
            ],
            vec![gate("reset", "restarts the cycle on a rising gate")],
            vec![cv("out", "offset plus depth times the wave")],
            dsp::lfo,
        ),
        native(
            "noise",
            "noise: white, pink or brown",
            vec![
                KnobSpec::linear("colour", 0.0, 2.0, 0.0, Unit::None).named(&NOISE_COLOURS),
                KnobSpec::linear("level", 0.0, 1.0, 0.5, Unit::None),
            ],
            Vec::new(),
            vec![audio("out", "the noise")],
            dsp::noise,
        ),
        native(
            "vcf",
            "filter: low-pass, band-pass and high-pass, morphing",
            vec![
                KnobSpec::log("cutoff", 20.0, 18_000.0, 1_000.0, Unit::Hz).octave_jack(5.0),
                KnobSpec::linear("resonance", 0.0, 0.95, 0.2, Unit::None),
                KnobSpec::linear("mode", 0.0, 2.0, 0.0, Unit::None).named(&FILTER_MODES),
                KnobSpec::linear("drive", 0.0, 1.0, 0.0, Unit::None),
            ],
            vec![audio("in", "sound to filter")],
            vec![audio("out", "the filtered sound")],
            dsp::vcf,
        ),
    ]
}

/// `vca`, `env`, `clock`, `seq`.
fn shaping_kinds() -> Vec<KindSpec> {
    use crate::dsp::native as dsp;
    let mut steps = vec![KnobSpec::stepped("steps", 1.0, 16.0, 8.0, Unit::Steps)];
    for name in [
        "step1", "step2", "step3", "step4", "step5", "step6", "step7", "step8", "step9", "step10",
        "step11", "step12", "step13", "step14", "step15", "step16",
    ] {
        steps.push(KnobSpec::linear(name, -24.0, 24.0, 0.0, Unit::Semitones));
    }
    steps.push(KnobSpec::linear("gate", 0.05, 1.0, 0.5, Unit::None));
    steps.push(KnobSpec::linear("chance", 0.0, 1.0, 1.0, Unit::None));
    vec![
        native(
            "vca",
            "amplifier: gain times the cv input plus bias, or plain gain when cv is unplugged",
            vec![
                KnobSpec::linear("gain", 0.0, 1.0, 1.0, Unit::None),
                KnobSpec::linear("bias", 0.0, 1.0, 0.0, Unit::None),
            ],
            vec![
                audio("in", "sound to amplify"),
                cv("cv", "gain control, bias plus gain times this"),
            ],
            vec![audio("out", "the amplified sound")],
            dsp::vca,
        ),
        native(
            "env",
            "ADSR envelope",
            vec![
                KnobSpec::log("attack", 0.001, 10.0, 0.01, Unit::Seconds),
                KnobSpec::log("decay", 0.001, 10.0, 0.3, Unit::Seconds),
                KnobSpec::linear("sustain", 0.0, 1.0, 0.5, Unit::None),
                KnobSpec::log("release", 0.001, 20.0, 0.5, Unit::Seconds),
            ],
            vec![gate("gate", "attack while high, release when it falls")],
            vec![cv("out", "envelope, 0 to 1")],
            dsp::env,
        ),
        native(
            "clock",
            "clock on the wall's beat",
            vec![
                KnobSpec::stepped("division", 0.0, 6.0, 1.0, Unit::None).named(&CLOCK_LABELS),
                KnobSpec::linear("swing", 0.0, 0.75, 0.0, Unit::None),
                KnobSpec::linear("width", 0.05, 0.95, 0.5, Unit::None),
            ],
            vec![gate("reset", "restarts the count on a rising gate")],
            vec![
                gate("out", "gate on every division"),
                cv("beat", "ramp 0 to 1 over each division"),
            ],
            dsp::clock,
        ),
        native(
            "seq",
            "step sequencer: up to 16 steps of pitch",
            steps,
            vec![
                gate("clock", "advances a step on a rising gate"),
                gate("reset", "the next clock plays step 1"),
            ],
            vec![
                cv("pitch", "the step's pitch, 1.0 per octave"),
                gate("gate", "gate for each step that plays"),
            ],
            dsp::seq,
        ),
    ]
}

/// `sh`, `quant`, `slew`, `mix`, `out`.
fn control_kinds() -> Vec<KindSpec> {
    use crate::dsp::native as dsp;
    vec![
        native(
            "sh",
            "sample and hold: samples its input (or noise, when unplugged) on each trigger",
            vec![KnobSpec::linear("slew", 0.0, 1.0, 0.0, Unit::Seconds)],
            vec![
                cv("in", "signal to sample; noise when unplugged"),
                gate("trig", "samples on a rising gate"),
            ],
            vec![cv("out", "the held value")],
            dsp::sh,
        ),
        native(
            "quant",
            "quantiser: holds pitch to the nearest note of a scale",
            vec![
                KnobSpec::stepped("scale", 0.0, 9.0, 0.0, Unit::None).named(&SCALE_NAMES),
                KnobSpec::stepped("root", 0.0, 11.0, 0.0, Unit::None).named(&NOTE_NAMES),
            ],
            vec![cv("in", "pitch, 1.0 per octave")],
            vec![cv("out", "quantised pitch")],
            dsp::quant,
        ),
        native(
            "slew",
            "slew limiter: rise and fall are the time to move 1.0",
            vec![
                KnobSpec::log("rise", 0.0, 5.0, 0.1, Unit::Seconds),
                KnobSpec::log("fall", 0.0, 5.0, 0.1, Unit::Seconds),
            ],
            vec![cv("in", "any signal")],
            vec![cv("out", "the slewed signal")],
            dsp::slew,
        ),
        native(
            "mix",
            "four-input mixer",
            vec![
                KnobSpec::linear("level_a", 0.0, 1.0, 0.5, Unit::None),
                KnobSpec::linear("level_b", 0.0, 1.0, 0.5, Unit::None),
                KnobSpec::linear("level_c", 0.0, 1.0, 0.5, Unit::None),
                KnobSpec::linear("level_d", 0.0, 1.0, 0.5, Unit::None),
            ],
            vec![
                audio("a", "signal, at level_a"),
                audio("b", "signal, at level_b"),
                audio("c", "signal, at level_c"),
                audio("d", "signal, at level_d"),
            ],
            vec![audio("out", "the sum")],
            dsp::mix,
        ),
        native(
            "out",
            "output to the master bus; mono when right is unplugged",
            vec![
                KnobSpec::linear("level", 0.0, 1.0, 0.7, Unit::None),
                KnobSpec::linear("pan", -1.0, 1.0, 0.0, Unit::None),
            ],
            vec![
                audio("left", "sound, left or mono"),
                audio("right", "sound, right"),
            ],
            Vec::new(),
            dsp::out,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn knob(kind: Kind, name: &str) -> &'static KnobSpec {
        let spec = kind.spec();
        &spec.knobs[spec.knob_index(name).unwrap()]
    }

    #[test]
    fn a_candidate_with_two_ports_of_one_name_is_refused() {
        let candidate = |inputs: Vec<PortSpec>, outputs: Vec<PortSpec>| Candidate {
            name: "twin",
            family: "fx",
            about: String::new(),
            params: &[],
            inputs,
            outputs,
            build: Builder::Adapted(crate::adapters::Adapter::Speak),
        };
        let gate_twice = vec![gate("gate", "one"), gate("gate", "two")];
        assert!(admit(candidate(Vec::new(), gate_twice.clone()), &[]).is_err());
        assert!(admit(candidate(gate_twice, Vec::new()), &[]).is_err());
        // One name on an input and an output is fine (as `in`/`out` pairs
        // and effects' `left`/`right` are).
        let both = candidate(vec![gate("gate", "in")], vec![gate("gate", "out")]);
        assert!(admit(both, &[]).is_ok());
    }

    #[test]
    fn the_constants_name_the_wall_s_own_kinds() {
        for (kind, name) in [
            (Kind::VCO, "vco"),
            (Kind::LFO, "lfo"),
            (Kind::NOISE, "noise"),
            (Kind::VCF, "vcf"),
            (Kind::VCA, "vca"),
            (Kind::ENV, "env"),
            (Kind::CLOCK, "clock"),
            (Kind::SEQ, "seq"),
            (Kind::SH, "sh"),
            (Kind::QUANT, "quant"),
            (Kind::SLEW, "slew"),
            (Kind::MIX, "mix"),
            (Kind::OUT, "out"),
        ] {
            assert_eq!(kind.name(), name);
            assert_eq!(kind.spec().family, "synth");
        }
        // The wall's own delay and reverb are gone: effects come from kazoo-fx.
        assert!(Kind::from_name("delay").is_none());
        assert!(Kind::from_name("reverb").is_none());
    }

    #[test]
    fn every_kind_fits_the_engine_and_names_round_trip() {
        for kind in Kind::all() {
            let spec = kind.spec();
            assert_eq!(spec.kind, kind);
            assert_eq!(Kind::from_name(kind.name()), Some(kind));
            assert!(spec.knobs.len() <= MAX_KNOBS, "{kind}");
            assert!(spec.inputs.len() <= MAX_INPUTS, "{kind}");
            assert!(spec.outputs.len() <= MAX_OUTPUTS, "{kind}");
            for knob in &spec.knobs {
                assert!(knob.min < knob.max, "{kind}.{}", knob.name);
                assert!(
                    (knob.clamp(knob.default) - knob.default).abs() < f32::EPSILON,
                    "{kind}.{} default out of range",
                    knob.name
                );
            }
            let names = spec.jack_names();
            for name in &names {
                assert_eq!(
                    names.iter().filter(|n| *n == name).count(),
                    1,
                    "{kind}.{name}"
                );
            }
        }
        assert_eq!(Kind::from_name("vcoo"), None);
    }

    #[test]
    fn effects_become_stereo_kinds_with_a_knob_per_parameter() {
        let kind = Kind::from_name("testgain").unwrap();
        let spec = kind.spec();
        assert_eq!(spec.family, "fx");
        assert_eq!(spec.jack_names(), vec!["left", "right", "gain", "mode"]);
        assert_eq!(spec.outputs.len(), 2);
        assert!(matches!(spec.build, Builder::Effect(_)));
        let mode = knob(kind, "mode");
        assert!(mode.stepped);
        assert_eq!(mode.labels, &["soft", "hard"]);
        assert_eq!(knob(kind, "gain").unit, Unit::Other("dB"));
    }

    #[test]
    fn effects_that_do_not_fit_are_left_out_with_a_reason() {
        use crate::dsp::effect::testing::{BAD_KINDS, KINDS};
        let registry = Registry::build(BAD_KINDS.iter().chain(KINDS.iter()));
        assert_eq!(registry.kinds().len(), native_kinds().len() + 1);
        assert_eq!(registry.skipped().len(), BAD_KINDS.len());
        for (why, word) in
            registry
                .skipped()
                .iter()
                .zip(["lower-case", "another", "taken", "range"])
        {
            assert!(why.contains(word), "{why}");
        }
    }

    #[test]
    fn safety_caps_hold() {
        assert!((knob(Kind::VCF, "resonance").clamp(5.0) - 0.95).abs() < f32::EPSILON);
        assert!((knob(Kind::OUT, "level").clamp(3.0) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn clamping_rounds_steps_and_refuses_nonsense() {
        let octave = knob(Kind::VCO, "octave");
        assert!((octave.clamp(1.4) - 1.0).abs() < f32::EPSILON);
        assert!((octave.clamp(-9.0) + 4.0).abs() < f32::EPSILON);
        assert!((octave.clamp(f32::NAN)).abs() < f32::EPSILON);
        let cutoff = knob(Kind::VCF, "cutoff");
        assert!((cutoff.clamp(f32::INFINITY) - 1_000.0).abs() < f32::EPSILON);
    }

    #[test]
    fn log_knobs_travel_in_ratios() {
        let cutoff = knob(Kind::VCF, "cutoff");
        assert!(cutoff.normalise(20.0).abs() < 1e-6);
        assert!((cutoff.normalise(18_000.0) - 1.0).abs() < 1e-6);
        let middle = cutoff.denormalise(0.5);
        assert!(
            (middle - (20.0_f32 * 18_000.0).sqrt()).abs() < 0.5,
            "{middle}"
        );
        for value in [25.0, 440.0, 9_000.0] {
            let back = cutoff.denormalise(cutoff.normalise(value));
            assert!((back - value).abs() / value < 1e-4, "{value} -> {back}");
        }
        // A zero-based log range still reaches zero at the bottom.
        let rise = knob(Kind::SLEW, "rise");
        assert!(rise.denormalise(0.0).abs() < f32::EPSILON);
        assert!((rise.denormalise(1.0) - 5.0).abs() < 1e-4);
        assert!(rise.normalise(0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn a_jack_adds_half_the_range_per_volt_and_stays_in_range() {
        let width = knob(Kind::VCO, "width");
        assert!((width.modulate(0.5, 1.0) - 0.95).abs() < 1e-6);
        assert!((width.modulate(0.5, 0.2) - 0.59).abs() < 1e-6);
        assert!((width.modulate(0.5, f32::NAN) - 0.5).abs() < 1e-6);
        let rate = knob(Kind::LFO, "rate");
        assert!((rate.modulate(1.0, 0.1) - 2.4995).abs() < 1e-3);
        assert!((rate.modulate(1.0, -10.0) - 0.01).abs() < 1e-6);
        // Modulation never crosses a safety cap.
        let resonance = knob(Kind::VCF, "resonance");
        assert!((resonance.modulate(0.9, 5.0) - 0.95).abs() < 1e-6);
        // Stepped knobs stay on whole steps.
        let division = knob(Kind::CLOCK, "division");
        assert!((division.modulate(1.0, 0.3) - 2.0).abs() < 1e-6);
    }

    #[test]
    fn the_cutoff_jack_moves_octaves() {
        let cutoff = knob(Kind::VCF, "cutoff");
        assert!((cutoff.modulate(200.0, 1.0) - 6_400.0).abs() < 0.5);
        assert!((cutoff.modulate(200.0, -0.2) - 100.0).abs() < 0.01);
        assert!((cutoff.modulate(10_000.0, 1.0) - 18_000.0).abs() < 0.5);
    }

    #[test]
    fn jacks_are_found_by_name() {
        let vcf = Kind::VCF.spec();
        assert_eq!(vcf.jack("in"), Some(Jack::Input(0)));
        assert_eq!(vcf.jack("resonance"), Some(Jack::Knob(1)));
        assert_eq!(vcf.jack("out"), None);
        assert_eq!(vcf.output_index("out"), Some(0));
        assert_eq!(Kind::SEQ.spec().knobs.len(), 19);
        assert_eq!(Kind::SEQ.spec().defaults().len(), 19);
    }

    #[test]
    fn sync_tables_read_the_knob() {
        assert_eq!(sync_beats(&SYNC_DIVISIONS, 0.0), None);
        assert_eq!(sync_beats(&SYNC_DIVISIONS, 3.0), Some(1.0));
        assert_eq!(sync_beats(&SYNC_DIVISIONS, 9.0), Some(64.0));
        assert_eq!(sync_beats(&SYNC_DIVISIONS, 40.0), Some(64.0));
        assert_eq!(sync_beats(&SYNC_DIVISIONS, f32::NAN), None);
        assert_eq!(SYNC_LABELS.len(), SYNC_DIVISIONS.len() + 1);
        assert_eq!(SCALE_NAMES.len(), SCALES.len());
    }

    #[test]
    fn an_effect_named_with_digits_is_taken() {
        use crate::dsp::effect::testing::DIGIT_KINDS;
        let registry = Registry::build(DIGIT_KINDS.iter());
        assert!(registry.skipped().is_empty(), "{:?}", registry.skipped());
        assert!(registry.kinds().iter().any(|spec| spec.name == "sampler12"));
    }

    #[test]
    fn every_kind_the_studio_offers_fits() {
        assert!(
            registry().skipped().is_empty(),
            "{:?}",
            registry().skipped()
        );
    }

    #[test]
    fn kind_names_may_hold_digits() {
        for good in ["sampler12", "tape", "big_room", "x", "a1_b2"] {
            assert!(valid_kind_name(good), "{good}");
        }
        for bad in [
            "",
            "12sampler",
            "Bad-Id",
            "tape_",
            "_tape",
            "sampler 1",
            "averyveryverylongkindname1",
        ] {
            assert!(!valid_kind_name(bad), "{bad}");
        }
    }

    #[test]
    fn effect_units_map_to_the_wall_s() {
        assert_eq!(Unit::from_text("Hz"), Unit::Hz);
        assert_eq!(Unit::from_text("s"), Unit::Seconds);
        assert_eq!(Unit::from_text(""), Unit::None);
        assert_eq!(Unit::from_text("%").name(), "%");
    }
}
