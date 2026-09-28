//! The player's knobs, and what they mean for one block.

use kazoo_fx::{Curve, ParamSpec};

/// How a voice plays its sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// From start to end once per trigger, whatever the gate does after.
    OneShot,
    /// From start to end while the gate is high; released when it falls.
    Gate,
    /// From start into the loop, round the loop while held, then released.
    Loop,
    /// Like loop, but back and forth between the loop points.
    PingPong,
    /// One slice per trigger, picked by the `slice` knob.
    Slice,
    /// A cloud of grains read around a position, while held.
    Granular,
}

impl Mode {
    /// The mode a `mode` knob value names.
    #[must_use]
    pub const fn from_step(step: u32) -> Self {
        match step {
            0 => Self::OneShot,
            1 => Self::Gate,
            2 => Self::Loop,
            3 => Self::PingPong,
            4 => Self::Slice,
            _ => Self::Granular,
        }
    }

    /// Whether the gate falling releases the voice.
    #[must_use]
    pub const fn is_held(self) -> bool {
        !matches!(self, Self::OneShot | Self::Slice)
    }
}

/// Knob indices, in the order of [`PARAMS`].
pub mod index {
    /// Playback mode (stepped).
    pub const MODE: usize = 0;
    /// Play backwards (stepped).
    pub const REVERSE: usize = 1;
    /// Transpose in semitones.
    pub const PITCH: usize = 2;
    /// Fine tune in cents.
    pub const FINE: usize = 3;
    /// Where playback starts, as a fraction of the sample.
    pub const START: usize = 4;
    /// Where playback ends, as a fraction of the sample.
    pub const END: usize = 5;
    /// Loop start, as a fraction of the sample.
    pub const LOOP_START: usize = 6;
    /// Loop end, as a fraction of the sample.
    pub const LOOP_END: usize = 7;
    /// Loop crossfade in seconds.
    pub const CROSSFADE: usize = 8;
    /// Envelope attack.
    pub const ATTACK: usize = 9;
    /// Envelope decay.
    pub const DECAY: usize = 10;
    /// Envelope sustain level.
    pub const SUSTAIN: usize = 11;
    /// Envelope release.
    pub const RELEASE: usize = 12;
    /// Output level.
    pub const LEVEL: usize = 13;
    /// Stereo balance.
    pub const PAN: usize = 14;
    /// Velocity sensitivity.
    pub const VELOCITY: usize = 15;
    /// How the sample is sliced (stepped).
    pub const SLICES: usize = 16;
    /// Which slice plays.
    pub const SLICE: usize = 17;
    /// Granular read position.
    pub const POSITION: usize = 18;
    /// Grain length.
    pub const SIZE: usize = 19;
    /// Grains per second.
    pub const DENSITY: usize = 20;
    /// Random spread of grain positions.
    pub const SPRAY: usize = 21;
    /// Random spread of grain pitches.
    pub const SPREAD: usize = 22;
    /// Hold the granular position still (stepped).
    pub const FREEZE: usize = 23;
}

/// The most equal slices the `slices` knob offers.
pub const MAX_SLICES: u32 = 32;

const MODES: &[&str] = &["one-shot", "gate", "loop", "ping-pong", "slice", "granular"];
const OFF_ON: &[&str] = &["off", "on"];
const SLICINGS: &[&str] = &[
    "onsets", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15",
    "16", "17", "18", "19", "20", "21", "22", "23", "24", "25", "26", "27", "28", "29", "30", "31",
    "32",
];

const fn linear(
    name: &'static str,
    min: f32,
    max: f32,
    default: f32,
    unit: &'static str,
) -> ParamSpec {
    ParamSpec {
        name,
        min,
        max,
        default,
        unit,
        curve: Curve::Linear,
    }
}

const fn log(
    name: &'static str,
    min: f32,
    max: f32,
    default: f32,
    unit: &'static str,
) -> ParamSpec {
    ParamSpec {
        name,
        min,
        max,
        default,
        unit,
        curve: Curve::Log,
    }
}

