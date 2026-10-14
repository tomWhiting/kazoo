use std::f32::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use color_eyre::Result;
use kazoo_core::analysis::PitchDetectorConfig;
use kazoo_core::host::link::HubEvent;
use kazoo_core::ipc::types::{TRANSPORT_PLAYING, TransportSyncMsg};
use kazoo_core::voice::VoiceFrame;
use kazoo_core::voice::judge::{self, Judge, OnsetConfig};
use ringbuf::traits::{Producer, Split};
use ringbuf::{HeapProd, HeapRb};

use super::{Context, FxChain, FxSettings, MAX_PARAMS, Slot, ceiling, mouth_default};
use crate::engine::{Command, DeskOut, EngineSetup, MouthEngine, Stats};

const RATE: u32 = 48_000;
const BLOCK: usize = 128;
const AMP: f32 = 0.251_188_64;

fn saw(count: usize) -> Vec<f32> {
    (0..count)
        .map(|i| ((i as f32 * 220.0 / RATE as f32).fract().mul_add(2.0, -1.0)) * AMP)
        .collect()
}

fn sine(count: usize) -> Vec<f32> {
    (0..count)
        .map(|i| (TAU * 220.0 * i as f32 / RATE as f32).sin() * AMP)
        .collect()
}

fn only(slot: Slot, kind: usize) -> FxSettings {
    let mut settings = FxSettings::default();
    settings.slots[slot as usize].on = true;
    settings.slots[slot as usize].active = kind;
    settings
}

fn chain(settings: &FxSettings) -> FxChain {
    FxChain::new(RATE, BLOCK, settings).unwrap()
}

fn render(chain: &mut FxChain, input: &[f32], bpm: f64, stats: &Stats) -> [Vec<f32>; 2] {
    let mut output = [
        Vec::with_capacity(input.len()),
        Vec::with_capacity(input.len()),
    ];
    let mut left = [0.0; BLOCK];
    let mut right = [0.0; BLOCK];
    for chunk in input.chunks(BLOCK) {
        chain.process(
            Context { bpm },
            chunk,
            [&mut left[..chunk.len()], &mut right[..chunk.len()]],
            stats,
        );
        output[0].extend_from_slice(&left[..chunk.len()]);
        output[1].extend_from_slice(&right[..chunk.len()]);
    }
    output
}

fn param(settings: &mut FxSettings, slot: Slot, kind: usize, name: &str, value: f32) -> usize {
    let description = slot.kind(kind).unwrap();
    let index = description
        .params
        .iter()
        .position(|spec| spec.name == name)
        .unwrap();
    settings.slots[slot as usize].values[kind][index] = description.params[index].clamp(value);
    index
}

fn max_step(output: &[f32]) -> f32 {
    output
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0, f32::max)
}

fn bound(first: &[Vec<f32>; 2], second: &[Vec<f32>; 2]) -> f32 {
    first
        .iter()
        .chain(second.iter())
        .map(|side| max_step(side))
        .fold(0.0, f32::max)
        * 1.5
}

#[test]
fn the_chain_starts_in_order_and_all_off_is_the_synth_bit_for_bit() {
    let settings = FxSettings::default();
    assert_eq!(settings.order, Slot::ALL);
    assert!(settings.slots.iter().all(|slot| !slot.on));
    let mut chain = chain(&settings);
    let input = saw(RATE as usize * 2);
    let output = render(&mut chain, &input, 120.0, &Stats::default());
    for side in output {
        assert!(
            side.iter()
                .zip(&input)
                .all(|(a, b)| a.to_bits() == b.to_bits())
        );
    }
    assert!(chain.pedals.iter().all(|slot| slot.asleep));
}

#[test]
fn each_slot_is_its_kazoo_fx_kind() {
    let input = saw(RATE as usize);
    for slot in Slot::ALL {
        for kind in 0..slot.ids().len() {
            let description = slot.kind(kind).unwrap();
            let mut effect = (description.build)();
            effect.prepare(RATE as f32);
            effect.set_seed(slot.seed(kind));
            for (index, spec) in description.params.iter().enumerate() {
                effect.set_param(index, mouth_default(slot, kind, spec));
            }
            effect.reset();
            let output = render(
                &mut chain(&only(slot, kind)),
                &input,
                120.0,
                &Stats::default(),
            );
            let mut left = [0.0; BLOCK];
            let mut right = [0.0; BLOCK];
            for (chunk, got) in input
                .chunks(BLOCK)
                .zip(output[0].chunks(BLOCK).zip(output[1].chunks(BLOCK)))
            {
                effect.process(
                    &Context { bpm: 120.0 },
                    [chunk, chunk],
                    [&mut left[..chunk.len()], &mut right[..chunk.len()]],
                );
                for (side, reference) in [got.0, got.1].into_iter().zip([&left, &right]) {
                    assert!(
                        side.iter()
                            .zip(reference)
                            .all(|(a, b)| a.to_bits() == b.to_bits()),
                        "{}",
                        description.id
                    );
                }
            }
        }
    }
}

