//! Real-time-safe audio block transport primitives.
//!
//! This module provides the in-process shape of Kazoo's future shared-memory
//! audio plane: fixed-size, frame-indexed audio blocks moving through bounded
//! lock-free rings. The backing storage is currently `ringbuf::HeapRb`, but the
//! public semantics are deliberately block-oriented so the mixer can later swap
//! in a shared-memory implementation without changing callback mixing logic.
//!
//! Design rules:
//!
//! - allocate during construction, never in `push_*`/`pop_*` hot paths;
//! - blocks are fixed-capacity and carry absolute frame/sequence metadata;
//! - late/missing blocks are observable as underruns, never waits;
//! - non-finite samples are sanitized before entering the transport.

use std::fmt;

use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Observer, Producer, Split};

use crate::protocol::{AudioBlockHeader, BlockFlags, BufferId};
use crate::{DEFAULT_BUFFER_SIZE, sanitize_sample};

/// Default number of blocks retained in an audio block ring.
pub const DEFAULT_AUDIO_RING_BLOCKS: usize = 8;

/// Configuration for a fixed-capacity audio block ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioRingConfig {
    /// Identifier of the logical audio buffer.
    pub buffer_id: BufferId,
    /// Interleaved channel count.
    pub channels: u16,
    /// Frames per block.
    pub block_frames: u32,
    /// Number of whole blocks retained in the ring.
    pub capacity_blocks: u32,
}

impl AudioRingConfig {
    /// Create a new audio ring configuration, clamping zero values to safe
    /// defaults.
    #[must_use]
    pub const fn new(
        buffer_id: BufferId,
        channels: u16,
        block_frames: u32,
        capacity_blocks: u32,
    ) -> Self {
        Self {
            buffer_id,
            channels: if channels == 0 { 1 } else { channels },
            block_frames: if block_frames == 0 {
                DEFAULT_BUFFER_SIZE as u32
            } else {
                block_frames
            },
            capacity_blocks: if capacity_blocks == 0 {
                DEFAULT_AUDIO_RING_BLOCKS as u32
            } else {
                capacity_blocks
            },
        }
    }

    /// Number of interleaved samples in one block.
    #[must_use]
    pub const fn samples_per_block(self) -> usize {
        self.channels as usize * self.block_frames as usize
    }

    /// Ring capacity measured in interleaved samples.
    #[must_use]
    pub const fn sample_capacity(self) -> usize {
        self.samples_per_block() * self.capacity_blocks as usize
    }
}

/// Frame-indexed audio block borrowed from the caller.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioBlock<'a> {
    /// Block metadata.
    pub header: AudioBlockHeader,
    /// Interleaved sample payload. Length must match `frames * channels`.
    pub samples: &'a [f32],
}

/// Owned metadata returned by an audio block consumer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoppedAudioBlock {
    /// Block metadata.
    pub header: AudioBlockHeader,
    /// Number of samples copied into the destination buffer.
    pub samples_copied: usize,
}

/// Push failure from a full or invalid audio block ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioRingPushError {
    /// The provided sample slice does not match the header frame/channel count.
    InvalidSampleCount {
        /// Expected interleaved sample count.
        expected: usize,
        /// Actual interleaved sample count.
        actual: usize,
    },
    /// The ring has insufficient free capacity for the complete block.
    Full,
}

/// Pop failure from an empty or undersized audio block ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioRingPopError {
    /// No complete block is currently available.
    Empty,
    /// Caller-provided output buffer is too small for this block.
    OutputTooSmall {
        /// Required interleaved sample count.
        required: usize,
        /// Actual destination capacity.
        actual: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BlockMeta {
    header: AudioBlockHeader,
    samples: usize,
}

/// Producer half of a fixed-capacity frame-indexed audio ring.
pub struct AudioBlockProducer {
    config: AudioRingConfig,
    sample_prod: ringbuf::HeapProd<f32>,
    meta_prod: ringbuf::HeapProd<BlockMeta>,
    next_sequence: u64,
}

impl fmt::Debug for AudioBlockProducer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AudioBlockProducer")
            .field("config", &self.config)
            .field("next_sequence", &self.next_sequence)
            .finish_non_exhaustive()
    }
}

impl AudioBlockProducer {
    /// Ring configuration.
    #[must_use]
    pub const fn config(&self) -> AudioRingConfig {
        self.config
    }

