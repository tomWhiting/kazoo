//! Mixer engine for `kazoo-mix`.
//!
//! The engine is designed to be owned by the CPAL output callback. All storage is
//! allocated before the stream starts; rendering performs fixed work over the
//! configured channel slots and never allocates, locks, blocks, or performs I/O.

use kazoo_core::Pan;
use kazoo_core::audio_transport::{AudioBlockConsumer, AudioRingPopError, PoppedAudioBlock};
use kazoo_core::protocol::ChannelId;

/// Default number of channel slots in the first mixer slice.
pub const DEFAULT_CHANNEL_SLOTS: usize = 16;

/// Maximum number of interleaved output samples rendered by one callback.
pub const MAX_CALLBACK_SAMPLES: usize = 8192;

/// Linear stereo level pair.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct StereoLevel {
    /// Left-channel value.
    pub left: f32,
    /// Right-channel value.
    pub right: f32,
}

impl StereoLevel {
    /// Zeroed stereo level.
    pub const ZERO: Self = Self {
        left: 0.0,
        right: 0.0,
    };
}

/// Channel-strip controls applied during mixing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChannelControls {
    /// Linear pre-fader gain.
    pub gain: f32,
    /// Stereo pan position.
    pub pan: Pan,
    /// Hard mute.
    pub muted: bool,
    /// Solo state.
    pub soloed: bool,
}

impl Default for ChannelControls {
    fn default() -> Self {
        Self {
            gain: 1.0,
            pan: Pan::CENTER,
            muted: false,
            soloed: false,
        }
    }
}

/// Mutable mixer engine owned by the audio callback.
#[derive(Debug)]
pub struct MixerEngine {
    channels: Vec<ChannelStrip>,
    next_frame: u64,
    master_peak: StereoLevel,
    master_rms: StereoLevel,
    master_gain: f32,
}

impl MixerEngine {
    /// Create a new engine with fixed channel slots.
    #[must_use]
    pub fn new(channel_slots: usize) -> Self {
        let mut channels = Vec::with_capacity(channel_slots);
        for idx in 0..channel_slots {
            channels.push(ChannelStrip::empty(ChannelId(
                u16::try_from(idx).unwrap_or(u16::MAX),
            )));
        }

        Self {
            channels,
            next_frame: 0,
            master_peak: StereoLevel::ZERO,
            master_rms: StereoLevel::ZERO,
            master_gain: 1.0,
        }
    }

    /// Number of channel slots.
    #[must_use]
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    /// Last rendered master peak in normalized sample units.
    #[must_use]
    pub const fn master_peak(&self) -> StereoLevel {
        self.master_peak
    }

    /// Last rendered master RMS in normalized sample units.
    #[must_use]
    pub const fn master_rms(&self) -> StereoLevel {
        self.master_rms
    }

    /// Current absolute output frame.
    #[must_use]
    pub const fn next_frame(&self) -> u64 {
        self.next_frame
    }

    /// Immutable channel snapshots for UI/status code.
    #[must_use]
    pub fn channel_snapshots(&self) -> Vec<ChannelSnapshot> {
        self.channels.iter().map(ChannelStrip::snapshot).collect()
    }

    /// Copy channel snapshots into a caller-provided fixed buffer.
    ///
    /// Returns the number of snapshots written. This is suitable for callback
    /// status mirroring because it does not allocate.
    pub fn copy_channel_snapshots(&self, output: &mut [ChannelSnapshot]) -> usize {
        let count = output.len().min(self.channels.len());
        for (target, channel) in output.iter_mut().zip(self.channels.iter()).take(count) {
            *target = channel.snapshot();
        }
        count
    }

    /// Attach an audio block consumer to a channel slot.
    ///
    /// This is intended to be called before the audio stream starts in the first
    /// implementation slice. Dynamic attachment will be mediated by a scheduler
    /// thread later, not by mutating callback state from the UI thread.
    pub fn attach_consumer(
        &mut self,
        slot: usize,
        name: impl Into<String>,
        consumer: AudioBlockConsumer,
    ) -> Result<(), MixerEngineError> {
        let Some(channel) = self.channels.get_mut(slot) else {
            return Err(MixerEngineError::InvalidSlot { slot });
        };

        channel.attach(name.into(), consumer);
        Ok(())
    }