// Radix-two FFT band sums keep this acoustic check independent of DSP internals.
fn band_energy(samples: &[f32], low: f64, high: f64) -> f64 {
    let count = samples.len();
    assert!(count.is_power_of_two());
    let mut bins: Vec<_> = samples
        .iter()
        .enumerate()
        .map(|(i, value)| {
            let window = 0.5_f64.mul_add(
                -(std::f64::consts::TAU * i as f64 / count as f64).cos(),
                0.5,
            );
            (f64::from(*value) * window, 0.0_f64)
        })
        .collect();
    for i in 0..count {
        let reversed = i.reverse_bits() >> (usize::BITS - count.trailing_zeros());
        if reversed > i {
            bins.swap(i, reversed);
        }
    }
    let mut size = 2;
    while size <= count {
        for start in (0..count).step_by(size) {
            for offset in 0..size / 2 {
                let phase = -std::f64::consts::TAU * offset as f64 / size as f64;
                let (sin, cos) = phase.sin_cos();
                let a = bins[start + offset];
                let b = bins[start + offset + size / 2];
                let b = (b.0.mul_add(cos, -b.1 * sin), b.0.mul_add(sin, b.1 * cos));
                bins[start + offset] = (a.0 + b.0, a.1 + b.1);
                bins[start + offset + size / 2] = (a.0 - b.0, a.1 - b.1);
            }
        }
        size *= 2;
    }
    bins.iter()
        .take(count / 2)
        .enumerate()
        .filter(|(i, _)| {
            let hz = *i as f64 * f64::from(RATE) / count as f64;
            hz >= low && hz < high
        })
        .map(|(_, (re, im))| re.mul_add(*re, im * im))
        .sum()
}

#[test]
fn the_megaphone_is_the_radios_horn() {
    let settings = only(Slot::Megaphone, 0);
    let description = Slot::Megaphone.kind(0).unwrap();
    for (name, expected) in [("model", 3.0_f32), ("drive", 50.0), ("noise", 0.0)] {
        let index = description
            .params
            .iter()
            .position(|spec| spec.name == name)
            .unwrap();
        assert_eq!(
            settings.slots[Slot::Megaphone as usize].values[0][index].to_bits(),
            expected.to_bits()
        );
    }
    let mut seed = 7_u32;
    let input: Vec<_> = (0..RATE)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed as f32 / u32::MAX as f32).mul_add(2.0, -1.0) * AMP
        })
        .collect();
    let mut chain = chain(&settings);
    let output = render(&mut chain, &input, 120.0, &Stats::default());
    let window = &output[0][output[0].len() - 8192..];
    assert!(band_energy(window, 0.0, 250.0) < band_energy(window, 1000.0, 3000.0) * 0.01);
    let silence = render(&mut chain, &vec![0.0; 4800], 120.0, &Stats::default());
    assert!(silence.iter().all(|side| {
        side[2400..]
            .iter()
            .all(|sample| sample.abs() < 10.0_f32.powf(-90.0 / 20.0))
    }));
}

#[test]
fn switching_on_and_off_never_clicks() {
    let input = sine(RATE as usize);
    for slot in Slot::ALL {
        let stats = Stats::default();
        let off = render(&mut chain(&FxSettings::default()), &input, 120.0, &stats);
        let on = render(&mut chain(&only(slot, 0)), &input, 120.0, &stats);
        let limit = bound(&off, &on);
        let mut chain = chain(&FxSettings::default());
        let mut output = [Vec::new(), Vec::new()];
        let interval = 1776;
        for switch in 0..10 {
            chain.on(slot, switch % 2 == 0);
            let part = render(
                &mut chain,
                &input[switch * interval..(switch + 1) * interval],
                120.0,
                &stats,
            );
            for (to, from) in output.iter_mut().zip(part) {
                to.extend(from);
            }
        }
        assert!(
            output.iter().all(|side| max_step(side) <= limit),
            "{}: {} > {limit}, peak {:?}",
            slot.name(),
            max_step(&output[0]),
            output[0]
                .windows(2)
                .enumerate()
                .max_by(|a, b| (a.1[1] - a.1[0]).abs().total_cmp(&(b.1[1] - b.1[0]).abs()))
        );
    }
}

fn peak_at(samples: &[f32], seconds: f64) -> usize {
    let frame = (seconds * f64::from(RATE)).round() as usize;
    let first = frame.saturating_sub(48);
    let last = (frame + 49).min(samples.len());
    first
        + samples[first..last]
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .unwrap()
            .0
}

