//! Built-in audio sources.
//!
//! [`DemoEightOhEight`] is a built-in 808 pattern that plugs into the desk
//! exactly as an instrument does: through a hub link to the desk's own
//! socket, following the song to the frame. [`TestToneSource`] feeds a sine
//! straight into a block ring, for exercising the engine without a hub.

use std::f32::consts::TAU;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use kazoo_808::sequencer::Sequencer;
use kazoo_808::synth::{DrumMachine, VoiceIndex};
use kazoo_core::audio_transport::{AudioBlock, AudioBlockProducer, AudioRingPushError};
use kazoo_core::ipc::client::HubMessage;
use kazoo_core::ipc::follow::TransportFollower;
use kazoo_core::ipc::link::{HubAddress, HubLink, HubLinkAudio, LinkConfig, LinkStatus, hub_link};
use kazoo_core::protocol::{AudioBlockHeader, BlockFlags};

use crate::shared::SharedState;
use crate::studio_clock::{DeskClock, DeskTimer};
use crate::worker::join_worker;

/// How long the demo waits while it is a lead ahead of the desk.
const PACE_INTERVAL: Duration = Duration::from_millis(1);

/// Frames per block the built-in sources render.
pub const DEMO_BLOCK_FRAMES: u32 = 256;

/// Name the demo shows on its strip.
pub const DEMO_NAME: &str = "808 demo";

/// The built-in 808 pattern, playing through the desk like any instrument.
#[derive(Debug)]
pub struct DemoEightOhEight {
    link: HubLink,
    running: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}

impl DemoEightOhEight {
    /// Plug the demo into the hub on `socket`. It renders a lead ahead of
    /// the desk's clock in `shared`, and plays while the song plays.
    ///
    /// # Errors
    ///
    /// Fails if the link or the render thread cannot be started.
    pub fn start(socket: PathBuf, sample_rate: u32, shared: Arc<SharedState>) -> io::Result<Self> {
        let config = LinkConfig {
            address: HubAddress::Socket(socket),
            ..LinkConfig::new(DEMO_NAME, 2, sample_rate, DEMO_BLOCK_FRAMES)
        };
        let (link, audio) = hub_link(config)?;
        let running = Arc::new(AtomicBool::new(true));
        let thread_running = Arc::clone(&running);
        let join = thread::Builder::new()
            .name("kazoo-mix-808-demo".to_string())
            .spawn(move || {
                let mut demo = DemoRender::new(audio, sample_rate, shared);
                demo.run(&thread_running);
            })?;
        Ok(Self {
            link,
            running,
            join: Some(join),
        })
    }

    /// The demo's link to the desk.
    #[must_use]
    pub fn status(&self) -> LinkStatus {
        self.link.status()
    }
}

impl Drop for DemoEightOhEight {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(join) = self.join.take() {
            join_worker("808 demo", join);
        }
    }
}

/// The demo's render thread.
struct DemoRender {
    audio: HubLinkAudio,
    follower: TransportFollower,
    timer: DeskTimer,
    shared: Arc<SharedState>,
    drums: DrumMachine,
    sequencer: Sequencer,
    block: Vec<f32>,
    /// Stream and studio frames when pacing last started, so the demo stays
    /// a lead ahead of the desk.
    base: Option<(u64, u64)>,
}

impl DemoRender {
    fn new(audio: HubLinkAudio, sample_rate: u32, shared: Arc<SharedState>) -> Self {
        // Sample rates are whole numbers of Hz, far inside f32.
        let rate = sample_rate as f32;
        let mut sequencer = Sequencer::new(rate);
        program_default_808_pattern(&mut sequencer);
        Self {
            audio,
            follower: TransportFollower::new(sample_rate),
            timer: DeskTimer::new(sample_rate),
            shared,
            drums: DrumMachine::new(rate),
            sequencer,
            block: vec![0.0; DEMO_BLOCK_FRAMES as usize * 2],
            base: None,
        }
    }

    fn run(&mut self, running: &AtomicBool) {
        let mut was_connected = false;
        while running.load(Ordering::Acquire) {
            let connected = self.audio.is_connected();
            if connected != was_connected {
                // Joining (or losing) the desk: pace afresh from here.
                self.base = None;
                was_connected = connected;
            }
            self.drain_hub();
            let clock = self.timer.read(&self.shared);
            if self.far_enough_ahead(clock) {
                thread::sleep(PACE_INTERVAL);
                continue;
            }
            self.render_block();
        }
    }

    /// Whether the demo has rendered a lead ahead of the frame the desk is
    /// playing: one device buffer, one block and the margin, like the hub
    /// gives an instrument.
    fn far_enough_ahead(&mut self, clock: DeskClock) -> bool {
        let stream = self.audio.stream_frame();
        let (base_stream, base_studio) = *self
            .base
            .get_or_insert_with(|| (stream, clock.playing_now()));
        let rendered = stream.wrapping_sub(base_stream);
        let played = clock.playing_now().saturating_sub(base_studio);
        let lead = u64::from(clock.device_frames)
            + u64::from(DEMO_BLOCK_FRAMES)
            + u64::from(clock.margin_frames);
        rendered >= played.saturating_add(lead)
    }

