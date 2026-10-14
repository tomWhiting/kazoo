use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crossbeam_channel::Receiver;
use kazoo_core::ipc::link::HubLinkAudio;
use kazoo_core::sanitize_sample;
use kazoo_core::voice::judge::{JudgeFeed, JudgeResults};
use kazoo_core::voice::quantise::{QuantiseSettings, Quantiser};
use kazoo_core::voice::{VoiceFrame, VoiceTracker};
use ringbuf::traits::{Consumer, Observer, Producer};
use ringbuf::{HeapCons, HeapProd};

use crate::synth::{SynthSettings, VoiceSynth};

pub trait DeskOut {
    fn send_voice(&mut self, frame: &VoiceFrame) -> bool;
    fn send_audio(&mut self, frames: u32, samples: &[f32]) -> bool;
    fn is_connected(&self) -> bool;
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

    fn shutdown_requested(&mut self) -> bool {
        let mut shutdown = false;
        for event in kazoo_core::host::link::take_events(&mut self.events) {
            shutdown |= matches!(event, kazoo_core::host::link::HubEvent::Shutdown);
        }
        shutdown
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Command {
    SetQuantise(QuantiseSettings),
    SetSynth(SynthSettings),
    SetSilent(bool),
    Shutdown,
}

#[derive(Debug, Default)]
pub struct Stats {
    pub mic_dropped: AtomicU64,
    pub mic_skipped: AtomicU64,
    pub display_dropped: AtomicU64,
    pub voice_refused: AtomicU64,
    pub audio_refused: AtomicU64,
    pub synth_fault: AtomicBool,
    pub shutdown: AtomicBool,
}

pub struct EngineSetup<D> {
    pub rate: u32,
    pub block: usize,
    pub mic: HeapCons<f32>,
    pub commands: Receiver<Command>,
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
        if setup.rate == 0 || setup.block == 0 {
            return Err(kazoo_core::Error::Config(
                "mouth rate and block must be positive".into(),
            ));
        }
        let block = setup.block.min(4096);
        Ok(Self {
            mic_in: setup.mic,
            commands: setup.commands,
            display: setup.display,
            desk: setup.desk,
            judge_feed: setup.judge.0,
            judge_results: setup.judge.1,
            tracker: VoiceTracker::new(setup.rate),
            quantiser: Quantiser::new(setup.rate),
            synth: VoiceSynth::new(setup.rate, block)?,
            mic: vec![0.0; block],
            mono: vec![0.0; block],
            stereo: vec![0.0; block * 2],
            silent: false,
            samples: 0,
            stats: setup.stats,
        })
    }

    #[must_use]
    pub const fn desk(&self) -> &D {
        &self.desk
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
                Command::SetSilent(silent) => self.silent = silent,
                Command::Shutdown => {
                    self.silent = true;
                    self.stats.shutdown.store(true, Ordering::Release);
                }
            }
        }
        if self.desk.shutdown_requested() {
            self.stats.shutdown.store(true, Ordering::Release);
        }
        if channels == 0 {
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
    }

    fn render_chunk(&mut self, out: &mut [f32], channels: usize) {
        let count = out.len() / channels;
        if count == 0 {
            return;
        }
        let filled = self.mic_in.pop_slice(&mut self.mic[..count]);
        self.mic[filled..count].fill(0.0);
        for sample in &mut self.mic[..filled] {
            *sample = sanitize_sample(*sample);
        }
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
        if self.silent || self.stats.synth_fault.load(Ordering::Relaxed) {
            self.mono[..count].fill(0.0);
        }
        for (stereo, &sample) in self.stereo[..count * 2]
            .chunks_exact_mut(2)
            .zip(&self.mono[..count])
        {
            stereo.fill(sanitize_sample(sample));
        }
        let connected = self.desk.is_connected();
        let accepted = self
            .desk
            .send_audio(count as u32, &self.stereo[..count * 2]);
        if connected && !accepted {
            self.stats.audio_refused.fetch_add(1, Ordering::Relaxed);
        }
        if !connected && !accepted {
            for (frame, &sample) in out.chunks_exact_mut(channels).zip(&self.mono[..count]) {
                frame.fill(sanitize_sample(sample));
            }
        }
    }
}

#[cfg(test)]
mod tests;