#[test]
fn a_delay_rings_out_after_it_is_switched_off() {
    let mut settings = only(Slot::Delay, 0);
    param(&mut settings, Slot::Delay, 0, "mix", 1.0);
    param(&mut settings, Slot::Delay, 0, "feedback", 0.0);
    let mut chain = chain(&settings);
    let stats = Stats::default();
    let mut input = vec![0.0; RATE as usize * 3];
    input[0] = AMP;
    input[9600] = AMP;
    let mut output = render(&mut chain, &input[..2400], 120.0, &stats);
    chain.on(Slot::Delay, false);
    let mut asleep_at = None;
    let mut left = [0.0; BLOCK];
    let mut right = [0.0; BLOCK];
    for (index, chunk) in input[2400..].chunks(BLOCK).enumerate() {
        chain.process(
            Context { bpm: 120.0 },
            chunk,
            [&mut left[..chunk.len()], &mut right[..chunk.len()]],
            &stats,
        );
        output[0].extend_from_slice(&left[..chunk.len()]);
        output[1].extend_from_slice(&right[..chunk.len()]);
        if chain.pedals[Slot::Delay as usize].asleep && asleep_at.is_none() {
            asleep_at = Some(2400 + index * BLOCK + chunk.len());
        }
    }
    let last_audible = output[0]
        .iter()
        .zip(&output[1])
        .rposition(|(left, right)| left.abs() >= 1e-6 || right.abs() >= 1e-6)
        .unwrap();
    assert!(asleep_at.unwrap() <= last_audible + RATE as usize + BLOCK);
    let repeat = peak_at(&output[0], 0.375);
    assert!(repeat.abs_diff(18_000) <= 48);
    assert!(output[0][repeat].abs() > 0.01);
    assert!(output.iter().all(|side| {
        side[27_552..27_648]
            .iter()
            .all(|sample| sample.abs() < 1e-6)
    }));
    assert!(chain.pedals[Slot::Delay as usize].asleep);
}

#[derive(Debug)]
struct FakeDesk {
    connected: bool,
    events: std::collections::VecDeque<HubEvent>,
    frame: u64,
}

impl DeskOut for FakeDesk {
    fn send_voice(&mut self, frame: &VoiceFrame) -> bool {
        frame.hz.is_finite()
    }
    fn send_audio(&mut self, frames: u32, samples: &[f32]) -> bool {
        assert_eq!(samples.len(), frames as usize * 2);
        self.frame += u64::from(frames);
        self.connected
    }
    fn is_connected(&self) -> bool {
        self.connected
    }
    fn drain_events(&mut self, on: &mut dyn FnMut(HubEvent)) {
        while let Some(event) = self.events.pop_front() {
            on(event);
        }
    }
    fn stream_frame(&self) -> u64 {
        self.frame
    }
}

struct Rig {
    engine: MouthEngine<FakeDesk>,
    judge: Judge,
    commands: crossbeam_channel::Sender<Command>,
    mic: HeapProd<f32>,
}

impl Rig {
    fn new(connected: bool, settings: &crate::controls::Settings) -> Result<Self> {
        let (mic_tx, mic) = HeapRb::<f32>::new(4096).split();
        let (display, display_rx) = HeapRb::<VoiceFrame>::new(1024).split();
        drop(display_rx);
        let (commands, receiver) = crossbeam_channel::bounded(64);
        let (feed, results, judge) = judge::spawn(
            PitchDetectorConfig::for_rate(RATE),
            BLOCK,
            OnsetConfig::default(),
        )?;
        let engine = MouthEngine::with_settings(
            EngineSetup {
                rate: RATE,
                block: BLOCK,
                mic,
                commands: receiver,
                display,
                desk: FakeDesk {
                    connected,
                    events: std::collections::VecDeque::with_capacity(16),
                    frame: 0,
                },
                judge: (feed, results),
                stats: Arc::new(Stats::default()),
            },
            settings,
        )?;
        Ok(Self {
            engine,
            judge,
            commands,
            mic: mic_tx,
        })
    }

    fn sync(&mut self, bpm: f32, frame: u64) {
        self.engine
            .desk
            .events
            .push_back(HubEvent::TransportSync(TransportSyncMsg {
                state: TRANSPORT_PLAYING,
                bpm,
                at_frame: frame,
                beat: 0.0,
            }));
        self.engine.render(&mut [0.0; BLOCK * 2], 2);
    }

    fn repeat(&mut self, bpm: f64) {
        self.engine.fx.pedals[Slot::Delay as usize].kinds[0]
            .effect
            .reset();
        let context = self.engine.transport(BLOCK);
        assert!((context.bpm - bpm).abs() < f64::EPSILON);
        let mut input = vec![0.0; RATE as usize];
        input[0] = AMP;
        let output = render(&mut self.engine.fx, &input, context.bpm, &self.engine.stats);
        let expected = 60.0 / bpm;
        let peak = peak_at(&output[0], expected);
        assert!(peak.abs_diff((expected * f64::from(RATE)) as usize) <= 48);
        assert!(output[0][peak].abs() > 0.01);
    }

