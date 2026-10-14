use std::f32::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use color_eyre::Result;
use kazoo_core::analysis::{PitchDetectorConfig, score::signals};
use kazoo_core::synthesis::OscillatorShape;
use kazoo_core::voice::judge::{self, Judge, OnsetConfig};
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};

use super::{Command, DeskOut, EngineSetup, MouthEngine, Stats};
use crate::synth::SynthSettings;
use kazoo_core::voice::VoiceFrame;
use kazoo_core::voice::quantise::QuantiseSettings;

const RATE: u32 = 48_000;
const BLOCK: usize = 128;

#[derive(Debug)]
struct FakeHub {
    connected: bool,
    voice: Vec<VoiceFrame>,
    audio: Vec<(u32, f32)>,
    events: std::collections::VecDeque<kazoo_core::host::link::HubEvent>,
    frames: u64,
}

impl FakeHub {
    fn new(connected: bool) -> Self {
        Self {
            connected,
            voice: Vec::with_capacity(8000),
            audio: Vec::with_capacity(8000),
            events: std::collections::VecDeque::with_capacity(16),
            frames: 0,
        }
    }
}

impl DeskOut for FakeHub {
    fn send_voice(&mut self, frame: &VoiceFrame) -> bool {
        self.voice.push(*frame);
        true
    }

    fn send_audio(&mut self, frames: u32, samples: &[f32]) -> bool {
        assert_eq!(samples.len(), frames as usize * 2);
        assert!(samples.iter().all(|sample| sample.is_finite()));
        assert!(
            samples
                .chunks_exact(2)
                .all(|pair| pair[0].to_bits() == pair[1].to_bits())
        );
        let peak = samples
            .iter()
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        self.frames += u64::from(frames);
        self.audio.push((frames, peak));
        self.connected
    }

    fn drain_events(&mut self, on: &mut dyn FnMut(kazoo_core::host::link::HubEvent)) {
        while let Some(event) = self.events.pop_front() {
            on(event);
        }
    }

    fn stream_frame(&self) -> u64 {
        self.frames
    }

    fn is_connected(&self) -> bool {
        self.connected
    }
}

struct Rig {
    engine: MouthEngine<FakeHub>,
    mic: HeapProd<f32>,
    display: HeapCons<VoiceFrame>,
    commands: crossbeam_channel::Sender<Command>,
    stats: Arc<Stats>,
    judge: Judge,
}

impl Rig {
    fn new(connected: bool, block: usize) -> Result<Self> {
        let (mic, mic_rx) = HeapRb::new(RATE as usize / 10).split();
        let (display_tx, display) = HeapRb::new(1024).split();
        let (commands, command_rx) = crossbeam_channel::bounded(64);
        let (feed, results, judge) = judge::spawn(
            PitchDetectorConfig::for_rate(RATE),
            block,
            OnsetConfig::default(),
        )?;
        let stats = Arc::new(Stats::default());
        let engine = MouthEngine::new(EngineSetup {
            rate: RATE,
            block,
            mic: mic_rx,
            commands: command_rx,
            display: display_tx,
            desk: FakeHub::new(connected),
            judge: (feed, results),
            stats: Arc::clone(&stats),
        })?;
        Ok(Self {
            engine,
            mic,
            display,
            commands,
            stats,
            judge,
        })
    }

    fn block(&mut self, input: &[f32], out: &mut [f32]) {
        assert_eq!(self.mic.push_slice(input), input.len());
        self.engine.render(out, 2);
    }

    fn finish(self) {
        drop(self.engine);
        assert!(self.judge.join().is_ok(), "pitch judge failed to stop");
    }
}

fn sine(hz: f32, count: usize) -> Vec<f32> {
    (0..count)
        .map(|sample| (TAU * hz * sample as f32 / RATE as f32).sin() * 0.5)
        .collect()
}

fn play(rig: &mut Rig, input: &[f32]) -> Vec<f32> {
    let mut result = Vec::with_capacity(input.len());
    let mut out = [0.0; BLOCK * 2];
    for block in input.chunks(BLOCK) {
        rig.block(block, &mut out[..block.len() * 2]);
        result.extend(out[..block.len() * 2].chunks_exact(2).map(|frame| frame[0]));
    }
    result
}

fn cents(hz: f32, expected: f32) -> f32 {
    (1200.0 * (hz / expected).log2()).abs()
}

fn crossings(samples: &[f32]) -> Vec<f32> {
    samples
        .windows(2)
        .enumerate()
        .filter(|(_, pair)| pair[0] <= 0.0 && pair[1] > 0.0)
        .map(|(index, pair)| index as f32 - pair[0] / (pair[1] - pair[0]))
        .collect()
}

fn measured_hz(samples: &[f32]) -> f32 {
    let crossings = crossings(samples);
    assert!(
        crossings.len() > 2,
        "synth did not produce measurable cycles"
    );
    let span = crossings[crossings.len() - 1] - crossings[0];
    (crossings.len() - 1) as f32 * RATE as f32 / span
}

