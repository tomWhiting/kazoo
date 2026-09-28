//! The body of the audio output callback.
//!
//! [`AudioCallback::process`] is everything the device callback does: patch in
//! or pull out sources the hub has queued, pull the desk's controls from
//! shared state, render the engine into the device buffer
//! (in as many chunks as it takes, inside one meter window), and publish
//! meters, clip lights and health back to the desk. It lives here, not in
//! `main`, so it can be tested without an audio device.
//!
//! Each callback's meters are folded into the shared meters, which the desk
//! drains once per frame (see `shared`), so no transient is lost between two
//! desk frames.
//!
//! Real-time rules hold throughout: all storage is allocated in
//! [`AudioCallback::new`]; `process` never allocates, frees, locks, blocks or
//! performs I/O.

use std::sync::Arc;

use crate::engine::{ChannelSnapshot, MAX_RENDER_FRAMES, MixerEngine};
use crate::patchbay::PatchbayCallback;
use crate::shared::{DESK_CHANNELS, SharedState};

/// Frames rendered per engine call by default. Device buffers larger than this
/// are rendered in several calls within one meter window.
pub const CALLBACK_RENDER_FRAMES: usize = MAX_RENDER_FRAMES * 4;

/// Everything the output callback owns.
#[derive(Debug)]
pub struct AudioCallback {
    engine: MixerEngine,
    shared: Arc<SharedState>,
    channels: usize,
    mix: Vec<f32>,
    snapshots: [ChannelSnapshot; DESK_CHANNELS],
    patchbay: Option<PatchbayCallback>,
    /// Transport revision last scheduled.
    followed: Option<u64>,
}

impl AudioCallback {
    /// Prepare a callback for a device with `channels` output channels.
    #[must_use]
    pub fn new(engine: MixerEngine, shared: Arc<SharedState>, channels: u16) -> Self {
        Self::with_render_frames(engine, shared, channels, CALLBACK_RENDER_FRAMES)
    }

    /// As [`Self::new`], rendering at most `render_frames` frames per engine
    /// call.
    #[must_use]
    pub fn with_render_frames(
        engine: MixerEngine,
        shared: Arc<SharedState>,
        channels: u16,
        render_frames: usize,
    ) -> Self {
        let channels = usize::from(channels.max(1));
        Self {
            engine,
            shared,
            channels,
            mix: vec![0.0; render_frames.max(1) * channels],
            snapshots: [ChannelSnapshot::EMPTY; DESK_CHANNELS],
            patchbay: None,
            followed: None,
        }
    }

    /// Take patch commands (sources plugged in and pulled out) from the hub
    /// through `patchbay`, applied at the start of every callback, and tell
    /// the hub through it about every transport change scheduled.
    #[must_use]
    pub fn with_patchbay(mut self, patchbay: PatchbayCallback) -> Self {
        self.patchbay = Some(patchbay);
        self
    }

    /// The engine, for inspection.
    #[must_use]
    pub const fn engine(&self) -> &MixerEngine {
        &self.engine
    }

    /// Schedule any transport change the desk or an instrument asked for:
    /// far enough ahead that every instrument hears of it before rendering
    /// that frame, and never inside this buffer or the next.
    fn schedule_transport(&mut self, frames: usize) {
        let transport = self.shared.transport();
        if self.followed == Some(transport.revision) {
            return;
        }
        self.followed = Some(transport.revision);
        let ahead = u64::from(self.shared.schedule_ahead()).max(2 * frames as u64);
        let anchor = self.engine.schedule_transport(
            self.engine.next_frame().wrapping_add(ahead),
            transport.playing,
            f64::from(transport.bpm),
        );
        if let Some(patchbay) = self.patchbay.as_mut() {
            if patchbay.announce(anchor).is_err() {
                // The hub is not taking changes: the instruments miss this
                // one. Counted, and shown on the desk.
                self.shared.note_engine_fault();
            }
        }
    }