    fn finish(self) {
        drop(self.engine);
        assert!(self.judge.join().is_ok());
    }
}

#[test]
fn the_delay_follows_the_desks_tempo() -> Result<()> {
    let mut settings = crate::controls::Settings {
        fx: only(Slot::Delay, 0),
        ..crate::controls::Settings::default()
    };
    param(&mut settings.fx, Slot::Delay, 0, "lsync", 9.0);
    param(&mut settings.fx, Slot::Delay, 0, "mix", 1.0);
    param(&mut settings.fx, Slot::Delay, 0, "feedback", 0.0);
    let mut rig = Rig::new(true, &settings)?;
    rig.sync(100.0, 0);
    rig.repeat(100.0);
    rig.engine.desk.frame = u64::from(RATE);
    rig.sync(150.0, u64::from(RATE));
    for _ in 0..38 {
        rig.engine.render(&mut [0.0; BLOCK * 2], 2);
    }
    rig.repeat(150.0);
    rig.engine.desk.connected = false;
    rig.engine.render(&mut [0.0; BLOCK * 2], 2);
    assert!((rig.engine.own_bpm - 150.0).abs() < f64::EPSILON);
    assert!(rig.engine.desk_bpm.is_none());
    rig.finish();
    let mut unplugged = Rig::new(false, &settings)?;
    unplugged.repeat(120.0);
    unplugged.finish();
    Ok(())
}

#[test]
fn kinds_switch_without_allocating_or_clicking() {
    let stats = Stats::default();
    let input = sine(RATE as usize);
    for slot in [Slot::Distortion, Slot::Reverb] {
        let mut chain = chain(&only(slot, 0));
        let steady: Vec<_> = (0..slot.ids().len())
            .map(|kind| {
                render(
                    &mut super::tests::chain(&only(slot, kind)),
                    &input,
                    120.0,
                    &stats,
                )
            })
            .collect();
        let mut left = [0.0; BLOCK];
        let mut right = [0.0; BLOCK];
        let mut previous = [0.0_f32; 2];
        let mut largest = 0.0_f32;
        let before = assert_no_alloc::violation_count();
        assert_no_alloc::assert_no_alloc(|| {
            for kind in (1..slot.ids().len()).chain(std::iter::once(0)) {
                chain.kind(slot, kind as u8, &stats);
                for chunk in input[..4800].chunks(BLOCK) {
                    chain.process(
                        Context { bpm: 120.0 },
                        chunk,
                        [&mut left[..chunk.len()], &mut right[..chunk.len()]],
                        &stats,
                    );
                    for (i, side) in [&left[..chunk.len()], &right[..chunk.len()]]
                        .into_iter()
                        .enumerate()
                    {
                        for &sample in side {
                            assert!(sample.is_finite());
                            largest = largest.max((sample - previous[i]).abs());
                            previous[i] = sample;
                        }
                    }
                }
            }
        });
        assert_eq!(assert_no_alloc::violation_count(), before);
        let limit = steady
            .iter()
            .flat_map(|output| output.iter())
            .map(|side| max_step(side))
            .fold(0.0, f32::max)
            * 1.5;
        assert!(largest <= limit, "{}: {largest} > {limit}", slot.name());
    }
}

#[test]
fn a_reorder_swaps_without_a_click() {
    let mut settings = FxSettings::default();
    for slot in &mut settings.slots {
        slot.on = true;
    }
    let input = sine(RATE as usize);
    let stats = Stats::default();
    let old = render(&mut chain(&settings), &input, 120.0, &stats);
    let mut order = Slot::ALL;
    order.swap(1, 2);
    let mut reordered = settings;
    reordered.order = order;
    let new = render(&mut chain(&reordered), &input, 120.0, &stats);
    let limit = bound(&old, &new);
    let mut chain = chain(&settings);
    let warm = render(&mut chain, &input[..24_000], 120.0, &stats);
    chain.reorder(order, &stats);
    let output = render(&mut chain, &input[24_000..24_960], 120.0, &stats);
    assert_eq!(chain.order, order);
    assert!(chain.pedals.iter().all(|slot| !slot.presence.moving()));
    for side in 0..2 {
        assert!((output[side][0] - warm[side][23_999]).abs() <= limit);
        assert!(max_step(&output[side]) <= limit);
    }
    chain.reorder([Slot::Delay; Slot::ALL.len()], &stats);
    assert_eq!(chain.order, order);
    assert_eq!(stats.fx_refused.load(Ordering::Relaxed), 1);
}

