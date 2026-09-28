//! Percussion voices and rhythm generators as wall modules.
//!
//! A voice module is struck by a rising `trigger` gate at the exact sample
//! it rises: the block is split there, so the hit lands where the gate
//! does. The velocity is how far the gate rises past its threshold (0.5):
//! a gate of 1 or more is full velocity, 0.75 is half, and one only just
//! high barely sounds. The `accent` gate held at that sample plays it
//! accented, and
//! a rising `choke` gate damps whatever is ringing. A rhythm module reads
//! its `clock` and `reset` gates sample by sample and writes one output per
//! pattern lane.

use kazoo_perc::{Rhythm, RhythmKind, Voice, VoiceKind};

use super::apply;
use crate::dsp::{Edge, Io, Module, Tick, finite};
use crate::{MAX_KNOBS, MAX_OUTPUTS, SUB_BLOCK};

/// Voice inputs, in catalogue order.
pub const IN_TRIGGER: usize = 0;
/// See [`IN_TRIGGER`].
pub const IN_ACCENT: usize = 1;
/// See [`IN_TRIGGER`].
pub const IN_CHOKE: usize = 2;

/// Rhythm inputs, in catalogue order.
pub const IN_CLOCK: usize = 0;
/// See [`IN_CLOCK`].
pub const IN_RESET: usize = 1;

/// Output bound: voices stay below +6 dBFS, so this never bites.
const LIMIT: f32 = 16.0;

/// The velocity of a strike by a gate at `level` (above the threshold):
/// the part above it, stretched to 0–1. Never 0, which would choke.
fn velocity(level: f32) -> f32 {
    ((level - crate::dsp::GATE_HIGH) * 2.0).clamp(f32::MIN_POSITIVE, 1.0)
}

/// A percussion voice, struck by its gates.
#[derive(Debug)]
pub struct VoiceModule {
    voice: Box<dyn Voice>,
    applied: [f32; MAX_KNOBS],
    trigger: Edge,
    choke: Edge,
}

impl VoiceModule {
    /// Build `kind` and prepare it for `sample_rate` (this allocates).
    #[must_use]
    pub fn new(kind: &'static VoiceKind, sample_rate: f32) -> Self {
        let mut voice = (kind.build)();
        voice.prepare(sample_rate);
        Self {
            voice,
            applied: [f32::NAN; MAX_KNOBS],
            trigger: Edge::new(),
            choke: Edge::new(),
        }
    }
}

impl Module for VoiceModule {
    fn process(&mut self, _tick: &Tick, io: Io<'_>) {
        let voice = &mut self.voice;
        apply(&io, &mut self.applied, |index, value| {
            voice.set_param(index, value);
        });
        let out = &mut io.outputs[0];
        let mut from = 0;
        for frame in 0..SUB_BLOCK {
            let level = finite(io.inputs[IN_TRIGGER][frame]);
            let choked = self.choke.rising(io.inputs[IN_CHOKE][frame]);
            let struck = self.trigger.rising(level);
            if !(choked || struck) {
                continue;
            }
            // Everything before this sample sounds as it was.
            self.voice.process(&mut out[from..frame]);
            from = frame;
            if choked {
                self.voice.trigger(0.0, false);
            }
            if struck {
                let accent = finite(io.inputs[IN_ACCENT][frame]) > crate::dsp::GATE_HIGH;
                self.voice.trigger(velocity(level), accent);
            }
        }
        self.voice.process(&mut out[from..]);
        for sample in out.iter_mut() {
            *sample = finite(*sample).clamp(-LIMIT, LIMIT);
        }
    }

    fn reset(&mut self) {
        self.voice.reset();
        self.trigger.reset();
        self.choke.reset();
    }
}

/// A rhythm generator, stepped by its clock.
#[derive(Debug)]
pub struct RhythmModule {
    rhythm: Box<dyn Rhythm>,
    lanes: usize,
    applied: [f32; MAX_KNOBS],
}

impl RhythmModule {
    /// Build `kind` and prepare it for `sample_rate` (this allocates).
    #[must_use]
    pub fn new(kind: &'static RhythmKind, sample_rate: f32) -> Self {
        let mut rhythm = (kind.build)();
        rhythm.prepare(sample_rate);
        Self {
            rhythm,
            lanes: kind.outputs.len().min(MAX_OUTPUTS),
            applied: [f32::NAN; MAX_KNOBS],
        }
    }
}

impl Module for RhythmModule {
    fn process(&mut self, _tick: &Tick, io: Io<'_>) {
        let rhythm = &mut self.rhythm;
        apply(&io, &mut self.applied, |index, value| {
            rhythm.set_param(index, value);
        });
        let [a, b, c, d] = &mut *io.outputs;
        let mut lanes: [&mut [f32]; MAX_OUTPUTS] = [a, b, c, d];
        self.rhythm.process(
            &io.inputs[IN_CLOCK],
            &io.inputs[IN_RESET],
            &mut lanes[..self.lanes],
        );
    }

    fn reset(&mut self) {
        self.rhythm.reset();
    }
}

#[cfg(test)]
mod tests;