    /// Render one device buffer and publish the results.
    pub fn process<T>(&mut self, data: &mut [T])
    where
        T: cpal::FromSample<f32>,
    {
        if let Some(patchbay) = self.patchbay.as_mut() {
            let report = patchbay.service(&mut self.engine);
            for _ in 0..report.stranded {
                self.shared.note_engine_fault();
            }
        }
        let slots = self.engine.channel_count().min(DESK_CHANNELS);
        for slot in 0..slots {
            if self
                .engine
                .configure_channel(slot, self.shared.channel_controls(slot))
                .is_err()
            {
                self.shared.note_engine_fault();
            }
        }
        self.engine.configure_master(self.shared.master_controls());
        self.schedule_transport(data.len() / self.channels);
        self.engine.set_click(self.shared.click());

        self.engine.begin_meter_window();
        for chunk in data.chunks_mut(self.mix.len()) {
            let rendered = &mut self.mix[..chunk.len()];
            self.engine.render_block(rendered, self.channels);
            for (target, sample) in chunk.iter_mut().zip(rendered.iter()) {
                *target = T::from_sample_(*sample);
            }
        }
        self.engine.end_meter_window();
        self.shared.set_studio_frame(self.engine.next_frame());
        self.shared.set_beat(self.engine.metronome_beat());

        let frames = data.len() / self.channels;
        if frames == 0 {
            return;
        }
        let count = self.engine.copy_channel_snapshots(&mut self.snapshots);
        for (slot, snapshot) in self.snapshots.iter().take(count).enumerate() {
            self.shared.publish_channel(slot, snapshot, frames);
        }
        self.shared.publish_master(
            self.engine.master_peak(),
            self.engine.master_rms(),
            frames,
            self.engine.master_clipped(),
        );
        self.shared
            .set_callback_frames(u32::try_from(frames).unwrap_or(u32::MAX));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::short_name;
    use kazoo_core::audio_transport::{
        AudioBlock, AudioBlockProducer, AudioRingConfig, audio_block_ring,
    };
    use kazoo_core::protocol::{AudioBlockHeader, BlockFlags, BufferId};

    fn setup(
        render_frames: usize,
        block_frames: u32,
        blocks: u32,
    ) -> (AudioCallback, AudioBlockProducer, Arc<SharedState>) {
        let (producer, consumer) =
            audio_block_ring(AudioRingConfig::new(BufferId(1), 2, block_frames, blocks));
        let mut engine = MixerEngine::new(DESK_CHANNELS, 48_000).unwrap();
        engine
            .attach_consumer(0, short_name("src"), consumer)
            .unwrap();
        let shared = Arc::new(SharedState::new());
        let callback =
            AudioCallback::with_render_frames(engine, Arc::clone(&shared), 2, render_frames);
        (callback, producer, shared)
    }

    fn push(producer: &mut AudioBlockProducer, start_frame: u64, frames: u32, value: f32) {
        let samples = vec![value; frames as usize * 2];
        producer
            .push_block(AudioBlock {
                header: AudioBlockHeader {
                    start_frame,
                    frames,
                    channels: 2,
                    sequence: 0,
                    flags: BlockFlags::default(),
                },
                samples: &samples,
            })
            .unwrap();
    }

    /// Render enough silence for every gain glide to settle.
    fn warm(callback: &mut AudioCallback) {
        let mut warm = vec![0.0_f32; 1024];
        callback.process(&mut warm);
    }

    #[test]
    fn clip_in_an_early_chunk_survives_a_multi_chunk_callback() {
        let (mut callback, mut producer, shared) = setup(4, 4, 16);
        warm(&mut callback);
        let now = callback.engine().next_frame();
        push(&mut producer, now, 4, 1.5);
        for block in 1..4 {
            push(&mut producer, now + block * 4, 4, 0.1);
        }
        let mut data = [0.0_f32; 32];
        callback.process(&mut data);
        assert!(shared.channel_readout(0).clip);
        assert!(shared.master_readout().clip);
        // The warm-up callback (512 frames) and this one (16) are both there.
        assert_eq!(shared.take_channel_meters(0).frames, 512 + 16);
        assert_eq!(shared.callback_frames(), Some(16));
    }

    #[test]
    fn a_transient_between_desk_frames_is_not_lost() {
        let (mut callback, mut producer, shared) = setup(64, 4, 16);
        warm(&mut callback);
        // Start from drained meters: the warm-up callback published silence.
        assert_eq!(shared.take_channel_meters(0).frames, 512);
        assert_eq!(shared.take_master_meters().frames, 512);
        let now = callback.engine().next_frame();
        push(&mut producer, now, 4, 0.8);
        push(&mut producer, now + 4, 4, 0.05);
        let mut data = [0.0_f32; 8];
        callback.process(&mut data);
        callback.process(&mut data);
        // The desk looks only now: the loud callback is still there.
        let take = shared.take_channel_meters(0);
        assert!((take.peak.left - 0.8).abs() < 1e-4, "{take:?}");
        assert_eq!(take.frames, 8);
        assert!((shared.take_master_meters().peak.left - 0.8).abs() < 1e-4);

        push(&mut producer, now + 8, 4, 0.05);
        callback.process(&mut data);
        assert!((shared.take_channel_meters(0).peak.left - 0.05).abs() < 1e-4);
    }

    #[test]
    fn empty_callback_publishes_nothing() {
        let (mut callback, _producer, shared) = setup(64, 4, 16);
        let mut data = [0.0_f32; 8];
        callback.process(&mut data);
        assert_eq!(shared.callback_frames(), Some(4));
        let mut empty: [f32; 0] = [];
        callback.process(&mut empty);
        assert_eq!(shared.callback_frames(), Some(4));
    }

    #[test]
    fn integer_sample_formats_are_converted() {
        let (mut callback, mut producer, _shared) = setup(64, 4, 16);
        let mut warm = vec![0_i16; 1024];
        callback.process(&mut warm);
        let now = callback.engine().next_frame();
        push(&mut producer, now, 4, 0.5);
        let mut data = [0_i16; 8];
        callback.process(&mut data);
        assert!(
            data.iter().all(|s| (i32::from(*s) - 16_384).abs() < 2),
            "{data:?}"
        );
    }

    #[test]
    fn controls_reach_the_engine_every_callback() {
        let (mut callback, _producer, shared) = setup(64, 4, 16);
        let mut controls = shared.channel_controls(2);
        controls.muted = true;
        shared.store_channel_controls(2, controls);
        let mut data = [0.0_f32; 8];
        callback.process(&mut data);
        assert!(callback.engine().channel_controls(2).unwrap().muted);
    }

    /// The ring the hub gives an instrument must keep large device buffers
    /// fed: an instrument that tops it up between callbacks yields
    /// continuous audio at every callback size the desk supports.
    #[test]
    fn an_instrument_ring_feeds_large_device_buffers() {
        const BLOCK: u32 = 512;
        for device_frames in [4096_u32, 6000, 16_384] {
            let (producer, consumer) = audio_block_ring(crate::hub::strip_ring(0, 2));
            let mut engine = MixerEngine::new(DESK_CHANNELS, 48_000).unwrap();
            engine
                .attach_consumer(0, short_name("inst"), consumer)
                .unwrap();
            let shared = Arc::new(SharedState::new());
            let mut callback = AudioCallback::new(engine, Arc::clone(&shared), 2);
            let mut producer = producer;
            let block = vec![0.5; BLOCK as usize * 2];
            let mut next_source_frame = 0_u64;
            let mut data = vec![0.0_f32; device_frames as usize * 2];
            for round in 0..20 {
                // Top the ring up, as the hub does between callbacks.
                while producer
                    .push_block(AudioBlock {
                        header: AudioBlockHeader {
                            start_frame: next_source_frame,
                            frames: BLOCK,
                            channels: 2,
                            sequence: 0,
                            flags: BlockFlags::default(),
                        },
                        samples: &block,
                    })
                    .is_ok()
                {
                    next_source_frame += u64::from(BLOCK);
                }
                callback.process(&mut data);
                if round > 0 {
                    let silent = data.iter().filter(|s| s.abs() < 0.4).count();
                    assert_eq!(
                        silent, 0,
                        "{device_frames} frames, round {round}: {silent} silent samples"
                    );
                }
            }
            let snapshot = callback.engine().channel_snapshots()[0];
            assert_eq!(snapshot.resyncs, 0, "{device_frames} frames");
        }
    }

    #[test]
    fn the_metronome_follows_the_transport_and_is_heard_when_on() {
        let (mut callback, _producer, shared) = setup(4096, 256, 8);
        let mut data = vec![0.0_f32; 1024];
        callback.process(&mut data);
        assert_eq!(shared.beat(), None);

        // Play lands two buffers ahead: the instruments hear of it first.
        shared.set_playing(true);
        callback.process(&mut data);
        assert_eq!(shared.beat(), None);
        callback.process(&mut data);
        callback.process(&mut data);
        assert_eq!(shared.beat(), Some(0));
        assert!(
            data.iter().all(|s| s.abs() < 1e-6),
            "the click is off by default"
        );

        shared.set_playing(false);
        for _ in 0..3 {
            callback.process(&mut data);
        }
        assert_eq!(shared.beat(), None);
        shared.set_click(true);
        shared.set_playing(true);
        let mut heard = false;
        for _ in 0..3 {
            callback.process(&mut data);
            heard |= data.iter().any(|s| s.abs() > 0.05);
        }
        assert!(heard, "beat one clicks as play starts");
    }

    #[test]
    fn every_scheduled_change_is_fed_to_the_hub_with_its_frame() {
        let (callback, _producer, shared) = setup(4096, 256, 8);
        let (mut hub, callback_end) = crate::patchbay::patchbay(4);
        let mut callback = callback.with_patchbay(callback_end);
        let mut data = vec![0.0_f32; 1024];
        callback.process(&mut data);
        let initial = hub.next_song().unwrap();
        assert!(!initial.playing);

        shared.set_schedule_ahead(4_800);
        shared.set_playing(true);
        let frame = callback.engine().next_frame();
        callback.process(&mut data);
        let play = hub.next_song().unwrap();
        assert!(play.playing);
        assert_eq!(play.frame, frame + 4_800);
        crate::test_support::assert_f64_eq(play.beat, 0.0);
        assert!(hub.next_song().is_none(), "nothing changed since");
        callback.process(&mut data);
        assert!(hub.next_song().is_none());
    }
}