#[test]
fn a_frame_is_published_every_64_samples() -> Result<()> {
    let mut rig = Rig::new(false, BLOCK)?;
    let input = sine(220.0, RATE as usize);
    let output = play(&mut rig, &input);
    assert_eq!(output.len(), RATE as usize);
    let frames = &rig.engine.desk().voice;
    assert_eq!(frames.len(), 750);
    for (index, frame) in frames.iter().enumerate() {
        assert_eq!(frame.at, (index as u64 + 1) * 64);
    }
    rig.finish();
    Ok(())
}

#[test]
fn quantise_reaches_the_published_frame() -> Result<()> {
    let mut rig = Rig::new(false, BLOCK)?;
    rig.commands
        .try_send(Command::SetQuantise(QuantiseSettings {
            snap_ms: 0.0,
            blend: 1.0,
            keep_vibrato: false,
            ..QuantiseSettings::default()
        }))?;
    let flat = 440.0 * (-30.0_f32 / 1200.0).exp2();
    let output = play(&mut rig, &sine(flat, RATE as usize / 2));
    assert!(output.iter().any(|sample| sample.abs() > 0.01));
    let frames: Vec<_> = rig
        .engine
        .desk()
        .voice
        .iter()
        .filter(|frame| frame.voiced && frame.at > 12_000)
        .collect();
    assert!(!frames.is_empty());
    for frame in frames {
        assert!(cents(frame.snapped_hz, 440.0) < 0.1, "{frame:?}");
        assert!(cents(frame.hz, flat) < 1.0, "{frame:?}");
    }
    rig.finish();
    Ok(())
}

#[test]
fn the_synth_plays_the_voice_hertz_for_hertz() -> Result<()> {
    let mut rig = Rig::new(false, BLOCK)?;
    rig.commands.try_send(Command::SetSynth(SynthSettings {
        shape: OscillatorShape::Saw,
        air: 0.0,
        ..SynthSettings::default()
    }))?;
    let change = RATE as usize / 2;
    let mut input = sine(220.0, change);
    input.extend(sine(330.0, change));
    let output = play(&mut rig, &input);
    let window = RATE as usize / 20;
    assert!(cents(measured_hz(&output[change - window..change]), 220.0) < 5.0);
    assert!(cents(measured_hz(&output[output.len() - window..]), 330.0) < 5.0);
    let crossings = crossings(&output[change - BLOCK..change + window]);
    let changed = crossings.windows(2).find_map(|pair| {
        let hz = RATE as f32 / (pair[1] - pair[0]);
        (pair[1] >= BLOCK as f32 && hz > 220.0 && cents(hz, 220.0) > 5.0)
            .then_some((pair[1] - BLOCK as f32) / RATE as f32)
    });
    let limit = 2.0 / 330.0 + 0.001 + BLOCK as f32 / RATE as f32;
    assert!(
        changed.is_some_and(|seconds| seconds <= limit),
        "pitch change at {changed:?}; limit {limit}"
    );
    rig.finish();
    Ok(())
}

#[test]
fn silence_in_is_silence_out() -> Result<()> {
    let mut rig = Rig::new(false, BLOCK)?;
    let mut input = sine(220.0, RATE as usize / 2);
    input.resize(input.len() + RATE as usize / 2, 0.0);
    let output = play(&mut rig, &input);
    let after_tail = RATE as usize / 2 + RATE as usize * 3 / 10;
    assert!(
        output[after_tail..]
            .iter()
            .all(|sample| sample.abs() < 0.0001),
        "output exceeds -80 dBFS after 300 ms"
    );
    rig.finish();
    Ok(())
}

#[test]
fn silent_mode_publishes_but_plays_nothing() -> Result<()> {
    for connected in [false, true] {
        let mut rig = Rig::new(connected, BLOCK)?;
        rig.commands.try_send(Command::SetSilent(true))?;
        let output = play(&mut rig, &sine(220.0, RATE as usize));
        assert_eq!(rig.engine.desk().voice.len(), 750);
        assert!(rig.engine.desk().voice.iter().any(|frame| frame.voiced));
        assert!(output.iter().all(|sample| sample.to_bits() == 0));
        assert!(
            rig.engine
                .desk()
                .audio
                .iter()
                .all(|(_, peak)| peak.to_bits() == 0)
        );
        rig.finish();
    }
    Ok(())
}

#[test]
fn plugged_in_the_desk_gets_the_audio_and_the_speakers_are_silent() -> Result<()> {
    let mut rig = Rig::new(true, BLOCK)?;
    let input = sine(220.0, RATE as usize);
    let output = play(&mut rig, &input);
    assert_eq!(rig.engine.desk().audio.len(), input.len().div_ceil(BLOCK));
    assert!(rig.engine.desk().audio.iter().any(|(_, peak)| *peak > 0.01));
    assert_eq!(
        rig.engine
            .desk()
            .audio
            .iter()
            .map(|(frames, _)| u64::from(*frames))
            .sum::<u64>(),
        u64::from(RATE)
    );
    assert!(output.iter().all(|sample| sample.to_bits() == 0));
    rig.finish();
    Ok(())
}

