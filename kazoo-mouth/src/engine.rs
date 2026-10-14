use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crossbeam_channel::Receiver;
use kazoo_core::host::link::HubEvent;
use kazoo_core::ipc::follow::{TransportFollower, beats_in};
use kazoo_core::ipc::link::HubLinkAudio;
use kazoo_core::sanitize_sample;
use kazoo_core::voice::judge::{JudgeFeed, JudgeResults};
use kazoo_core::voice::quantise::{QuantiseSettings, Quantiser};
use kazoo_core::voice::{VoiceFrame, VoiceTracker};
use kazoo_fx::Context;
use ringbuf::traits::{Consumer, Observer, Producer};
use ringbuf::{HeapCons, HeapProd};

use crate::synth::{SynthSettings, VoiceSynth};

pub(crate) mod capture;
pub(crate) mod fx;
pub(crate) mod looper;
mod meters;
use fx::{FxChain, Slot};
use looper::{LoopFeed, LoopLength, Looper};

pub trait DeskOut {
    fn send_voice(&mut self, frame: &VoiceFrame) -> bool;
    fn send_audio(&mut self, frames: u32, samples: &[f32]) -> bool;
    fn is_connected(&self) -> bool;
    fn drain_events(&mut self, on: &mut dyn FnMut(HubEvent));
    fn stream_frame(&self) -> u64;
    fn shutdown_requested(&mut self) -> bool {
        false
    }
}

impl DeskOut for HubLinkAudio {
    fn send_voice(&mut self, frame: &VoiceFrame) -> bool {
        Self::send_voice(self, frame)
    }

    fn send_audio(&mut self, frames: u32, samples: &[f32]) -> bool {
        Self::send_audio(self, frames, samples)
    }

    fn is_connected(&self) -> bool {
        Self::is_connected(self)
    }

    fn drain_events(&mut self, on: &mut dyn FnMut(HubEvent)) {
        kazoo_core::host::link::drain_events(&mut self.events, on);
    }

    fn stream_frame(&self) -> u64 {
        Self::stream_frame(self)
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Command {
    SetQuantise(QuantiseSettings),
    SetSynth(SynthSettings),
    SetGate(kazoo_core::voice::GateSettings),
    SetSilent(bool),
    Shutdown,
    FxOn {
        slot: Slot,
        on: bool,
    },
    FxKind {
        slot: Slot,
        kind: u8,
    },
    FxParam {
        slot: Slot,
        kind: u8,
        index: u8,
        value: f32,
    },
    FxOrder([Slot; Slot::ALL.len()]),
    OwnTempo(f64),
    LoopRecord,
    LoopUndo,
    LoopClear,
    LoopLength(LoopLength),
    LoopPlay(bool),
    LoopVolume(f32),
}

#[derive(Debug, Default)]
pub struct Stats {
    pub capture: capture::Capture,
    pub fx_latency: AtomicU64,
    pub mic_dropped: AtomicU64,
    pub mic_skipped: AtomicU64,
    pub display_dropped: AtomicU64,
    pub voice_refused: AtomicU64,
    pub audio_refused: AtomicU64,
    pub synth_fault: AtomicBool,
    pub shutdown: AtomicBool,
    pub fx_faults: AtomicU64,
    pub fx_refused: AtomicU64,
    pub desk_lost: AtomicU64,
    pub own_bpm: AtomicU64,
    pub desk_bpm: AtomicU64,
    pub fx_moving: AtomicU64,
    pub fx_input: [AtomicU32; Slot::ALL.len()],
    pub fx_output: [AtomicU32; Slot::ALL.len()],
    pub mic_rms: AtomicU32,
    pub output_rms: AtomicU32,
}

impl Stats {
    /// Stats whose scope capture is sized for an engine running at `rate`.
    /// `Stats::default()` captures at 48 kHz.
    #[must_use]
    pub fn new(rate: u32) -> Self {
        Self {
            capture: capture::Capture::new(rate),
            ..Self::default()
        }
    }
}

pub struct EngineSetup<D> {
    pub rate: u32,
    pub block: usize,
    pub mic: HeapCons<f32>,
    pub(crate) commands: Receiver<Command>,
    pub display: HeapProd<VoiceFrame>,
    pub desk: D,
    pub judge: (JudgeFeed, JudgeResults),
    pub stats: Arc<Stats>,
}

pub struct MouthEngine<D> {
    mic_in: HeapCons<f32>,
    commands: Receiver<Command>,
    display: HeapProd<VoiceFrame>,
    desk: D,
    judge_feed: JudgeFeed,
    judge_results: JudgeResults,
    tracker: VoiceTracker,
    quantiser: Quantiser,
    synth: VoiceSynth,
    mic: Vec<f32>,
    mono: Vec<f32>,
    stereo: Vec<f32>,
    left: Box<[f32]>,
    right: Box<[f32]>,
    fx: FxChain,
    follower: TransportFollower,
    desk_bpm: Option<f64>,
    own_bpm: f64,
    beat: Option<f64>,
    beats: Box<[f64]>,
    looper: Looper,
    loop_feed: Option<LoopFeed>,
    loop_output: Box<[f32]>,
    loop_commands: [Option<Command>; 64],
    loop_command_count: usize,
    rate: u32,
    mic_meter: meters::Rms,
    output_meter: meters::Rms,
    silent: bool,
    samples: u64,
    stats: Arc<Stats>,
}

impl<D: std::fmt::Debug> std::fmt::Debug for EngineSetup<D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineSetup")
            .field("rate", &self.rate)
            .field("block", &self.block)
            .field("desk", &self.desk)
            .finish_non_exhaustive()
    }
}

