//! Built-in non-callback audio sources for exercising the mixer transport.
//!
//! These are not the final studio client model. They are local producers that
//! feed the same `AudioBlockProducer` path future clients will use, making the
//! first `kazoo-mix` binary audibly test the real block-ring architecture.

use std::f32::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use kazoo_808::sequencer::Sequencer;
use kazoo_808::synth::{DrumMachine, VoiceIndex};
use kazoo_core::audio_transport::{AudioBlock, AudioBlockProducer, AudioRingPushError};
use kazoo_core::protocol::{AudioBlockHeader, BlockFlags};

/// Handle for a background procedural 808 producer.
#[derive(Debug)]
pub struct EightOhEightSource {
    running: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}

impl EightOhEightSource {
    /// Start a simple internal 808 pattern feeding frame-indexed blocks into the
    /// provided producer.
    #[must_use]
    pub fn start(
        mut producer: AudioBlockProducer,
        sample_rate: u32,
        channels: u16,
        block_frames: u32,
    ) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let thread_running = Arc::clone(&running);
        let join = thread::Builder::new()
            .name("kazoo-mix-808-source".to_string())
            .spawn(move || {
                run_808_source(
                    &mut producer,
                    &thread_running,
                    sample_rate.max(1),
                    channels.max(1),
                    block_frames.max(1),
                );
            })
            .ok();

        Self { running, join }
    }
}

impl Drop for EightOhEightSource {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn run_808_source(
    producer: &mut AudioBlockProducer,
    running: &AtomicBool,
    sample_rate: u32,
    channels: u16,
    block_frames: u32,
) {
    let sample_rate_f32 = sample_rate as f32;
    let channels_usize = usize::from(channels);
    let block_samples = block_frames as usize * channels_usize;
    let mut block = vec![0.0_f32; block_samples];
    let mut drum_machine = DrumMachine::new(sample_rate_f32);
    let mut sequencer = Sequencer::new(sample_rate_f32);
    program_default_808_pattern(&mut sequencer);
    sequencer.clock.set_bpm(122.0);
    sequencer.play();

    let mut start_frame = 0_u64;
    let block_duration = Duration::from_secs_f64(f64::from(block_frames) / f64::from(sample_rate));

    while running.load(Ordering::Acquire) {
        for frame in 0..block_frames as usize {
            let _ = sequencer.tick(&mut drum_machine);
            let sample = kazoo_core::soft_limit(drum_machine.process() * 0.7);
            let base = frame * channels_usize;
            for channel in 0..channels_usize {
                block[base + channel] = sample;
            }
        }

        let header = AudioBlockHeader {
            start_frame,
            frames: block_frames,
            channels,
            sequence: 0,
            flags: BlockFlags {
                silent: false,
                loop_wrapped: false,
                final_segment: true,
            },
        };

        match producer.push_block(AudioBlock {
            header,
            samples: &block,
        }) {
            Ok(()) => {
                start_frame = start_frame.wrapping_add(u64::from(block_frames));
                thread::sleep(block_duration / 2);
            }
            Err(AudioRingPushError::Full) => thread::sleep(block_duration / 2),
            Err(AudioRingPushError::InvalidSampleCount { .. }) => break,
        }
    }
}

fn program_default_808_pattern(sequencer: &mut Sequencer) {
    for step in [0, 4, 8, 12] {
        sequencer.toggle_step(VoiceIndex::Kick as usize, step);
    }
    for step in [4, 12] {
        sequencer.toggle_step(VoiceIndex::Snare as usize, step);
        sequencer.toggle_accent(VoiceIndex::Snare as usize, step);
    }
    for step in (0..16).step_by(2) {
        sequencer.toggle_step(VoiceIndex::ClosedHiHat as usize, step);
        sequencer.set_step_velocity(VoiceIndex::ClosedHiHat as usize, step, 0.45);
    }
    for step in [2, 10] {
        sequencer.toggle_step(VoiceIndex::OpenHiHat as usize, step);
        sequencer.set_step_velocity(VoiceIndex::OpenHiHat as usize, step, 0.35);
    }
    let step = 15;
    sequencer.toggle_step(VoiceIndex::Clap as usize, step);
    sequencer.set_step_velocity(VoiceIndex::Clap as usize, step, 0.35);
}

/// Handle for a background test tone producer.
#[derive(Debug)]
pub struct TestToneSource {
    running: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}

impl TestToneSource {
    /// Start a low-level sine source that feeds frame-indexed blocks into the
    /// provided producer.
    #[must_use]
    pub fn start(
        mut producer: AudioBlockProducer,
        sample_rate: u32,
        channels: u16,
        block_frames: u32,
    ) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let thread_running = Arc::clone(&running);
        let join = thread::Builder::new()
            .name("kazoo-mix-test-tone".to_string())
            .spawn(move || {
                run_test_tone(
                    &mut producer,
                    &thread_running,
                    sample_rate.max(1),
                    channels.max(1),
                    block_frames.max(1),
                );
            })
            .ok();

        Self { running, join }
    }
}

impl Drop for TestToneSource {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn run_test_tone(
    producer: &mut AudioBlockProducer,
    running: &AtomicBool,
    sample_rate: u32,
    channels: u16,
    block_frames: u32,
) {
    let channels_usize = usize::from(channels);
    let block_samples = block_frames as usize * channels_usize;
    let mut block = vec![0.0_f32; block_samples];
    let mut phase = 0.0_f32;
    let phase_inc = TAU * 220.0 / sample_rate as f32;
    let mut start_frame = 0_u64;
    let block_duration = Duration::from_secs_f64(f64::from(block_frames) / f64::from(sample_rate));

    while running.load(Ordering::Acquire) {
        for frame in 0..block_frames as usize {
            let sample = phase.sin() * 0.08;
            phase = (phase + phase_inc).rem_euclid(TAU);
            let base = frame * channels_usize;
            for channel in 0..channels_usize {
                block[base + channel] = sample;
            }
        }

        let header = AudioBlockHeader {
            start_frame,
            frames: block_frames,
            channels,
            sequence: 0,
            flags: BlockFlags {
                silent: false,
                loop_wrapped: false,
                final_segment: true,
            },
        };

        match producer.push_block(AudioBlock {
            header,
            samples: &block,
        }) {
            Ok(()) => {
                start_frame = start_frame.wrapping_add(u64::from(block_frames));
                thread::sleep(block_duration / 2);
            }
            Err(AudioRingPushError::Full) => {
                thread::sleep(block_duration / 2);
            }
            Err(AudioRingPushError::InvalidSampleCount { .. }) => break,
        }
    }
}