#[test]
fn nonsense_in_is_survived() {
    let stats = Stats::default();
    let mut settings = FxSettings::default();
    for slot in &mut settings.slots {
        slot.on = true;
    }
    let mut chain = chain(&settings);
    for slot in Slot::ALL {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            chain.param(slot, 0, 0, value, &stats);
        }
    }
    let input: Vec<_> = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e30]
        .into_iter()
        .cycle()
        .take(4096)
        .collect();
    let output = render(&mut chain, &input, 120.0, &stats);
    assert!(
        output
            .iter()
            .flatten()
            .map(|sample| ceiling(*sample))
            .all(|sample| sample.is_finite() && sample.abs() < 1.0)
    );
    let mut reference = super::tests::chain(&settings);
    let finite_input: Vec<_> = input
        .iter()
        .map(|sample| kazoo_core::sanitize_sample(*sample))
        .collect();
    let conditioned = render(&mut reference, &finite_input, 120.0, &stats);
    assert!(
        conditioned
            .iter()
            .flatten()
            .all(|sample| sample.is_finite())
    );
    let clean = sine(RATE as usize);
    let actual = render(&mut chain, &clean, 120.0, &stats);
    let expected = render(&mut reference, &clean, 120.0, &stats);
    assert!(
        actual
            .iter()
            .flatten()
            .zip(expected.iter().flatten())
            .all(|(a, b)| a.to_bits() == b.to_bits())
    );
}

#[test]
fn the_ceiling_holds_full_scale() {
    let mut settings = FxSettings::default();
    settings.slots[Slot::Distortion as usize].on = true;
    settings.slots[Slot::Delay as usize].on = true;
    let distortion = Slot::Distortion.kind(0).unwrap();
    let level = distortion
        .params
        .iter()
        .find(|spec| spec.name == "level")
        .unwrap()
        .max;
    param(&mut settings, Slot::Distortion, 0, "level", level);
    param(&mut settings, Slot::Delay, 0, "feedback", 0.95);
    param(&mut settings, Slot::Delay, 0, "freeze", 1.0);
    let mut chain = chain(&settings);
    let input: Vec<_> = saw(RATE as usize * 10)
        .into_iter()
        .map(|sample| sample / AMP)
        .collect();
    let output = render(&mut chain, &input, 120.0, &Stats::default());
    for sample in output.iter().flatten() {
        let held = ceiling(*sample);
        assert!(held.abs() < 1.0);
        if sample.abs() <= 0.8 {
            assert_eq!(held.to_bits(), sample.to_bits());
        }
    }
    for value in [f32::MAX, -f32::MAX, 1.0, -1.0] {
        assert!(ceiling(value).abs() < 1.0);
    }
}

#[test]
fn render_with_every_slot_on_never_allocates() -> Result<()> {
    let mut settings = crate::controls::Settings::default();
    for slot in &mut settings.fx.slots {
        slot.on = true;
    }
    let mut rig = Rig::new(false, &settings)?;
    let input = sine(RATE as usize * 10);
    let before = assert_no_alloc::violation_count();
    let mut output = [0.0; BLOCK * 2];
    assert_no_alloc::assert_no_alloc(|| {
        for turn in 1..=4 {
            for slot in Slot::ALL {
                assert!(
                    rig.commands
                        .try_send(Command::FxKind {
                            slot,
                            kind: (turn % slot.ids().len()) as u8
                        })
                        .is_ok()
                );
            }
            let span = input.len() / 4;
            for block in input[(turn - 1) * span..turn * span].chunks(BLOCK) {
                assert_eq!(rig.mic.push_slice(block), block.len());
                rig.engine.render(&mut output[..block.len() * 2], 2);
                assert!(output.iter().all(|sample| sample.is_finite()));
            }
        }
    });
    assert_eq!(assert_no_alloc::violation_count(), before);
    rig.finish();
    Ok(())
}

#[test]
fn every_kind_fits_max_params() {
    for slot in Slot::ALL {
        for kind in 0..slot.ids().len() {
            assert!(slot.kind(kind).unwrap().params.len() <= MAX_PARAMS);
        }
    }
}

#[test]
fn a_bypass_during_a_kind_fade_finishes_both_ramps() {
    let mut chain = chain(&only(Slot::Distortion, 0));
    let stats = Stats::default();
    chain.on(Slot::Distortion, false);
    render(&mut chain, &sine(128), 120.0, &stats);
    chain.kind(Slot::Distortion, 1, &stats);
    render(&mut chain, &sine(1024), 120.0, &stats);
    let slot = &chain.pedals[Slot::Distortion as usize];
    assert!(slot.asleep);
    assert!(slot.fading_from.is_none());
    assert!(!slot.gain.moving());
    assert!(!slot.fade.moving());
    assert_eq!(stats.fx_moving.load(Ordering::Relaxed), 0);
}