    /// Update channel-strip controls for a slot.
    pub fn configure_channel(
        &mut self,
        slot: usize,
        controls: ChannelControls,
    ) -> Result<(), MixerEngineError> {
        let Some(channel) = self.channels.get_mut(slot) else {
            return Err(MixerEngineError::InvalidSlot { slot });
        };

        channel.controls = controls;
        Ok(())
    }

    /// Set final master gain.
    pub fn set_master_gain(&mut self, gain: f32) {
        self.master_gain = gain.clamp(0.0, 4.0);
    }

    /// Render one output callback into an interleaved `f32` buffer.
    pub fn render_f32(&mut self, output: &mut [f32], output_channels: usize) {
        let output_channels = output_channels.max(1);
        let frames = output.len() / output_channels;
        let render_len = frames * output_channels;
        output[..render_len].fill(0.0);

        if render_len == 0 {
            self.master_peak = StereoLevel::ZERO;
            self.master_rms = StereoLevel::ZERO;
            return;
        }

        let any_solo = self.channels.iter().any(ChannelStrip::soloed);

        for channel in &mut self.channels {
            channel.render_into(
                self.next_frame,
                frames,
                output_channels,
                &mut output[..render_len],
                any_solo,
            );
        }

        let mut peak = StereoLevel::ZERO;
        let mut sum_sq = StereoLevel::ZERO;
        for frame_idx in 0..frames {
            let base = frame_idx * output_channels;
            let left = kazoo_core::soft_limit(output[base] * self.master_gain);
            output[base] = left;
            peak.left = peak.left.max(left.abs());
            sum_sq.left += left * left;

            let right = if output_channels > 1 {
                let limited = kazoo_core::soft_limit(output[base + 1] * self.master_gain);
                output[base + 1] = limited;
                limited
            } else {
                left
            };
            peak.right = peak.right.max(right.abs());
            sum_sq.right += right * right;

            for channel in 2..output_channels {
                output[base + channel] =
                    kazoo_core::soft_limit(output[base + channel] * self.master_gain);
            }
        }

        self.master_peak = peak;
        self.master_rms = StereoLevel {
            left: (sum_sq.left / frames as f32).sqrt(),
            right: (sum_sq.right / frames as f32).sqrt(),
        };
        self.next_frame = self.next_frame.wrapping_add(frames as u64);
    }
}

/// Error returned by mixer engine setup operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixerEngineError {
    /// Requested channel slot does not exist.
    InvalidSlot {
        /// Requested slot index.
        slot: usize,
    },
}

#[derive(Debug)]
struct ChannelStrip {
    id: ChannelId,
    name: String,
    consumer: Option<AudioBlockConsumer>,
    controls: ChannelControls,
    peak: StereoLevel,
    rms: StereoLevel,
    underruns: u64,
    sequence_gaps: u64,
    buffered_block: Option<PoppedAudioBlock>,
    buffered_offset_frames: usize,
    buffered_samples: Vec<f32>,
}

impl ChannelStrip {
    fn empty(id: ChannelId) -> Self {
        Self {
            id,
            name: "empty".to_string(),
            consumer: None,
            controls: ChannelControls::default(),
            peak: StereoLevel::ZERO,
            rms: StereoLevel::ZERO,
            underruns: 0,
            sequence_gaps: 0,
            buffered_block: None,
            buffered_offset_frames: 0,
            buffered_samples: Vec::new(),
        }
    }

    fn attach(&mut self, name: String, consumer: AudioBlockConsumer) {
        let samples_per_block = consumer.config().samples_per_block();
        self.name = name;
        self.consumer = Some(consumer);
        self.peak = StereoLevel::ZERO;
        self.rms = StereoLevel::ZERO;
        self.underruns = 0;
        self.sequence_gaps = 0;
        self.buffered_block = None;
        self.buffered_offset_frames = 0;
        self.buffered_samples.resize(samples_per_block, 0.0);
    }