impl<D: std::fmt::Debug> std::fmt::Debug for MouthEngine<D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MouthEngine")
            .field("desk", &self.desk)
            .field("silent", &self.silent)
            .field("samples", &self.samples)
            .finish_non_exhaustive()
    }
}

impl<D: DeskOut> MouthEngine<D> {
    pub fn new(setup: EngineSetup<D>) -> kazoo_core::Result<Self> {
        Self::with_settings(setup, &crate::controls::settings::Settings::default())
    }

    pub(crate) fn with_settings(
        setup: EngineSetup<D>,
        settings: &crate::controls::settings::Settings,
    ) -> kazoo_core::Result<Self> {
        if setup.rate == 0 || setup.block == 0 {
            return Err(kazoo_core::Error::Config(
                "mouth rate and block must be positive".into(),
            ));
        }
        if setup.stats.capture.rate() != setup.rate {
            return Err(kazoo_core::Error::Config(format!(
                "mouth stats capture at {} Hz, the engine runs at {} Hz",
                setup.stats.capture.rate(),
                setup.rate
            )));
        }
        let block = setup.block.min(4096);
        let mut synth = VoiceSynth::new(setup.rate, block)?;
        synth.set(settings.synth)?;
        let mut tracker = VoiceTracker::new(setup.rate);
        tracker.set_gate(settings.gate);
        let mut quantiser = Quantiser::new(setup.rate);
        quantiser.set(settings.quantise);
        let own_bpm = if settings.own_bpm.is_finite() {
            settings.own_bpm.clamp(30.0, 300.0)
        } else {
            120.0
        };
        setup
            .stats
            .own_bpm
            .store(own_bpm.to_bits(), Ordering::Relaxed);
        let (looper, loop_feed) =
            Looper::new(setup.rate, settings.loop_length, settings.loop_volume);
        Ok(Self {
            mic_in: setup.mic,
            commands: setup.commands,
            display: setup.display,
            desk: setup.desk,
            judge_feed: setup.judge.0,
            judge_results: setup.judge.1,
            tracker,
            quantiser,
            synth,
            mic: vec![0.0; block],
            mono: vec![0.0; block],
            stereo: vec![0.0; block * 2],
            left: vec![0.0; block].into_boxed_slice(),
            right: vec![0.0; block].into_boxed_slice(),
            fx: FxChain::new(setup.rate, block, &settings.fx)?,
            follower: TransportFollower::new(setup.rate),
            desk_bpm: None,
            own_bpm,
            beat: None,
            beats: vec![f64::NAN; block].into_boxed_slice(),
            looper,
            loop_feed: Some(loop_feed),
            loop_output: vec![0.0; block].into_boxed_slice(),
            loop_commands: [None; 64],
            loop_command_count: 0,
            rate: setup.rate,
            mic_meter: meters::Rms::default(),
            output_meter: meters::Rms::default(),
            silent: settings.silent,
            samples: 0,
            stats: setup.stats,
        })
    }

    #[must_use]
    pub const fn desk(&self) -> &D {
        &self.desk
    }

    pub(crate) const fn take_loop_feed(&mut self) -> Option<LoopFeed> {
        self.loop_feed.take()
    }