#[test]
fn parameters_are_kept_for_each_kind_and_invalid_commands_are_counted() {
    let mut chain = chain(&only(Slot::Distortion, 0));
    let stats = Stats::default();
    let description = Slot::Distortion.kind(0).unwrap();
    let index = description
        .params
        .iter()
        .position(|spec| spec.name == "level")
        .unwrap();
    let value = description.params[index].max;
    chain.param(Slot::Distortion, 0, index as u8, value, &stats);
    chain.kind(Slot::Distortion, 1, &stats);
    render(&mut chain, &sine(128), 120.0, &stats);
    chain.kind(Slot::Distortion, 2, &stats);
    render(&mut chain, &sine(1024), 120.0, &stats);
    chain.kind(Slot::Distortion, 0, &stats);
    render(&mut chain, &sine(1024), 120.0, &stats);
    assert_eq!(
        chain.pedals[Slot::Distortion as usize].kinds[0].values[index].to_bits(),
        value.to_bits()
    );
    chain.kind(Slot::Distortion, u8::MAX, &stats);
    chain.param(Slot::Distortion, u8::MAX, 0, 1.0, &stats);
    chain.param(Slot::Distortion, 0, u8::MAX, 1.0, &stats);
    assert_eq!(stats.fx_refused.load(Ordering::Relaxed), 3);
}

#[test]
fn a_transport_change_lands_on_its_frame_and_stopping_clears_the_beat() -> Result<()> {
    use kazoo_core::ipc::types::TRANSPORT_STOPPED;

    let mut rig = Rig::new(true, &crate::controls::Settings::default())?;
    rig.sync(100.0, 64);
    assert!(rig.engine.beats[..64].iter().all(|beat| beat.is_nan()));
    assert_eq!(rig.engine.beats[64].to_bits(), 0.0_f64.to_bits());
    assert!((rig.engine.beats[65] - 100.0 / (60.0 * f64::from(RATE))).abs() < f64::EPSILON);
    let frame = rig.engine.desk.frame;
    rig.engine
        .desk
        .events
        .push_back(HubEvent::TransportSync(TransportSyncMsg {
            state: TRANSPORT_STOPPED,
            bpm: 150.0,
            at_frame: frame,
            beat: 7.0,
        }));
    rig.engine.render(&mut [0.0; BLOCK * 2], 2);
    assert!(rig.engine.beats.iter().all(|beat| beat.is_nan()));
    assert_eq!(
        rig.engine.stats.desk_bpm.load(Ordering::Relaxed),
        150.0_f64.to_bits()
    );
    rig.engine
        .desk
        .events
        .push_back(HubEvent::TransportSync(TransportSyncMsg {
            state: TRANSPORT_PLAYING,
            bpm: f32::NAN,
            at_frame: frame,
            beat: 0.0,
        }));
    rig.engine.render(&mut [0.0; BLOCK * 2], 2);
    assert_eq!(rig.engine.stats.desk_lost.load(Ordering::Relaxed), 1);
    rig.finish();
    Ok(())
}

fn harmony_slot() -> Slot {
    Slot::ALL
        .into_iter()
        .find(|slot| slot.name() == "harmony")
        .unwrap()
}

fn amplitude(samples: &[f32], hz: f32) -> f32 {
    let (sin, cos) = samples
        .iter()
        .enumerate()
        .fold((0.0, 0.0), |(sin, cos), (i, x)| {
            let phase = TAU * hz * i as f32 / RATE as f32;
            (x.mul_add(phase.sin(), sin), x.mul_add(phase.cos(), cos))
        });
    2.0 * sin.hypot(cos) / samples.len() as f32
}

#[test]
fn harmony_intervals_and_octaves_spectrum() {
    let slot = harmony_slot();
    for (preset, shift, hz) in [
        (0.0, -12.0, 110.0),
        (1.0, 12.0, 440.0),
        (4.0, 7.0, 329.627_56),
    ] {
        let mut settings = only(slot, 0);
        param(&mut settings, slot, 0, "preset", preset);
        param(&mut settings, slot, 0, "dry", -60.0);
        param(&mut settings, slot, 0, "voice 1 shift", shift);
        param(&mut settings, slot, 0, "voice 1 level", 0.0);
        param(&mut settings, slot, 0, "voice 2 level", -60.0);
        let output = render(
            &mut chain(&settings),
            &sine(RATE as usize * 2),
            120.0,
            &Stats::default(),
        );
        let window = &output[0][RATE as usize..];
        assert!(
            amplitude(window, hz) > AMP * 0.5,
            "{hz}: shifted fundamental is too quiet"
        );
        let peak = (0..=200)
            .map(|i| hz * (i as f32).mul_add(0.0002, 0.98))
            .max_by(|a, b| amplitude(window, *a).total_cmp(&amplitude(window, *b)))
            .unwrap();
        assert!((peak / hz - 1.0).abs() < 0.01, "{peak} != {hz}");
    }
    let output = render(
        &mut chain(&only(slot, 0)),
        &sine(RATE as usize * 2),
        120.0,
        &Stats::default(),
    );
    for (hz, db) in [(110.0, 0.0), (220.0, 0.0), (440.0, -3.0)] {
        let actual = 20.0 * (amplitude(&output[0][RATE as usize..], hz) / AMP).log10();
        assert!(
            (actual - db).abs() <= 3.0,
            "{hz}: {actual} dB expected {db}"
        );
    }
}