    /// Push an audio block into the ring.
    ///
    /// This method sanitizes samples while copying. If the full block cannot be
    /// accepted, no partial block metadata is committed and `Full` is returned.
    pub fn push_block(&mut self, block: AudioBlock<'_>) -> Result<(), AudioRingPushError> {
        let frames = block.header.frames as usize;
        let channels = block.header.channels as usize;
        let expected = frames * channels;
        let actual = block.samples.len();

        if expected != actual || channels == 0 || frames == 0 {
            return Err(AudioRingPushError::InvalidSampleCount { expected, actual });
        }

        if expected > self.sample_prod.vacant_len() || self.meta_prod.is_full() {
            return Err(AudioRingPushError::Full);
        }

        for sample in block.samples {
            // Capacity was checked above; if this fails, the ring invariant is
            // broken. Keep returning Full rather than panicking in real-time code.
            if self.sample_prod.try_push(sanitize_sample(*sample)).is_err() {
                return Err(AudioRingPushError::Full);
            }
        }

        let mut header = block.header;
        if header.sequence == 0 {
            header.sequence = self.next_sequence;
            self.next_sequence = self.next_sequence.wrapping_add(1);
        }

        self.meta_prod
            .try_push(BlockMeta {
                header,
                samples: expected,
            })
            .map_err(|_| AudioRingPushError::Full)
    }

    /// Push a block marked as intentional silence.
    pub fn push_silence(
        &mut self,
        start_frame: u64,
        frames: u32,
    ) -> Result<(), AudioRingPushError> {
        let channels = self.config.channels;
        let sample_count = frames as usize * channels as usize;

        if sample_count > self.sample_prod.vacant_len() || self.meta_prod.is_full() {
            return Err(AudioRingPushError::Full);
        }

        for _ in 0..sample_count {
            if self.sample_prod.try_push(0.0).is_err() {
                return Err(AudioRingPushError::Full);
            }
        }

        let header = AudioBlockHeader {
            start_frame,
            frames,
            channels,
            sequence: self.next_sequence,
            flags: BlockFlags {
                silent: true,
                loop_wrapped: false,
                final_segment: true,
            },
        };
        self.next_sequence = self.next_sequence.wrapping_add(1);

        self.meta_prod
            .try_push(BlockMeta {
                header,
                samples: sample_count,
            })
            .map_err(|_| AudioRingPushError::Full)
    }
}

/// Consumer half of a fixed-capacity frame-indexed audio ring.
pub struct AudioBlockConsumer {
    config: AudioRingConfig,
    sample_cons: ringbuf::HeapCons<f32>,
    meta_cons: ringbuf::HeapCons<BlockMeta>,
    expected_sequence: u64,
    underruns: u64,
    sequence_gaps: u64,
}

impl fmt::Debug for AudioBlockConsumer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AudioBlockConsumer")
            .field("config", &self.config)
            .field("expected_sequence", &self.expected_sequence)
            .field("underruns", &self.underruns)
            .field("sequence_gaps", &self.sequence_gaps)
            .finish_non_exhaustive()
    }
}

impl AudioBlockConsumer {
    /// Ring configuration.
    #[must_use]
    pub const fn config(&self) -> AudioRingConfig {
        self.config
    }

    /// Number of observed empty-pop underruns.
    #[must_use]
    pub const fn underruns(&self) -> u64 {
        self.underruns
    }

    /// Number of observed sequence discontinuities.
    #[must_use]
    pub const fn sequence_gaps(&self) -> u64 {
        self.sequence_gaps
    }

    /// Pop one complete block into `output`.
    pub fn pop_block(&mut self, output: &mut [f32]) -> Result<PoppedAudioBlock, AudioRingPopError> {
        let Some(meta) = self.meta_cons.try_peek().copied() else {
            self.underruns = self.underruns.wrapping_add(1);
            return Err(AudioRingPopError::Empty);
        };

        if output.len() < meta.samples {
            // Do not consume metadata or samples if the caller cannot accept
            // the complete block. The next pop with a large enough buffer must
            // still see this same block.
            return Err(AudioRingPopError::OutputTooSmall {
                required: meta.samples,
                actual: output.len(),
            });
        }

        let meta = self.meta_cons.try_pop().unwrap_or(meta);

        if meta.header.sequence != self.expected_sequence {
            self.sequence_gaps = self.sequence_gaps.wrapping_add(1);
            self.expected_sequence = meta.header.sequence;
        }
        self.expected_sequence = self.expected_sequence.wrapping_add(1);

        for sample in &mut output[..meta.samples] {
            *sample = self.sample_cons.try_pop().unwrap_or(0.0);
        }

        Ok(PoppedAudioBlock {
            header: meta.header,
            samples_copied: meta.samples,
        })
    }

    /// Pop the next block only if it starts at `start_frame`.
    ///
    /// If no block is available or the next block has a different start frame,
    /// `output` is filled with silence and an underrun is recorded. This method
    /// never waits for late producers.
    pub fn pop_expected_or_silence(
        &mut self,
        start_frame: u64,
        output: &mut [f32],
    ) -> Option<PoppedAudioBlock> {
        match self.pop_block(output) {
            Ok(block) if block.header.start_frame == start_frame => Some(block),
            Ok(block) => {
                self.sequence_gaps = self.sequence_gaps.wrapping_add(1);
                output[..block.samples_copied].fill(0.0);
                None
            }
            Err(_) => {
                let silence_len = self.config.samples_per_block().min(output.len());
                output[..silence_len].fill(0.0);
                None
            }
        }
    }
}