    pub fn render(&mut self, out: &mut [f32], channels: usize) {
        out.fill(0.0);
        for command in self.commands.try_iter().take(64) {
            match command {
                Command::SetQuantise(settings) => self.quantiser.set(settings),
                Command::SetSynth(settings) => {
                    if self.synth.set(settings).is_err() {
                        self.stats.synth_fault.store(true, Ordering::Relaxed);
                    }
                }
                Command::SetGate(settings) => self.tracker.set_gate(settings),
                Command::SetSilent(silent) => self.silent = silent,
                Command::FxOn { slot, on } => self.fx.on(slot, on),
                Command::FxKind { slot, kind } => self.fx.kind(slot, kind, &self.stats),
                Command::FxParam {
                    slot,
                    kind,
                    index,
                    value,
                } => self.fx.param(slot, kind, index, value, &self.stats),
                Command::FxOrder(order) => self.fx.reorder(order, &self.stats),
                Command::OwnTempo(bpm) => {
                    if !bpm.is_nan() {
                        self.own_bpm = bpm.clamp(30.0, 300.0);
                        self.stats
                            .own_bpm
                            .store(self.own_bpm.to_bits(), Ordering::Relaxed);
                    }
                }
                Command::Shutdown => {
                    self.silent = true;
                    self.stats.shutdown.store(true, Ordering::Release);
                }
                command @ (Command::LoopRecord
                | Command::LoopUndo
                | Command::LoopClear
                | Command::LoopLength(_)
                | Command::LoopPlay(_)
                | Command::LoopVolume(_)) => {
                    self.loop_commands[self.loop_command_count] = Some(command);
                    self.loop_command_count += 1;
                }
            }
        }
        let follower = &mut self.follower;
        let stats = &self.stats;
        self.desk.drain_events(&mut |event| match event {
            HubEvent::TransportSync(sync) => {
                if follower.schedule(&sync).is_err() {
                    stats.desk_lost.fetch_add(1, Ordering::Relaxed);
                }
            }
            HubEvent::Shutdown => stats.shutdown.store(true, Ordering::Release),
            HubEvent::NoteEvent(_)
            | HubEvent::ParameterChange(_)
            | HubEvent::StreamReady { .. } => {}
        });
        if self.desk.shutdown_requested() {
            self.stats.shutdown.store(true, Ordering::Release);
        }
        if channels == 0 || out.is_empty() {
            let bpm = self.desk_bpm.unwrap_or(self.own_bpm);
            self.apply_loop_commands(bpm);
            self.looper.process(&[], &[], bpm, &mut []);
            return;
        }
        // Discard once per host call, so chunking a long callback keeps its audio.
        let frames = out.len() / channels;
        let waiting = self.mic_in.occupied_len();
        if waiting > frames.saturating_mul(2) {
            let skipped = self.mic_in.skip(waiting - frames);
            self.stats
                .mic_skipped
                .fetch_add(skipped as u64, Ordering::Relaxed);
        }
        for chunk in out.chunks_mut(self.mic.len() * channels) {
            self.render_chunk(chunk, channels);
        }
        self.looper.publish(self.desk_bpm.unwrap_or(self.own_bpm));
    }

    fn apply_loop_commands(&mut self, bpm: f64) {
        for command in &mut self.loop_commands[..self.loop_command_count] {
            if let Some(command) = command.take() {
                self.looper.command(command, bpm);
            }
        }
        self.loop_command_count = 0;
    }

    fn publish_mic(&mut self, count: usize) {
        self.mic_meter.stereo(
            &self.mic[..count],
            &self.mic[..count],
            (self.rate / 10) as usize,
            &self.stats.mic_rms,
        );
    }

    fn publish_capture(&self, count: usize) {
        self.stats
            .capture
            .push(&self.mic[..count], &self.stereo[..count * 2]);
        self.stats
            .fx_latency
            .store(self.fx.latency() as u64, Ordering::Relaxed);
    }