    fn snapshot(&self) -> ChannelSnapshot {
        ChannelSnapshot {
            id: self.id,
            connected: self.consumer.is_some(),
            name: short_name(&self.name),
            peak: self.peak,
            rms: self.rms,
            gain: self.controls.gain,
            pan: self.controls.pan.value(),
            muted: self.controls.muted,
            soloed: self.controls.soloed,
            underruns: self.underruns,
            sequence_gaps: self.sequence_gaps,
        }
    }

    const fn soloed(&self) -> bool {
        self.controls.soloed
    }

    fn render_into(
        &mut self,
        start_frame: u64,
        frames: usize,
        output_channels: usize,
        output: &mut [f32],
        any_solo: bool,
    ) {
        if self.consumer.is_none() {
            self.peak = StereoLevel::ZERO;
            self.rms = StereoLevel::ZERO;
            return;
        }

        if self.controls.muted || (any_solo && !self.controls.soloed) {
            self.refresh_consumer_stats();
            self.peak = StereoLevel::ZERO;
            self.rms = StereoLevel::ZERO;
            return;
        }

        let mut frames_mixed = 0;
        let mut peak = StereoLevel::ZERO;
        let mut sum_sq = StereoLevel::ZERO;

        while frames_mixed < frames {
            if self.buffered_block.is_none() {
                let expected_frame = start_frame.wrapping_add(frames_mixed as u64);
                let pop_result = self
                    .consumer
                    .as_mut()
                    .expect("consumer checked above")
                    .pop_block(&mut self.buffered_samples);
                match pop_result {
                    Ok(block) if block.header.start_frame == expected_frame => {
                        self.buffered_block = Some(block);
                        self.buffered_offset_frames = 0;
                    }
                    Ok(_block) => {
                        self.sequence_gaps = self.sequence_gaps.wrapping_add(1);
                        self.buffered_block = None;
                        self.buffered_offset_frames = 0;
                        break;
                    }
                    Err(AudioRingPopError::Empty | AudioRingPopError::OutputTooSmall { .. }) => {
                        self.refresh_consumer_stats();
                        break;
                    }
                }
            }

            let Some(block) = self.buffered_block else {
                break;
            };

            let mixed =
                self.mix_buffered_block(frames_mixed, frames, output_channels, output, block);
            peak = max_level(peak, mixed.peak);
            sum_sq.left += mixed.sum_sq.left;
            sum_sq.right += mixed.sum_sq.right;
            frames_mixed += mixed.frames;
            self.buffered_offset_frames += mixed.frames;

            if self.buffered_offset_frames >= block.header.frames as usize {
                self.buffered_block = None;
                self.buffered_offset_frames = 0;
            }
        }

        self.refresh_consumer_stats();
        self.peak = peak;
        self.rms = if frames > 0 {
            StereoLevel {
                left: (sum_sq.left / frames as f32).sqrt(),
                right: (sum_sq.right / frames as f32).sqrt(),
            }
        } else {
            StereoLevel::ZERO
        };
    }

    fn mix_buffered_block(
        &self,
        output_frame_offset: usize,
        total_output_frames: usize,
        output_channels: usize,
        output: &mut [f32],
        block: PoppedAudioBlock,
    ) -> MixResult {
        let input_channels = usize::from(block.header.channels.max(1));
        let available_frames = block.header.frames as usize - self.buffered_offset_frames;
        let frames_to_mix = available_frames.min(total_output_frames - output_frame_offset);
        let (left_gain, right_gain) = self.controls.pan.gains();
        let mut peak = StereoLevel::ZERO;
        let mut sum_sq = StereoLevel::ZERO;

        for frame_idx in 0..frames_to_mix {
            let input_frame = self.buffered_offset_frames + frame_idx;
            let output_frame = output_frame_offset + frame_idx;
            let in_base = input_frame * input_channels;
            let out_base = output_frame * output_channels;
            let left = self.buffered_samples.get(in_base).copied().unwrap_or(0.0);
            let right = if input_channels > 1 {
                self.buffered_samples
                    .get(in_base + 1)
                    .copied()
                    .unwrap_or(left)
            } else {
                left
            };

            let mono = (left + right) * 0.5 * self.controls.gain;
            let out_left = mono * left_gain;
            let out_right = mono * right_gain;

            if output_channels == 1 {
                output[out_base] += (out_left + out_right) * 0.5;
                peak.left = peak.left.max(output[out_base].abs());
                peak.right = peak.left;
                sum_sq.left += output[out_base] * output[out_base];
                sum_sq.right = sum_sq.left;
            } else {
                output[out_base] += out_left;
                output[out_base + 1] += out_right;
                peak.left = peak.left.max(output[out_base].abs());
                peak.right = peak.right.max(output[out_base + 1].abs());
                sum_sq.left += output[out_base] * output[out_base];
                sum_sq.right += output[out_base + 1] * output[out_base + 1];
                for channel in 2..output_channels {
                    output[out_base + channel] += mono;
                }
            }
        }

        MixResult {
            frames: frames_to_mix,
            peak,
            sum_sq,
        }
    }