    fn drain_hub(&mut self) {
        while let Some(message) = self.audio.try_recv() {
            match message {
                HubMessage::TransportSync(sync) => {
                    if self.follower.schedule(&sync).is_err() {
                        // The desk's own hub never sends more than the
                        // follower holds, nor anything invalid.
                        self.shared.note_engine_fault();
                    }
                }
                // The demo takes no notes or parameters; the link handles
                // the hub closing.
                HubMessage::NoteEvent(_)
                | HubMessage::ParameterChange(_)
                | HubMessage::Shutdown => {}
            }
        }
    }

    fn render_block(&mut self) {
        let first = self.audio.stream_frame();
        for frame in 0..DEMO_BLOCK_FRAMES as usize {
            if let Some(change) = self.follower.due(first + frame as u64) {
                self.sequencer.clock.set_bpm(change.bpm);
                match change.beat {
                    Some(beat) => self.sequencer.play_from(beat),
                    None => self.sequencer.stop(),
                }
            }
            self.sequencer.tick(&mut self.drums);
            let sample = kazoo_core::soft_limit(self.drums.process() * 0.7);
            self.block[frame * 2] = sample;
            self.block[frame * 2 + 1] = sample;
        }
        // Whether the desk took it or not, the block is rendered: the
        // stream position moves on either way.
        self.audio.send_audio(DEMO_BLOCK_FRAMES, &self.block);
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
    ///
    /// # Errors
    ///
    /// Fails if the source thread cannot be started.
    pub fn start(
        mut producer: AudioBlockProducer,
        sample_rate: u32,
        block_frames: u32,
    ) -> io::Result<Self> {
        let running = Arc::new(AtomicBool::new(true));
        let thread_running = Arc::clone(&running);
        let join = thread::Builder::new()
            .name("kazoo-mix-test-tone".to_string())
            .spawn(move || {
                run_test_tone(
                    &mut producer,
                    &thread_running,
                    sample_rate.max(1),
                    block_frames.max(1),
                );
            })?;

        Ok(Self {
            running,
            join: Some(join),
        })
    }
}

impl Drop for TestToneSource {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(join) = self.join.take() {
            join_worker("test tone", join);
        }
    }
}

fn run_test_tone(
    producer: &mut AudioBlockProducer,
    running: &AtomicBool,
    sample_rate: u32,
    block_frames: u32,
) {
    let channels = producer.config().channels;
    let channels_usize = usize::from(channels);
    let block_samples = block_frames as usize * channels_usize;
    let mut block = vec![0.0_f32; block_samples];
    let mut phase = 0.0_f32;
    let phase_inc = TAU * 220.0 / sample_rate as f32;
    let mut start_frame = 0_u64;
    let block_duration = Duration::from_secs_f64(f64::from(block_frames) / f64::from(sample_rate));
    // Keep a refused block and offer it again rather than skipping audio.
    let mut pending = false;

    while running.load(Ordering::Acquire) {
        if !pending {
            for frame in 0..block_frames as usize {
                let sample = phase.sin() * 0.08;
                phase = (phase + phase_inc).rem_euclid(TAU);
                let base = frame * channels_usize;
                for channel in 0..channels_usize {
                    block[base + channel] = sample;
                }
            }
            pending = true;
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
                pending = false;
            }
            Err(AudioRingPushError::Full) => {
                thread::sleep(block_duration / 2);
            }
            Err(err @ AudioRingPushError::InvalidSampleCount { .. }) => {
                unreachable!("blocks are sized from the ring's own channel count: {err:?}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kazoo_core::audio_transport::{AudioRingConfig, audio_block_ring};
    use kazoo_core::protocol::BufferId;
    use std::time::Instant;

    /// A full ring must not make the source skip audio: the tone read back
    /// is phase-continuous across every block boundary, and frames are
    /// contiguous.
    #[test]
    fn source_never_drops_audio_when_the_ring_is_full() {
        let block_frames = 64;
        let sample_rate = 8_000;
        let (producer, mut consumer) =
            audio_block_ring(AudioRingConfig::new(BufferId(1), 2, block_frames, 2));
        let source = TestToneSource::start(producer, sample_rate, block_frames);

        let mut samples = vec![0.0_f32; block_frames as usize * 2];
        let mut previous: Option<f32> = None;
        let mut expected_frame = 0_u64;
        let mut blocks = 0;
        let deadline = Instant::now() + Duration::from_secs(5);
        // Read slowly so the ring is full most of the time.
        while blocks < 40 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(3));
            let Ok(block) = consumer.pop_block(&mut samples) else {
                continue;
            };
            assert_eq!(block.header.start_frame, expected_frame);
            expected_frame += u64::from(block_frames);
            let first = samples[0];
            if let Some(last) = previous {
                // 220 Hz at 8 kHz and 0.08 amplitude moves at most ~0.014
                // per sample; a skipped block would jump much further.
                assert!(
                    (first - last).abs() < 0.02,
                    "discontinuity {last} -> {first}"
                );
            }
            previous = Some(samples[(block_frames as usize - 1) * 2]);
            blocks += 1;
        }
        drop(source);
        assert_eq!(blocks, 40, "source stalled");
    }
}