const fn stepped(name: &'static str, labels: &'static [&'static str], default: f32) -> ParamSpec {
    ParamSpec {
        name,
        min: 0.0,
        max: (labels.len() - 1) as f32,
        default,
        unit: "",
        curve: Curve::Stepped { labels },
    }
}

/// Every knob of a [`super::SamplePlayer`], in index order.
pub const PARAMS: &[ParamSpec] = &[
    stepped("mode", MODES, 0.0),
    stepped("reverse", OFF_ON, 0.0),
    linear("pitch", -48.0, 48.0, 0.0, "st"),
    linear("fine", -100.0, 100.0, 0.0, "ct"),
    linear("start", 0.0, 1.0, 0.0, ""),
    linear("end", 0.0, 1.0, 1.0, ""),
    linear("loop_start", 0.0, 1.0, 0.0, ""),
    linear("loop_end", 0.0, 1.0, 1.0, ""),
    linear("crossfade", 0.0, 1.0, 0.01, "s"),
    log("attack", 0.000_5, 10.0, 0.002, "s"),
    log("decay", 0.001, 10.0, 0.5, "s"),
    linear("sustain", 0.0, 1.0, 1.0, ""),
    log("release", 0.001, 20.0, 0.1, "s"),
    linear("level", 0.0, 1.0, 0.8, ""),
    linear("pan", -1.0, 1.0, 0.0, ""),
    linear("velocity", 0.0, 1.0, 0.5, ""),
    stepped("slices", SLICINGS, 0.0),
    linear("slice", 0.0, 1.0, 0.0, ""),
    linear("position", 0.0, 1.0, 0.0, ""),
    log("size", 0.005, 1.0, 0.08, "s"),
    log("density", 1.0, 200.0, 20.0, "Hz"),
    linear("spray", 0.0, 1.0, 0.02, "s"),
    linear("spread", 0.0, 24.0, 0.0, "st"),
    stepped("freeze", OFF_ON, 0.0),
];

/// The number of knobs.
pub const PARAM_COUNT: usize = PARAMS.len();

/// The index of the knob called `name`.
#[must_use]
pub fn param_index(name: &str) -> Option<usize> {
    PARAMS.iter().position(|spec| spec.name == name)
}

/// Every knob at its default.
pub(crate) fn defaults() -> [f32; PARAM_COUNT] {
    let mut values = [0.0; PARAM_COUNT];
    for (value, spec) in values.iter_mut().zip(PARAMS) {
        *value = spec.default;
    }
    values
}

/// The envelope's per-sample rates.
#[derive(Debug, Clone, Copy)]
pub(crate) struct EnvRates {
    /// Level added per sample while attacking.
    pub(crate) attack_step: f32,
    /// Per-sample factor toward sustain (60 dB over the decay time).
    pub(crate) decay: f32,
    pub(crate) sustain: f32,
    /// Per-sample factor toward silence (60 dB over the release time).
    pub(crate) release: f32,
}

/// Grain settings, in frames at the engine rate.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GrainSettings {
    pub(crate) position: f64,
    pub(crate) size: f64,
    pub(crate) interval: f64,
    pub(crate) spray: f64,
    pub(crate) spread: f32,
    pub(crate) freeze: bool,
    /// Loudness correction for overlapping grains.
    pub(crate) gain: f32,
}

/// The knobs worked out for one block.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Settings {
    pub(crate) mode: Mode,
    pub(crate) reverse: bool,
    /// Knob transposition in octaves.
    pub(crate) transpose: f32,
    pub(crate) start: f64,
    pub(crate) end: f64,
    pub(crate) loop_start: f64,
    pub(crate) loop_end: f64,
    /// Crossfade in seconds.
    pub(crate) crossfade: f64,
    pub(crate) env: EnvRates,
    pub(crate) level: f32,
    pub(crate) pan: f32,
    pub(crate) velocity: f32,
    /// 0 for onset slices, otherwise that many equal slices.
    pub(crate) slices: u32,
    pub(crate) slice: f32,
    pub(crate) grain: GrainSettings,
}

/// 60 dB, as a natural log of the amplitude ratio.
const SIXTY_DB: f32 = -6.907_755;

impl Settings {
    /// Work out what `values` mean at `rate`.
    pub(crate) fn new(values: &[f32; PARAM_COUNT], rate: f32) -> Self {
        use index as i;
        let per_sample = |seconds: f32| (SIXTY_DB / (seconds * rate).max(1.0)).exp();
        let size = f64::from(values[i::SIZE] * rate).max(1.0);
        let interval = f64::from(rate / values[i::DENSITY]).max(1.0);
        Self {
            mode: Mode::from_step(values[i::MODE] as u32),
            reverse: values[i::REVERSE] >= 0.5,
            transpose: (values[i::FINE] / 100.0 + values[i::PITCH]) / 12.0,
            start: f64::from(values[i::START]),
            end: f64::from(values[i::END]),
            loop_start: f64::from(values[i::LOOP_START]),
            loop_end: f64::from(values[i::LOOP_END]),
            crossfade: f64::from(values[i::CROSSFADE]),
            env: EnvRates {
                attack_step: 1.0 / (values[i::ATTACK] * rate).max(1.0),
                decay: per_sample(values[i::DECAY]),
                sustain: values[i::SUSTAIN],
                release: per_sample(values[i::RELEASE]),
            },
            level: values[i::LEVEL],
            pan: values[i::PAN],
            velocity: values[i::VELOCITY],
            slices: values[i::SLICES] as u32,
            slice: values[i::SLICE],
            grain: GrainSettings {
                position: f64::from(values[i::POSITION]),
                size,
                interval,
                spray: f64::from(values[i::SPRAY] * rate),
                spread: values[i::SPREAD],
                freeze: values[i::FREEZE] >= 0.5,
                gain: (1.0 / (size / interval).max(1.0).sqrt()) as f32,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_consistent() {
        assert_eq!(PARAMS.len(), index::FREEZE + 1);
        assert_eq!(param_index("slice"), Some(index::SLICE));
        assert_eq!(param_index("nope"), None);
        for spec in PARAMS {
            assert!(spec.min < spec.max, "{}", spec.name);
            assert!(
                (spec.min..=spec.max).contains(&spec.default),
                "{}",
                spec.name
            );
            if spec.curve == Curve::Log {
                assert!(spec.min > 0.0, "{}", spec.name);
            }
            if let Curve::Stepped { labels } = spec.curve {
                assert!((spec.max - spec.min + 1.0 - labels.len() as f32).abs() < f32::EPSILON);
            }
        }
        assert_eq!(SLICINGS.len() as u32, MAX_SLICES + 1);
        let settings = Settings::new(&defaults(), 48_000.0);
        assert_eq!(settings.mode, Mode::OneShot);
        assert!(settings.env.release > 0.0 && settings.env.release < 1.0);
    }
}