/// Create a fixed-capacity audio block ring.
#[must_use]
pub fn audio_block_ring(config: AudioRingConfig) -> (AudioBlockProducer, AudioBlockConsumer) {
    let sample_ring = HeapRb::<f32>::new(config.sample_capacity());
    let meta_ring = HeapRb::<BlockMeta>::new(config.capacity_blocks as usize);
    let (sample_prod, sample_cons) = sample_ring.split();
    let (meta_prod, meta_cons) = meta_ring.split();

    (
        AudioBlockProducer {
            config,
            sample_prod,
            meta_prod,
            next_sequence: 1,
        },
        AudioBlockConsumer {
            config,
            sample_cons,
            meta_cons,
            expected_sequence: 1,
            underruns: 0,
            sequence_gaps: 0,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> AudioRingConfig {
        AudioRingConfig::new(BufferId(1), 2, 4, 2)
    }

    #[test]
    fn config_clamps_zero_values() {
        let cfg = AudioRingConfig::new(BufferId(9), 0, 0, 0);

        assert_eq!(cfg.channels, 1);
        assert_eq!(cfg.block_frames, DEFAULT_BUFFER_SIZE as u32);
        assert_eq!(cfg.capacity_blocks, DEFAULT_AUDIO_RING_BLOCKS as u32);
    }

    #[test]
    fn pushes_and_pops_frame_indexed_block() {
        let (mut prod, mut cons) = audio_block_ring(config());
        let samples = [0.25; 8];
        let header = AudioBlockHeader {
            start_frame: 128,
            frames: 4,
            channels: 2,
            sequence: 0,
            flags: BlockFlags::default(),
        };

        prod.push_block(AudioBlock {
            header,
            samples: &samples,
        })
        .unwrap();

        let mut out = [0.0; 8];
        let popped = cons.pop_block(&mut out).unwrap();

        assert_eq!(popped.header.start_frame, 128);
        assert_eq!(popped.header.sequence, 1);
        assert_eq!(popped.samples_copied, 8);
        assert_eq!(out, samples);
    }

    #[test]
    fn rejects_invalid_sample_count() {
        let (mut prod, _) = audio_block_ring(config());
        let samples = [0.0; 7];
        let header = AudioBlockHeader {
            start_frame: 0,
            frames: 4,
            channels: 2,
            sequence: 0,
            flags: BlockFlags::default(),
        };

        assert_eq!(
            prod.push_block(AudioBlock {
                header,
                samples: &samples,
            }),
            Err(AudioRingPushError::InvalidSampleCount {
                expected: 8,
                actual: 7,
            })
        );
    }

    #[test]
    fn output_too_small_does_not_consume_block() {
        let (mut prod, mut cons) = audio_block_ring(config());
        let samples = [0.125; 8];
        let header = AudioBlockHeader {
            start_frame: 256,
            frames: 4,
            channels: 2,
            sequence: 0,
            flags: BlockFlags::default(),
        };

        prod.push_block(AudioBlock {
            header,
            samples: &samples,
        })
        .unwrap();

        let mut too_small = [0.0; 4];
        assert_eq!(
            cons.pop_block(&mut too_small),
            Err(AudioRingPopError::OutputTooSmall {
                required: 8,
                actual: 4,
            })
        );

        let mut out = [0.0; 8];
        let popped = cons.pop_block(&mut out).unwrap();
        assert_eq!(popped.header.start_frame, 256);
        assert_eq!(out, samples);
    }

    #[test]
    fn sanitizes_non_finite_samples() {
        let (mut prod, mut cons) = audio_block_ring(config());
        let samples = [
            f32::NAN,
            f32::INFINITY,
            -f32::INFINITY,
            0.5,
            0.0,
            0.0,
            0.0,
            0.0,
        ];
        let header = AudioBlockHeader {
            start_frame: 0,
            frames: 4,
            channels: 2,
            sequence: 0,
            flags: BlockFlags::default(),
        };

        prod.push_block(AudioBlock {
            header,
            samples: &samples,
        })
        .unwrap();

        let mut out = [1.0; 8];
        cons.pop_block(&mut out).unwrap();

        assert_eq!(out[0], 0.0);
        assert_eq!(out[1], 0.0);
        assert_eq!(out[2], 0.0);
        assert_eq!(out[3], 0.5);
    }

    #[test]
    fn empty_pop_counts_underrun_and_silences_output() {
        let (_, mut cons) = audio_block_ring(config());
        let mut out = [1.0; 8];

        assert_eq!(cons.pop_expected_or_silence(0, &mut out), None);
        assert_eq!(cons.underruns(), 1);
        assert_eq!(out, [0.0; 8]);
    }
}