    fn render_chunk(&mut self, out: &mut [f32], channels: usize) {
        let count = out.len() / channels;
        if count == 0 {
            self.apply_loop_commands(self.desk_bpm.unwrap_or(self.own_bpm));
            return;
        }
        let context = self.transport(count);
        self.apply_loop_commands(context.bpm);
        let filled = self.mic_in.pop_slice(&mut self.mic[..count]);
        self.mic[filled..count].fill(0.0);
        for sample in &mut self.mic[..filled] {
            *sample = sanitize_sample(*sample);
        }
        self.publish_mic(count);
        self.judge_feed.push(&self.mic[..count]);
        while let Some(result) = self.judge_results.pop() {
            self.tracker.judge(result);
        }
        let mut frames = [None; 64];
        let mut emitted = 0;
        self.tracker.process(&self.mic[..count], |frame| {
            frames[emitted] = Some(frame);
            emitted += 1;
        });
        let mut start = 0;
        for mut frame in frames.into_iter().take(emitted).flatten() {
            self.judge_feed.observe(frame);
            self.quantiser.process(&mut frame);
            self.synth.control(&frame);
            if !self.desk.send_voice(&frame) {
                self.stats.voice_refused.fetch_add(1, Ordering::Relaxed);
            }
            if self.display.try_push(frame).is_err() {
                self.stats.display_dropped.fetch_add(1, Ordering::Relaxed);
            }
            // The frame ends this segment; each hop gets its own synth controls.
            let end = (frame.at.saturating_sub(self.samples) as usize).min(count);
            self.synth
                .render(&self.mic[start..end], &mut self.mono[start..end]);
            start = end;
        }
        self.synth
            .render(&self.mic[start..count], &mut self.mono[start..count]);
        self.samples = self.samples.saturating_add(count as u64);
        self.looper.process_audio(
            &self.mono[..count],
            &self.beats[..count],
            context.bpm,
            &mut self.loop_output[..count],
        );
        for (live, playback) in self.mono[..count]
            .iter_mut()
            .zip(&self.loop_output[..count])
        {
            *live = sanitize_sample(*live + *playback);
        }
        self.fx.process(
            context,
            &self.mono[..count],
            [&mut self.left[..count], &mut self.right[..count]],
            &self.stats,
        );
        let mute = self.silent || self.stats.synth_fault.load(Ordering::Relaxed);
        for (i, stereo) in self.stereo[..count * 2].chunks_exact_mut(2).enumerate() {
            self.left[i] = fx::ceiling(self.left[i]);
            self.right[i] = fx::ceiling(self.right[i]);
            stereo[0] = if mute { 0.0 } else { self.left[i] };
            stereo[1] = if mute { 0.0 } else { self.right[i] };
        }
        if mute {
            self.output_meter = meters::Rms::default();
            self.stats.output_rms.store(0, Ordering::Relaxed);
        } else {
            self.output_meter.stereo(
                &self.left[..count],
                &self.right[..count],
                (self.rate / 10) as usize,
                &self.stats.output_rms,
            );
        }
        self.publish_capture(count);
        let connected = self.desk.is_connected();
        let accepted = self
            .desk
            .send_audio(count as u32, &self.stereo[..count * 2]);
        if connected && !accepted {
            self.stats.audio_refused.fetch_add(1, Ordering::Relaxed);
        }
        if !connected && !accepted && !mute {
            for (i, frame) in out.chunks_exact_mut(channels).enumerate() {
                frame[0] = if channels == 1 {
                    0.5 * (self.left[i] + self.right[i])
                } else {
                    self.left[i]
                };
                if channels > 1 {
                    frame[1] = self.right[i];
                }
            }
        }
    }

    fn transport(&mut self, count: usize) -> Context {
        let connected = self.desk.is_connected();
        if !connected {
            if let Some(bpm) = self.desk_bpm.take() {
                self.own_bpm = ((bpm * 10.0).round() / 10.0).clamp(30.0, 300.0);
                self.stats
                    .own_bpm
                    .store(self.own_bpm.to_bits(), Ordering::Relaxed);
                self.follower = TransportFollower::new(self.rate);
            }
            self.beat = None;
        }
        let first = self.desk.stream_frame();
        let mut bpm = self.desk_bpm.unwrap_or(self.own_bpm);
        for (i, beat) in self.beats[..count].iter_mut().enumerate() {
            if connected && let Some(change) = self.follower.due(first.saturating_add(i as u64)) {
                self.desk_bpm = Some(change.bpm);
                self.beat = change.beat;
            }
            if i == 0 {
                bpm = self.desk_bpm.unwrap_or(self.own_bpm);
            }
            *beat = self.beat.unwrap_or(f64::NAN);
            if let Some(beat) = &mut self.beat {
                *beat += beats_in(
                    1.0,
                    self.desk_bpm.unwrap_or(self.own_bpm),
                    f64::from(self.rate),
                );
            }
        }
        self.stats
            .desk_bpm
            .store(self.desk_bpm.map_or(0, f64::to_bits), Ordering::Relaxed);
        Context {
            bpm: if connected { bpm } else { self.own_bpm },
        }
    }
}

#[cfg(test)]
mod tests;