#[test]
fn harmony_all_voices_allocate_nothing_and_recover_from_nonfinite() {
    let slot = harmony_slot();
    let mut settings = only(slot, 0);
    param(&mut settings, slot, 0, "voice 3 level", -6.0);
    let mut chain = chain(&settings);
    let stats = Stats::default();
    let mut left = [0.0; BLOCK];
    let mut right = [0.0; BLOCK];
    let input = sine(BLOCK);
    let before = assert_no_alloc::violation_count();
    assert_no_alloc::assert_no_alloc(|| {
        for _ in 0..1000 {
            chain.process(
                Context { bpm: 120.0 },
                &input,
                [&mut left, &mut right],
                &stats,
            );
        }
    });
    assert_eq!(before, assert_no_alloc::violation_count());
    let mut fresh = super::tests::chain(&settings);
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        fresh.process(
            Context { bpm: 120.0 },
            &[value; BLOCK],
            [&mut left, &mut right],
            &stats,
        );
        assert!(left.iter().chain(&right).all(|x| *x == 0.0));
    }
    let output = render(&mut fresh, &sine(RATE as usize), 120.0, &stats);
    assert!(output.iter().flatten().all(|x| x.is_finite()));
    assert!(output[0].iter().any(|x| x.abs() > 0.1));
    fresh.process(
        Context { bpm: 120.0 },
        &[f32::NAN; BLOCK],
        [&mut left, &mut right],
        &stats,
    );
    assert!(left.iter().chain(&right).all(|x| *x == 0.0));
}

#[test]
fn harmony_cost_per_block() {
    use std::time::Instant;
    let slot = harmony_slot();
    let input: [f32; BLOCK] =
        std::array::from_fn(|i| (TAU * 220.0 * i as f32 / RATE as f32).sin() * AMP);
    let stats = Stats::default();
    let mut left = [0.0; BLOCK];
    let mut right = [0.0; BLOCK];
    let context = Context { bpm: 120.0 };
    let blocks = if cfg!(debug_assertions) { 32 } else { 1000 };
    for voices in 0..=3 {
        let mut settings = FxSettings::default();
        for saved in &mut settings.slots {
            saved.on = true;
        }
        settings.slots[slot as usize].on = voices > 0;
        for index in 0..3 {
            param(
                &mut settings,
                slot,
                0,
                ["voice 1 level", "voice 2 level", "voice 3 level"][index],
                if index < voices { 0.0 } else { -60.0 },
            );
        }
        let mut chain = chain(&settings);
        let description = slot.kind(0).unwrap();
        let mut harmony = (description.build)();
        harmony.prepare(RATE as f32);
        for (index, value) in settings.slots[slot as usize].values[0]
            .iter()
            .take(description.params.len())
            .enumerate()
        {
            harmony.set_param(index, *value);
        }
        harmony.reset();
        for _ in 0..100 {
            chain.process(context, &input, [&mut left, &mut right], &stats);
            harmony.process(&context, [&input, &input], [&mut left, &mut right]);
        }
        let start = Instant::now();
        for _ in 0..blocks {
            chain.process(context, &input, [&mut left, &mut right], &stats);
        }
        let chain_us = start.elapsed().as_secs_f64() * 1_000_000.0 / f64::from(blocks);
        let start = Instant::now();
        for _ in 0..blocks {
            harmony.process(&context, [&input, &input], [&mut left, &mut right]);
        }
        let harmony_us = start.elapsed().as_secs_f64() * 1_000_000.0 / f64::from(blocks);
        println!(
            "voices={voices} harmony_us={harmony_us:.3} chain_us={chain_us:.3} block_us=2666.667 harmony_percent={:.2}",
            harmony_us / 2666.667 * 100.0
        );
        assert!(left.iter().chain(&right).all(|x| x.is_finite()));
    }
}

/// A click is broadband: a step in level or waveform puts energy where a
/// tone of a few hundred hertz has none. The third difference falls 18 dB
/// per octave below its peak, so it hears a step at full size and the
/// tones under test 60 dB down. A smooth fade shows only at its corners,
/// where the gain's slope turns: at most the signal's peak over the ramp's
/// length.
fn clicks(samples: &[f32]) -> f32 {
    samples
        .windows(4)
        .map(|w| 3.0f32.mul_add(w[1] - w[2], w[3] - w[0]).abs())
        .fold(0.0, f32::max)
}