    fn refresh_consumer_stats(&mut self) {
        if let Some(consumer) = self.consumer.as_ref() {
            self.underruns = consumer.underruns();
            self.sequence_gaps = self.sequence_gaps.max(consumer.sequence_gaps());
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct MixResult {
    frames: usize,
    peak: StereoLevel,
    sum_sq: StereoLevel,
}

const fn max_level(a: StereoLevel, b: StereoLevel) -> StereoLevel {
    StereoLevel {
        left: if a.left > b.left { a.left } else { b.left },
        right: if a.right > b.right { a.right } else { b.right },
    }
}

fn short_name(name: &str) -> [u8; 12] {
    let mut out = [0_u8; 12];
    for (idx, byte) in name.as_bytes().iter().copied().take(out.len()).enumerate() {
        out[idx] = byte;
    }
    out
}

/// UI-safe snapshot of a channel strip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChannelSnapshot {
    /// Channel id.
    pub id: ChannelId,
    /// Whether an audio consumer is attached.
    pub connected: bool,
    /// Fixed-size short name buffer.
    pub name: [u8; 12],
    /// Last rendered peak.
    pub peak: StereoLevel,
    /// Last rendered RMS.
    pub rms: StereoLevel,
    /// Current linear gain.
    pub gain: f32,
    /// Current pan.
    pub pan: f32,
    /// Mute state.
    pub muted: bool,
    /// Solo state.
    pub soloed: bool,
    /// Missing-block count observed by this channel.
    pub underruns: u64,
    /// Sequence discontinuities observed by this channel.
    pub sequence_gaps: u64,
}

impl ChannelSnapshot {
    /// Empty disconnected snapshot for fixed-size status buffers.
    pub const EMPTY: Self = Self {
        id: ChannelId(0),
        connected: false,
        name: [0; 12],
        peak: StereoLevel::ZERO,
        rms: StereoLevel::ZERO,
        gain: 1.0,
        pan: 0.0,
        muted: false,
        soloed: false,
        underruns: 0,
        sequence_gaps: 0,
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use kazoo_core::audio_transport::{AudioBlock, AudioRingConfig, audio_block_ring};
    use kazoo_core::protocol::{AudioBlockHeader, BlockFlags, BufferId};

    #[test]
    fn empty_engine_renders_silence_and_advances_frame() {
        let mut engine = MixerEngine::new(2);
        let mut output = [1.0; 16];

        engine.render_f32(&mut output, 2);

        assert_eq!(output, [0.0; 16]);
        assert_eq!(engine.next_frame(), 8);
        assert_eq!(engine.master_peak(), StereoLevel::ZERO);
    }

    #[test]
    fn attached_channel_mixes_expected_block() {
        let config = AudioRingConfig::new(BufferId(1), 2, 4, 2);
        let (mut producer, consumer) = audio_block_ring(config);
        let samples = [0.5; 8];
        producer
            .push_block(AudioBlock {
                header: AudioBlockHeader {
                    start_frame: 0,
                    frames: 4,
                    channels: 2,
                    sequence: 0,
                    flags: BlockFlags::default(),
                },
                samples: &samples,
            })
            .unwrap();

        let mut engine = MixerEngine::new(1);
        engine.attach_consumer(0, "test", consumer).unwrap();

        let mut output = [0.0; 8];
        engine.render_f32(&mut output, 2);

        assert!(output.iter().any(|sample| sample.abs() > 0.0));
        assert_eq!(engine.next_frame(), 4);
    }

    #[test]
    fn source_block_can_span_multiple_callback_renders() {
        let config = AudioRingConfig::new(BufferId(1), 2, 4, 2);
        let (mut producer, consumer) = audio_block_ring(config);
        let samples = [0.25; 8];
        producer
            .push_block(AudioBlock {
                header: AudioBlockHeader {
                    start_frame: 0,
                    frames: 4,
                    channels: 2,
                    sequence: 0,
                    flags: BlockFlags::default(),
                },
                samples: &samples,
            })
            .unwrap();

        let mut engine = MixerEngine::new(1);
        engine.attach_consumer(0, "test", consumer).unwrap();

        let mut first = [0.0; 4];
        engine.render_f32(&mut first, 2);
        assert!(first.iter().any(|sample| sample.abs() > 0.0));
        assert_eq!(engine.next_frame(), 2);

        let mut second = [0.0; 4];
        engine.render_f32(&mut second, 2);
        assert!(second.iter().any(|sample| sample.abs() > 0.0));
        assert_eq!(engine.next_frame(), 4);
        assert_eq!(engine.channel_snapshots()[0].underruns, 0);
    }

    #[test]
    fn missing_block_renders_silence_and_counts_underrun() {
        let config = AudioRingConfig::new(BufferId(1), 2, 4, 2);
        let (_producer, consumer) = audio_block_ring(config);
        let mut engine = MixerEngine::new(1);
        engine.attach_consumer(0, "test", consumer).unwrap();

        let mut output = [1.0; 8];
        engine.render_f32(&mut output, 2);
        let snapshot = engine.channel_snapshots()[0];

        assert_eq!(output, [0.0; 8]);
        assert_eq!(snapshot.underruns, 1);
    }

    #[test]
    fn muted_channel_does_not_mix() {
        let config = AudioRingConfig::new(BufferId(1), 2, 4, 2);
        let (mut producer, consumer) = audio_block_ring(config);
        let samples = [0.75; 8];
        producer
            .push_block(AudioBlock {
                header: AudioBlockHeader {
                    start_frame: 0,
                    frames: 4,
                    channels: 2,
                    sequence: 0,
                    flags: BlockFlags::default(),
                },
                samples: &samples,
            })
            .unwrap();

        let mut engine = MixerEngine::new(1);
        engine.attach_consumer(0, "test", consumer).unwrap();
        engine
            .configure_channel(
                0,
                ChannelControls {
                    muted: true,
                    ..ChannelControls::default()
                },
            )
            .unwrap();

        let mut output = [0.0; 8];
        engine.render_f32(&mut output, 2);

        assert_eq!(output, [0.0; 8]);
    }

    #[test]
    fn hard_pan_biases_energy_to_one_side() {
        let config = AudioRingConfig::new(BufferId(1), 2, 4, 2);
        let (mut producer, consumer) = audio_block_ring(config);
        let samples = [0.5; 8];
        producer
            .push_block(AudioBlock {
                header: AudioBlockHeader {
                    start_frame: 0,
                    frames: 4,
                    channels: 2,
                    sequence: 0,
                    flags: BlockFlags::default(),
                },
                samples: &samples,
            })
            .unwrap();

        let mut engine = MixerEngine::new(1);
        engine.attach_consumer(0, "test", consumer).unwrap();
        engine
            .configure_channel(
                0,
                ChannelControls {
                    pan: Pan::new(-1.0),
                    ..ChannelControls::default()
                },
            )
            .unwrap();

        let mut output = [0.0; 8];
        engine.render_f32(&mut output, 2);

        let left: f32 = output.iter().step_by(2).map(|s| s.abs()).sum();
        let right: f32 = output.iter().skip(1).step_by(2).map(|s| s.abs()).sum();
        assert!(left > right);
    }
}
