//! The vocoder and the phrase player as wall modules.
//!
//! The vocoder hears its `carrier` (left and right; the right follows the
//! left when unplugged) through its `modulator`'s mouth, in stereo. The
//! speaker plays the phrase the `speak` op last gave it, on its `gate`:
//! the phrase arrives through a lock-free feed and the old one goes back
//! the same way, to be freed off the audio thread.

use kazoo_speech::{PhraseFeed, SpeechPlayer, Vocoder};

use super::apply;
use crate::dsp::{Block, Io, Module, Tick, finite};
use crate::{MAX_KNOBS, SUB_BLOCK};

/// Vocoder inputs, in catalogue order.
pub const IN_CARRIER_LEFT: usize = 0;
/// See [`IN_CARRIER_LEFT`].
pub const IN_CARRIER_RIGHT: usize = 1;
/// See [`IN_CARRIER_LEFT`].
pub const IN_MODULATOR: usize = 2;

/// The speaker's input.
pub const IN_GATE: usize = 0;

/// Input bound: anything louder is held here before it reaches the DSP.
const LIMIT: f32 = 16.0;

/// The channel vocoder.
#[derive(Debug)]
pub struct VocoderModule {
    vocoder: Vocoder,
    applied: [f32; MAX_KNOBS],
    carrier: [Block; 2],
    modulator: Block,
}

impl VocoderModule {
    /// A vocoder for `sample_rate` (this allocates).
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        Self {
            vocoder: Vocoder::new(sample_rate),
            applied: [f32::NAN; MAX_KNOBS],
            carrier: [[0.0; SUB_BLOCK]; 2],
            modulator: [0.0; SUB_BLOCK],
        }
    }
}

impl Module for VocoderModule {
    fn process(&mut self, _tick: &Tick, io: Io<'_>) {
        let vocoder = &mut self.vocoder;
        apply(&io, &mut self.applied, |index, value| {
            vocoder.set_param(index, value);
        });
        let stereo = io.connected[IN_CARRIER_RIGHT];
        for frame in 0..SUB_BLOCK {
            let left = finite(io.inputs[IN_CARRIER_LEFT][frame]).clamp(-LIMIT, LIMIT);
            self.carrier[0][frame] = left;
            self.carrier[1][frame] = if stereo {
                finite(io.inputs[IN_CARRIER_RIGHT][frame]).clamp(-LIMIT, LIMIT)
            } else {
                left
            };
            self.modulator[frame] = finite(io.inputs[IN_MODULATOR][frame]).clamp(-LIMIT, LIMIT);
        }
        let [left, right, ..] = &mut *io.outputs;
        self.vocoder.process(
            [&self.carrier[0], &self.carrier[1]],
            &self.modulator,
            [left, right],
        );
    }

    fn reset(&mut self) {
        self.vocoder.reset();
    }
}

/// The phrase player.
#[derive(Debug)]
pub struct SpeakModule {
    player: SpeechPlayer,
    applied: [f32; MAX_KNOBS],
}

impl SpeakModule {
    /// A player for `sample_rate`, with the feed that hands it phrases (this
    /// allocates).
    #[must_use]
    pub fn new(sample_rate: f32) -> (Self, PhraseFeed) {
        let (player, feed) = SpeechPlayer::new(sample_rate);
        (
            Self {
                player,
                applied: [f32::NAN; MAX_KNOBS],
            },
            feed,
        )
    }
}

impl Module for SpeakModule {
    fn process(&mut self, _tick: &Tick, io: Io<'_>) {
        let player = &mut self.player;
        apply(&io, &mut self.applied, |index, value| {
            player.set_param(index, value);
        });
        self.player.process(&io.inputs[IN_GATE], &mut io.outputs[0]);
    }

    fn reset(&mut self) {
        self.player.reset();
    }
}

#[cfg(test)]
mod tests;