/// Level in decibels over every window of 800 frames, hopped by 1 ms. 800
/// frames is one whole period of 60 Hz, the lowest partial a 240 Hz tone
/// takes two octaves down, and so of every octave of it: a window that
/// holds a whole number of periods reads a steady tone as steady, where a
/// 2 ms window reads its own waveform's swing as a level change.
fn levels(samples: &[f32]) -> Vec<f32> {
    samples
        .windows(800)
        .step_by(48)
        .map(|w| {
            10.0 * (w.iter().map(|x| x * x).sum::<f32>() / 800.0)
                .max(1e-20)
                .log10()
        })
        .collect()
}

fn tone_240(count: usize) -> Vec<f32> {
    (0..count)
        .map(|i| (TAU * 240.0 * i as f32 / RATE as f32).sin() * AMP)
        .collect()
}

fn harmony_at(preset: usize) -> FxSettings {
    let slot = Slot::Harmony;
    let mut settings = only(slot, 0);
    let saved = &mut settings.slots[slot as usize];
    super::harmony::adjust(&mut saved.values[0], &mut saved.custom, 0, preset as f32);
    settings
}

/// Renders `settings` settled, makes `change`, and renders until settled
/// again; answers (the worst click over what the steady sound and a fade
/// of the whole sound over the ramp allow, the worst level over the louder
/// steady level in dB).
fn transition(settings: &FxSettings, change: impl FnOnce(&mut FxChain, &Stats)) -> (f32, f32) {
    let stats = Stats::default();
    let input = tone_240(RATE as usize);
    let mut chain = chain(settings);
    let before = render(&mut chain, &input[..14_400], 120.0, &stats);
    change(&mut chain, &stats);
    let during = render(&mut chain, &input[14_400..33_600], 120.0, &stats);
    let after = render(&mut chain, &input[33_600..48_000], 120.0, &stats);
    let steady = [&before[0][4_800..], &after[0][..]];
    let peak = [&before[0][..], &during[0], &after[0]]
        .iter()
        .flat_map(|x| x.iter())
        .fold(0.0_f32, |peak, x| peak.max(x.abs()));
    let fade = peak / (RATE as f32 * super::RAMP_SECONDS);
    let click = clicks(&during[0]) / (steady.iter().map(|x| clicks(x)).fold(0.0, f32::max) + fade);
    let loudest = steady
        .iter()
        .flat_map(|x| levels(x))
        .fold(f32::MIN, f32::max);
    let mut joined = before[0][4_800..].to_vec();
    joined.extend_from_slice(&during[0]);
    joined.extend_from_slice(&after[0]);
    let swell = levels(&joined).into_iter().fold(f32::MIN, f32::max) - loudest;
    (click, swell)
}

#[test]
fn harmony_changes_mid_tone_never_click_or_swell() {
    let slot = Slot::Harmony;
    // Each pair moves an audible voice's shift: through unison with the dry
    // (octave up to down, octaves to two down) and across it (fifth to
    // octave down, minor third to octaves, octave down to octave up).
    for (from, to) in [(1, 0), (2, 3), (4, 0), (7, 2), (0, 1)] {
        let (click, swell) = transition(&harmony_at(from), |chain, stats| {
            chain.param(slot, 0, 0, to as f32, stats);
        });
        let name = |index: usize| super::harmony::PRESETS[index];
        assert!(
            click <= 1.0,
            "{} to {}: click {click}",
            name(from),
            name(to)
        );
        assert!(
            swell <= 1.0,
            "{} to {}: swell {swell} dB",
            name(from),
            name(to)
        );
    }
    for (index, value) in [(3, -60.0), (3, 6.0), (2, -11.0), (4, 7.0), (1, -60.0)] {
        let (click, swell) = transition(&harmony_at(2), |chain, stats| {
            chain.param(slot, 0, index, value, stats);
        });
        assert!(click <= 1.0, "knob {index} to {value}: click {click}");
        assert!(swell <= 1.0, "knob {index} to {value}: swell {swell} dB");
    }
    let stats = Stats::default();
    let mut chain = chain(&harmony_at(1));
    let input = tone_240(BLOCK);
    let mut left = [0.0; BLOCK];
    let mut right = [0.0; BLOCK];
    let before = assert_no_alloc::violation_count();
    assert_no_alloc::assert_no_alloc(|| {
        for block in 0..320 {
            if block % 40 == 0 {
                chain.param(slot, 0, 0, (block / 40) as f32, &stats);
            }
            chain.process(
                Context { bpm: 120.0 },
                &input,
                [&mut left, &mut right],
                &stats,
            );
        }
    });
    assert_eq!(before, assert_no_alloc::violation_count());
    for on in [false, true] {
        let mut settings = harmony_at(2);
        settings.slots[slot as usize].on = !on;
        let (click, swell) = transition(&settings, |chain, _| chain.on(slot, on));
        assert!(click <= 1.0, "on {on}: click {click}");
        assert!(swell <= 1.0, "on {on}: swell {swell} dB");
    }
}