#[test]
fn nonsense_mic_input_is_survived() -> Result<()> {
    let mut rig = Rig::new(false, BLOCK)?;
    let mut input = sine(220.0, RATE as usize / 10);
    input.extend(
        [f32::NAN, f32::INFINITY, f32::NEG_INFINITY]
            .into_iter()
            .cycle()
            .take(4800),
    );
    input.extend(sine(330.0, 4800));
    let output = play(&mut rig, &input);
    assert!(output.iter().all(|sample| sample.is_finite()));
    for frame in &rig.engine.desk().voice {
        assert!(
            [
                frame.hz,
                frame.volts,
                frame.snapped_hz,
                frame.snapped_volts,
                frame.loudness,
                frame.breath,
                frame.confidence
            ]
            .iter()
            .all(|value| value.is_finite())
        );
    }
    assert!(
        rig.engine
            .desk()
            .voice
            .iter()
            .any(|frame| frame.at > 10_000 && frame.voiced)
    );
    rig.finish();
    Ok(())
}

#[test]
fn render_never_allocates() -> Result<()> {
    let mut rig = Rig::new(true, BLOCK)?;
    let vowel = signals::suite(RATE)
        .into_iter()
        .find(|take| take.name == "vowel_ah")
        .ok_or_else(|| color_eyre::eyre::eyre!("vowel_ah fixture missing"))?;
    let input: Vec<_> = vowel
        .samples
        .iter()
        .copied()
        .cycle()
        .take(RATE as usize * 10)
        .collect();
    let mut output = [0.0; BLOCK * 2];
    let before = assert_no_alloc::violation_count();
    assert_no_alloc::assert_no_alloc(|| {
        for block in input.chunks(BLOCK) {
            assert_eq!(rig.mic.push_slice(block), block.len());
            rig.engine.render(&mut output[..block.len() * 2], 2);
        }
    });
    assert_eq!(assert_no_alloc::violation_count(), before);
    assert_eq!(rig.engine.desk().voice.len(), 7500);
    assert!(output.iter().all(|sample| sample.is_finite()));
    rig.finish();
    Ok(())
}

#[test]
fn callbacks_longer_than_4096_keep_the_frame_clock() -> Result<()> {
    let mut rig = Rig::new(false, 8192)?;
    let input = sine(220.0, 4096);
    let mut output = vec![0.0; 8192 * 2];
    assert_eq!(rig.mic.push_slice(&input), input.len());
    rig.engine.render(&mut output, 2);
    assert_eq!(rig.engine.desk().voice.len(), 128);
    assert_eq!(
        rig.engine.desk().voice.last().map(|frame| frame.at),
        Some(8192)
    );
    assert!(output.iter().all(|sample| sample.is_finite()));
    assert_eq!(rig.engine.desk().audio.len(), 2);
    rig.finish();
    Ok(())
}

#[test]
fn backlog_keeps_the_newest_block_and_display_keeps_the_oldest() -> Result<()> {
    let mut rig = Rig::new(false, BLOCK)?;
    let backlog = vec![0.5; BLOCK * 4];
    let mut output = [0.0; BLOCK * 2];
    assert_eq!(rig.mic.push_slice(&backlog), backlog.len());
    rig.engine.render(&mut output, 2);
    assert_eq!(
        rig.stats.mic_skipped.load(Ordering::Relaxed),
        (BLOCK * 3) as u64
    );
    let input = sine(220.0, 70_000);
    let output = play(&mut rig, &input);
    assert!(output.iter().all(|sample| sample.is_finite()));
    let frames: Vec<_> = rig.display.pop_iter().collect();
    assert_eq!(frames.len(), 1024);
    assert_eq!(frames[0].at, 64);
    assert_eq!(frames[1023].at, 65_536);
    assert!(rig.stats.display_dropped.load(Ordering::Relaxed) > 0);
    rig.finish();
    Ok(())
}

#[test]
fn the_meters_never_allocate() -> Result<()> {
    let mut rig = Rig::new(true, BLOCK)?;
    let input = [0.2; BLOCK];
    let mut output = [0.0; BLOCK * 2];
    for meter in rig.stats.fx_input.iter().chain(&rig.stats.fx_output) {
        meter.store(f32::NAN.to_bits(), Ordering::Relaxed);
    }
    let before = assert_no_alloc::violation_count();
    assert_no_alloc::assert_no_alloc(|| {
        for _ in 0..40 {
            rig.block(&input, &mut output);
        }
    });
    assert_eq!(assert_no_alloc::violation_count(), before);
    assert!(
        f32::from_bits(rig.stats.mic_rms.load(Ordering::Relaxed)) > 0.0,
        "mic RMS was not published"
    );
    for meter in rig.stats.fx_input.iter().chain(&rig.stats.fx_output) {
        assert!(f32::from_bits(meter.load(Ordering::Relaxed)).is_finite());
    }
    rig.finish();
    Ok(())
}
