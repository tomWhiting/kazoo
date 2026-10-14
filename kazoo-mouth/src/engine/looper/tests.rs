use std::f32::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Producer, Split};

use super::{BAR_CHOICES, LoopFeed, LoopLength, LoopOut, Looper, Note, State};
use crate::engine::{Command, DeskOut, EngineSetup, MouthEngine, Stats};

const RATE: u32 = 48_000;
const N: usize = 48_000;
const BLOCK: usize = 128;

fn rig() -> (Looper, LoopFeed) {
    let (mut looper, feed) = Looper::new(RATE, LoopLength::Free, 1.0);
    looper.volume.snap(1.0);
    (looper, feed)
}

fn render(looper: &mut Looper, input: &[f32], bpm: f64) -> Vec<f32> {
    let mut output = vec![0.0; input.len()];
    let beats = [f64::NAN; BLOCK];
    for (live, out) in input.chunks(BLOCK).zip(output.chunks_mut(BLOCK)) {
        looper.process(live, &beats[..live.len()], bpm, out);
    }
    output
}

fn constant(looper: &mut Looper, sample: f32, count: usize) -> Vec<f32> {
    render(looper, &vec![sample; count], 120.0)
}

fn take(sample: f32) -> (Looper, LoopFeed) {
    let (mut looper, feed) = rig();
    looper.command(Command::LoopRecord, 120.0);
    constant(&mut looper, sample, N);
    looper.command(Command::LoopRecord, 120.0);
    let tail = looper.tail;
    constant(&mut looper, sample, tail);
    (looper, feed)
}

fn dub(looper: &mut Looper, sample: f32, frames: usize) {
    looper.command(Command::LoopRecord, 120.0);
    constant(looper, sample, frames);
    looper.command(Command::LoopRecord, 120.0);
    constant(looper, sample, looper.tail);
}

fn sine(count: usize, offset: usize) -> Vec<f32> {
    (0..count)
        .map(|cell| (TAU * 220.0 * (cell + offset) as f32 / RATE as f32).sin() * 0.25)
        .collect()
}

fn maximum_step(samples: &[f32]) -> f32 {
    samples
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0, f32::max)
}

fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
}

fn sine_table() -> [f32; 64] {
    std::array::from_fn(|cell| (cell as f32 / 63.0 * std::f32::consts::FRAC_PI_2).sin())
}

fn quarter_sine(table: &[f32; 64], fraction: f32) -> f32 {
    let position = fraction * 63.0;
    let cell = position as usize;
    let next = (cell + 1).min(63);
    (table[next] - table[cell]).mul_add(position - cell as f32, table[cell])
}

#[test]
fn a_free_take_closes_on_the_second_press_and_plays_from_the_top() {
    let (mut looper, feed) = rig();
    let input: Vec<_> = (0..N).map(|cell| cell as f32 / N as f32).collect();
    looper.command(Command::LoopRecord, 120.0);
    render(&mut looper, &input, 120.0);
    looper.command(Command::LoopRecord, 120.0);
    assert_eq!(looper.n, Some(N));
    assert_eq!(looper.head, 0);
    let output = constant(&mut looper, 0.0, N * 4);
    for pass in output.chunks_exact(N) {
        for (actual, expected) in pass[looper.tail..].iter().zip(&input[looper.tail..]) {
            assert_eq!(actual.to_bits(), expected.to_bits());
        }
    }
    assert_eq!(feed.gauges.n.load(Ordering::Relaxed), N);
}

#[test]
fn the_seam_is_faded_once_and_baked() {
    let table = sine_table();
    let (mut looper, _) = rig();
    let input = sine(N + looper.tail, 37);
    looper.command(Command::LoopRecord, 120.0);
    render(&mut looper, &input[..N], 120.0);
    assert_eq!(&looper.buffers[0][..N], &input[..N]);
    looper.command(Command::LoopRecord, 120.0);
    let tail = looper.tail;
    render(&mut looper, &input[N..N + tail - 1], 120.0);
    assert_eq!(&looper.buffers[0][..N], &input[..N]);
    assert_eq!(&looper.buffers[0][N..N + tail - 1], &input[N..N + tail - 1]);
    render(&mut looper, &input[N + tail - 1..], 120.0);
    for cell in 0..tail {
        let phase = cell as f32 / tail as f32;
        let expected = quarter_sine(&table, phase).mul_add(
            input[cell],
            quarter_sine(&table, 1.0 - phase) * input[N + cell],
        );
        assert_eq!(looper.buffers[0][cell].to_bits(), expected.to_bits());
    }
    assert_eq!(&looper.buffers[0][tail..N], &input[tail..N]);
}

#[test]
fn a_sustained_sound_does_not_dip_at_the_seam() {
    let (mut looper, _) = rig();
    let mut seed = 0x1234_5678_u32;
    let noise: Vec<_> = (0..N * 3 / 2)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed as f32 / u32::MAX as f32).mul_add(2.0, -1.0) * 0.251_188_64
        })
        .collect();
    looper.command(Command::LoopRecord, 120.0);
    render(&mut looper, &noise[..N], 120.0);
    looper.command(Command::LoopRecord, 120.0);
    let tail = looper.tail;
    render(&mut looper, &noise[N..N + tail], 120.0);
    looper.head = 0;
    let output = constant(&mut looper, 0.0, N * 4);
    for pass in output.chunks_exact(N) {
        let db = 20.0 * (rms(&pass[..tail]) / rms(&pass[tail..tail + 4800])).log10();
        assert!(db.abs() <= 1.0, "seam RMS difference {db} dB");
    }
}

#[test]
fn the_seam_does_not_click() {
    let (mut looper, _) = rig();
    let input = sine(N + looper.tail, 37);
    let bound = maximum_step(&input) * 1.5;
    looper.command(Command::LoopRecord, 120.0);
    render(&mut looper, &input[..N], 120.0);
    looper.command(Command::LoopRecord, 120.0);
    render(&mut looper, &input[N..], 120.0);
    let output = constant(&mut looper, 0.0, N * 4);
    assert!(
        maximum_step(&output) <= bound,
        "step {} bound {bound}",
        maximum_step(&output)
    );
}

#[test]
fn the_first_pass_fades_in_under_the_live_voice() {
    let table = sine_table();
    let (mut looper, _) = rig();
    looper.command(Command::LoopRecord, 120.0);
    constant(&mut looper, 0.5, N);
    looper.command(Command::LoopRecord, 120.0);
    let tail = looper.tail;
    let output = constant(&mut looper, 0.5, tail);
    for (cell, actual) in output.iter().enumerate() {
        assert_eq!(
            actual.to_bits(),
            (0.5 * quarter_sine(&table, cell as f32 / tail as f32)).to_bits()
        );
    }
}

#[test]
fn an_overdub_is_a_layer_and_undo_takes_it_away() {
    let (mut looper, _) = take(0.25);
    looper.head = 1000;
    dub(&mut looper, 0.125, N);
    assert_eq!(looper.layers, 2);
    assert!((looper.read(10_000) - 0.375).abs() < f32::EPSILON);
    looper.command(Command::LoopUndo, 120.0);
    let tail = looper.tail;
    constant(&mut looper, 0.0, tail);
    assert_eq!(looper.layers, 1);
    assert_eq!(looper.read(10_000).to_bits(), 0.25_f32.to_bits());
}

#[test]
fn a_short_overdub_is_heard_only_where_it_was_sung() {
    let (mut looper, _) = take(0.25);
    dub(&mut looper, 0.5, N);
    looper.command(Command::LoopUndo, 120.0);
    let tail = looper.tail;
    constant(&mut looper, 0.0, tail);
    looper.head = 2000;
    dub(&mut looper, 0.125, 9600);
    assert!((looper.read(5000) - 0.375).abs() < f32::EPSILON);
    for cell in 12_000..N {
        assert_eq!(looper.read(cell).to_bits(), 0.25_f32.to_bits());
    }
}

#[test]
fn punches_fade_in_and_out() {
    let (mut looper, _) = take(0.25);
    looper.head = 1000;
    looper.command(Command::LoopRecord, 120.0);
    let tail = looper.tail;
    constant(&mut looper, 1.0, tail + 1000);
    for cell in 0..tail {
        assert_eq!(
            looper.buffers[1][1000 + cell].to_bits(),
            (cell as f32 / tail as f32).to_bits()
        );
    }
    let end = looper.head;
    looper.command(Command::LoopRecord, 120.0);
    constant(&mut looper, 1.0, tail);
    for cell in 0..tail {
        assert_eq!(
            looper.buffers[1][end + cell].to_bits(),
            (1.0 - cell as f32 / tail as f32).to_bits()
        );
    }
    let (mut looper, _) = rig();
    let wave = sine(N + looper.tail, 0);
    looper.command(Command::LoopRecord, 120.0);
    render(&mut looper, &wave[..N], 120.0);
    looper.command(Command::LoopRecord, 120.0);
    render(&mut looper, &wave[N..], 120.0);
    looper.head = 1200;
    looper.command(Command::LoopRecord, 120.0);
    let start = looper.head;
    let tail = looper.tail;
    let live: Vec<_> = sine(tail + 2400, start)
        .into_iter()
        .map(|sample| sample * 0.25)
        .collect();
    render(&mut looper, &live, 120.0);
    looper.command(Command::LoopRecord, 120.0);
    let live: Vec<_> = sine(tail, looper.head)
        .into_iter()
        .map(|sample| sample * 0.25)
        .collect();
    render(&mut looper, &live, 120.0);
    looper.head = start - 1;
    let heard = constant(&mut looper, 0.0, tail + 2402);
    assert!(maximum_step(&heard[..tail + 2]) <= maximum_step(&wave) * 1.5);
    let end = tail + 2400;
    assert!(maximum_step(&heard[end..]) <= maximum_step(&wave) * 1.5);
}

#[test]
fn a_fourth_layer_merges_the_oldest_two_and_nothing_is_heard_to_change() {
    let (mut looper, _) = take(0.1);
    dub(&mut looper, 0.2, N);
    dub(&mut looper, 0.3, N);
    let expected: Vec<_> = (0..N).map(|cell| looper.read(cell)).collect();
    looper.command(Command::LoopRecord, 120.0);
    assert_eq!(looper.merge, Some(0));
    let zero = [0.0; BLOCK];
    let beats = [f64::NAN; BLOCK];
    let mut output = [0.0; BLOCK];
    while looper.merge.is_some() {
        let start = looper.head;
        looper.process(&zero, &beats, 120.0, &mut output);
        for (index, sample) in output.iter().enumerate() {
            assert!((*sample - expected[(start + index) % N]).abs() < 1e-6);
        }
    }
    assert_eq!(looper.layers, 3);
    assert_eq!(looper.stack[0].buffer, 0);
    assert_eq!(looper.stack[1].buffer, 2);
    looper.command(Command::LoopRecord, 120.0);
    let tail = looper.tail;
    constant(&mut looper, 0.0, tail);
    for remaining in [2, 1, 0] {
        looper.command(Command::LoopUndo, 120.0);
        constant(&mut looper, 0.0, tail);
        assert_eq!(looper.layers, remaining);
        if remaining == 1 {
            assert!((looper.read(10_000) - 0.3).abs() < 1e-6);
        }
    }
    let (mut looper, feed) = take(0.1);
    looper.n = Some(N * 2);
    looper.stack[0].written = N * 2;
    looper.buffers[0][..N * 2].fill(0.1);
    dub(&mut looper, 0.2, N * 2);
    dub(&mut looper, 0.3, N * 2);
    looper.command(Command::LoopRecord, 120.0);
    looper.command(Command::LoopUndo, 120.0);
    constant(&mut looper, 0.0, tail);
    looper.command(Command::LoopUndo, 120.0);
    constant(&mut looper, 0.0, tail);
    looper.command(Command::LoopUndo, 120.0);
    assert_eq!(
        feed.gauges.note.load(Ordering::Relaxed),
        Note::Merging as u8
    );
    looper.head = 10_000;
    let before = looper.read(looper.head);
    looper.command(Command::LoopClear, 120.0);
    let output = constant(&mut looper, 0.0, tail);
    assert!(before.mul_add(-(1.0 - 1.0 / tail as f32), output[0]).abs() < 1e-6);
    assert_eq!(looper.state, State::Blank);
}

#[test]
fn a_merge_takes_half_a_second_at_any_callback_size() {
    for chunk in [64, 128, 1024, 4096] {
        let (mut looper, _) = rig();
        looper.n = Some(N * 32);
        looper.layers = 4;
        looper.state = State::Playing;
        looper.stack = std::array::from_fn(|buffer| super::Layer {
            buffer: buffer as u8,
            start: 0,
            written: N * 32,
        });
        looper.merge = Some(0);
        let live = vec![0.0; chunk];
        let beats = vec![f64::NAN; chunk];
        let mut output = vec![0.0; chunk];
        let mut frames = 0;
        while looper.merge.is_some() {
            looper.process(&live, &beats, 120.0, &mut output);
            frames += chunk;
        }
        assert!(frames <= 24_000 + chunk);
        assert_eq!(looper.layers, 3);
    }
}

#[test]
fn a_punch_waits_for_a_merge_and_never_writes_into_it() {
    let (mut looper, _) = take(0.1);
    dub(&mut looper, 0.2, N);
    dub(&mut looper, 0.3, N);
    looper.command(Command::LoopRecord, 120.0);
    looper.state = State::Playing;
    looper.command(Command::LoopRecord, 120.0);
    assert_eq!(looper.state, State::Waiting);
    let merging_buffer = usize::from(looper.stack[1].buffer);
    let before = looper.buffers[merging_buffer].clone();
    let frames = N.div_ceil(64);
    constant(&mut looper, 1.0, frames - 1);
    assert_eq!(looper.state, State::Waiting);
    assert_eq!(looper.buffers[merging_buffer], before);
    constant(&mut looper, 1.0, 1);
    assert_eq!(looper.state, State::Dubbing);
    assert_eq!(looper.stack[looper.layers - 1].written, 1);
    let recorded = looper.stack[looper.layers - 1].start;
    for (cell, sample) in looper.buffers[merging_buffer].iter().enumerate() {
        if cell != recorded {
            assert_eq!(sample.to_bits(), before[cell].to_bits());
        }
    }
}

#[test]
fn bars_mode_takes_its_bars_from_the_desks_bar_line() {
    let (mut looper, _) = rig();
    looper.command(Command::LoopLength(LoopLength::Bars(1)), 120.0);
    let mut out = [0.0; BLOCK];
    looper.process(&[0.25; BLOCK], &[2.5; BLOCK], 120.0, &mut out);
    looper.command(Command::LoopRecord, 120.0);
    let until_bar = ((4.0 - 2.6) * 24_000.0_f64).round() as usize;
    let beats: Vec<_> = (0..=until_bar + 96_000)
        .map(|frame| 2.6 + frame as f64 / 24_000.0)
        .collect();
    let live = vec![0.25; beats.len()];
    let mut output = vec![0.0; beats.len()];
    looper.process(
        &live[..until_bar],
        &beats[..until_bar],
        120.0,
        &mut output[..until_bar],
    );
    assert_eq!(looper.stack[0].written, 0);
    looper.process(
        &live[until_bar..],
        &beats[until_bar..],
        120.0,
        &mut output[until_bar..],
    );
    assert_eq!(looper.n, Some(96_000));
    assert_eq!(looper.anchor, Some(4.0));
    assert_eq!(looper.state, State::Playing);
    assert_eq!(looper.head, 1);
}

fn anchored_sine() -> Looper {
    let (mut looper, _) = rig();
    looper.length = LoopLength::Bars(1);
    looper.n = Some(96_000);
    looper.anchor = Some(4.0);
    looper.loop_bpm = 120.0;
    looper.layers = 1;
    looper.stack[0].written = 96_000;
    looper.state = State::Playing;
    looper.buffers[0][..96_000].copy_from_slice(&sine(96_000, 0));
    looper
}

#[test]
fn bars_mode_follows_a_desk_locate_without_a_click() {
    let mut looper = anchored_sine();
    looper.previous_beat = 9.3;
    looper.head = 31_200;
    let tail = looper.tail;
    let beats: Vec<_> = (0..=tail)
        .map(|frame| 6.0 + frame as f64 / 24_000.0)
        .collect();
    let mut output = vec![0.0; beats.len()];
    looper.process(&vec![0.0; beats.len()], &beats, 120.0, &mut output);
    assert_eq!(looper.head, 48_000 + tail + 1);
    assert!(looper.locate.is_none());
    assert!(maximum_step(&output) <= maximum_step(&sine(N, 0)) * 1.5);
}

#[test]
fn bars_mode_without_a_desk_uses_the_mouths_tempo() {
    let (mut looper, _) = rig();
    looper.command(Command::LoopLength(LoopLength::Bars(2)), 100.0);
    looper.command(Command::LoopRecord, 100.0);
    render(&mut looper, &vec![0.25; 230_400], 100.0);
    assert_eq!(looper.n, Some(230_400));
    assert_eq!(looper.anchor, None);
    assert_eq!(looper.state, State::Playing);
    assert!((looper.loop_bpm - 100.0).abs() < f64::EPSILON);
}

#[test]
fn a_tempo_change_keeps_the_loops_pitch_and_says_off_tempo() {
    let mut looper = anchored_sine();
    looper.previous_beat = f64::NAN;
    // Equal tape positions avoid counting different boundary crossings in half passes.
    let frames = looper.n.unwrap();
    let before = render(&mut looper, &vec![0.0; frames], 120.0);
    let after = render(&mut looper, &vec![0.0; frames], 128.0);
    assert_eq!(before, after);
    let crossings = |samples: &[f32]| {
        samples
            .windows(2)
            .filter(|pair| pair[0] <= 0.0 && pair[1] > 0.0)
            .count()
    };
    assert!((crossings(&before) as f64 / crossings(&after) as f64 - 1.0).abs() <= 0.001);
    assert!(looper.gauges.off_tempo.load(Ordering::Relaxed));
}

#[test]
fn a_length_that_does_not_fit_is_refused() {
    let (mut looper, feed) = rig();
    looper.command(Command::LoopLength(LoopLength::Bars(16)), 40.0);
    looper.command(Command::LoopRecord, 40.0);
    assert_eq!(looper.state, State::Blank);
    assert_eq!(looper.n, None);
    assert_eq!(
        feed.gauges.note.load(Ordering::Relaxed),
        Note::DoesNotFit as u8
    );
    let mut app = crate::controls::App::new(
        kazoo_core::voice::quantise::QuantiseSettings::default(),
        false,
    );
    app.loop_feed = Some(feed);
    app.accepted.own_bpm = 40.0;
    app.accepted.loop_length = LoopLength::Bars(4);
    let (sender, receiver) = crossbeam_channel::bounded(4);
    app.handle_key(
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('l'),
            crossterm::event::KeyModifiers::NONE,
        ),
        &sender,
        std::time::Instant::now(),
    )
    .unwrap();
    assert!(matches!(
        receiver.try_recv().unwrap(),
        Command::LoopLength(LoopLength::Free)
    ));
    assert!(
        app.notice(std::time::Instant::now())
            .unwrap()
            .contains("8 bars")
    );
    assert!(
        app.notice(std::time::Instant::now())
            .unwrap()
            .contains("16 bars")
    );
    assert_eq!(BAR_CHOICES, [1, 2, 4, 8, 16]);
}

#[derive(Debug)]
struct TestDesk;

impl DeskOut for TestDesk {
    fn send_voice(&mut self, _: &kazoo_core::voice::VoiceFrame) -> bool {
        true
    }
    fn send_audio(&mut self, _: u32, _: &[f32]) -> bool {
        false
    }
    fn is_connected(&self) -> bool {
        false
    }
    fn drain_events(&mut self, _: &mut dyn FnMut(kazoo_core::host::link::HubEvent)) {}
    fn stream_frame(&self) -> u64 {
        0
    }
}

fn engine() -> color_eyre::Result<(
    MouthEngine<TestDesk>,
    kazoo_core::voice::judge::Judge,
    ringbuf::HeapProd<f32>,
)> {
    let (mic_tx, mic) = HeapRb::<f32>::new(4096).split();
    let (display, _) = HeapRb::new(1024).split();
    let (_, commands) = crossbeam_channel::bounded(64);
    let (feed, results, judge) = kazoo_core::voice::judge::spawn(
        kazoo_core::analysis::PitchDetectorConfig::for_rate(RATE),
        BLOCK,
        kazoo_core::voice::judge::OnsetConfig::default(),
    )?;
    let engine = MouthEngine::new(EngineSetup {
        rate: RATE,
        block: BLOCK,
        mic,
        commands,
        display,
        desk: TestDesk,
        judge: (feed, results),
        stats: Arc::new(Stats::default()),
    })?;
    Ok((engine, judge, mic_tx))
}

#[test]
fn the_loop_is_recorded_before_the_effects_and_played_through_them() -> color_eyre::Result<()> {
    let (mut engine, judge, mut mic) = engine()?;
    engine.looper.volume.snap(1.0);
    engine.fx.on(crate::engine::fx::Slot::Reverb, true);
    engine.looper.command(Command::LoopRecord, 120.0);
    let mut out = [0.0; BLOCK * 2];
    let input = sine(N, 0);
    for live in input.chunks(BLOCK) {
        assert_eq!(mic.push_slice(live), live.len());
        engine.render(&mut out[..live.len() * 2], 2);
    }
    // Inspect the final synth block before the loop has started playing.
    let written = engine.looper.stack[0].written;
    assert_eq!(
        &engine.looper.buffers[0][written - BLOCK..written],
        &engine.mono[..BLOCK]
    );
    engine.looper.command(Command::LoopRecord, 120.0);
    for _ in 0..4 {
        engine.render(&mut out, 2);
    }
    assert!(
        engine.looper.buffers[0][..written]
            .iter()
            .any(|sample| sample.abs() > 0.001)
    );
    for _ in 0..400 {
        engine.render(&mut out, 2);
    }
    let mut settings = crate::engine::fx::FxSettings::default();
    settings.slots[crate::engine::fx::Slot::Reverb as usize].on = true;
    let mut reference = crate::engine::fx::FxChain::new(RATE, BLOCK, &settings)?;
    let mut left = [0.0; BLOCK];
    let mut right = [0.0; BLOCK];
    // Fresh chains receive the same loop and share neither live input nor reverb tails.
    engine.fx = crate::engine::fx::FxChain::new(RATE, BLOCK, &settings)?;
    engine.render(&mut out, 2);
    reference.process(
        kazoo_fx::Context { bpm: 120.0 },
        &engine.loop_output,
        [&mut left, &mut right],
        &engine.stats,
    );
    for cell in 0..BLOCK {
        assert!((engine.mono[cell] - engine.loop_output[cell]).abs() < 1e-6);
        assert!((out[cell * 2] - crate::engine::fx::ceiling(left[cell])).abs() < 1e-6);
        assert!((out[cell * 2 + 1] - crate::engine::fx::ceiling(right[cell])).abs() < 1e-6);
    }
    drop(engine);
    judge.join().map_err(|panic| {
        drop(panic);
        color_eyre::eyre::eyre!("judge panicked")
    })?;
    Ok(())
}

#[test]
fn the_loop_never_records_itself() {
    let (mut looper, _) = take(0.25);
    dub(&mut looper, 0.0, N);
    assert!(
        looper.buffers[1][..N]
            .iter()
            .all(|sample| sample.to_bits() == 0)
    );
    assert_eq!(looper.read(10_000).to_bits(), 0.25_f32.to_bits());
}

#[test]
fn volume_stop_and_clear_ramp_without_clicks() {
    let (mut looper, _) = take(0.25);
    looper.head = 1000;
    looper.command(Command::LoopVolume(0.0), 120.0);
    let output = constant(&mut looper, 0.0, 480);
    assert!(maximum_step(&output) <= 0.25 / 480.0 + 1e-6);
    assert_eq!(output.last().unwrap().to_bits(), 0);
    looper.volume.snap(1.0);
    looper.command(Command::LoopPlay(false), 120.0);
    let tail = looper.tail;
    let output = constant(&mut looper, 0.0, tail);
    assert!(maximum_step(&output) <= 0.25 / tail as f32 + 1e-6);
    assert_eq!(looper.state, State::Stopped);
    let held = looper.head;
    constant(&mut looper, 0.0, BLOCK);
    assert_eq!(looper.head, held);
    looper.command(Command::LoopPlay(true), 120.0);
    assert_eq!(looper.head, 0);
    constant(&mut looper, 0.0, tail);
    looper.command(Command::LoopClear, 120.0);
    let output = constant(&mut looper, 0.0, tail);
    assert!(maximum_step(&output) <= 0.25 / tail as f32 + 1e-6);
    assert_eq!(looper.state, State::Blank);
    assert_eq!(looper.n, None);
    let (mut looper, _) = rig();
    let wave = sine(N + looper.tail, 0);
    let bound = maximum_step(&wave) * 1.5;
    looper.command(Command::LoopRecord, 120.0);
    render(&mut looper, &wave[..N], 120.0);
    looper.command(Command::LoopRecord, 120.0);
    render(&mut looper, &wave[N..], 120.0);
    looper.head = 1000;
    for (command, frames) in [
        (Command::LoopVolume(0.0), 480),
        (Command::LoopVolume(1.0), 480),
        (Command::LoopPlay(false), looper.tail),
        (Command::LoopPlay(true), looper.tail),
        (Command::LoopClear, looper.tail),
    ] {
        let previous = constant(&mut looper, 0.0, 1)[0];
        looper.command(command, 120.0);
        let mut heard = vec![previous];
        heard.extend(constant(&mut looper, 0.0, frames));
        assert!(
            maximum_step(&heard) <= bound,
            "{command:?}: {} > {bound}",
            maximum_step(&heard)
        );
    }
}

#[test]
fn a_bounce_is_the_loop_with_its_seam_and_comes_back_without_a_free() {
    let (mut looper, mut feed) = take(0.25);
    dub(&mut looper, 0.125, N);
    looper.volume.snap(0.3);
    let expected: Vec<_> = (0..N).map(|cell| looper.read(cell)).collect();
    for _ in 0..4 {
        assert!(
            looper
                .out
                .try_push(LoopOut::Refused(vec![0.0; 1].into_boxed_slice()))
                .is_ok()
        );
    }
    assert!(
        feed.bounce_in
            .try_push(vec![0.0; N].into_boxed_slice())
            .is_ok()
    );
    let live = [0.0; BLOCK];
    let beats = [f64::NAN; BLOCK];
    let mut output = [0.0; BLOCK];
    let before = assert_no_alloc::violation_count();
    assert_no_alloc::assert_no_alloc(|| {
        for _ in 0..8 {
            looper.process(&live, &beats, 120.0, &mut output);
        }
    });
    assert_eq!(assert_no_alloc::violation_count(), before);
    assert!(looper.held.iter().any(Option::is_some));
    for _ in 0..4 {
        drop(feed.out.try_pop().unwrap());
    }
    assert_no_alloc::assert_no_alloc(|| looper.process(&live, &beats, 120.0, &mut output));
    let LoopOut::Bounced {
        samples,
        rate,
        layers,
    } = feed.out.try_pop().unwrap()
    else {
        panic!("bounce refused");
    };
    assert_eq!(rate, RATE);
    assert_eq!(layers, 2);
    assert_eq!(&*samples, &expected);
    assert_eq!(assert_no_alloc::violation_count(), before);
}

#[test]
fn a_change_during_a_bounce_refuses_it_back() {
    let (mut looper, mut feed) = take(0.25);
    assert!(
        feed.bounce_in
            .try_push(vec![0.0; N].into_boxed_slice())
            .is_ok()
    );
    constant(&mut looper, 0.0, 1);
    assert!(looper.bounce.is_some());
    looper.command(Command::LoopUndo, 120.0);
    assert!(matches!(feed.out.try_pop().unwrap(), LoopOut::Refused(_)));
    assert_eq!(
        feed.gauges.note.load(Ordering::Relaxed),
        Note::SaveCancelled as u8
    );
    assert!(
        feed.bounce_in
            .try_push(vec![0.0; N - 1].into_boxed_slice())
            .is_ok()
    );
    constant(&mut looper, 0.0, 1);
    assert!(matches!(feed.out.try_pop().unwrap(), LoopOut::Refused(_)));
}

#[test]
fn nonsense_in_never_reaches_the_tape() {
    let (mut looper, _) = rig();
    let input: Vec<_> = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e30]
        .into_iter()
        .cycle()
        .take(N)
        .collect();
    looper.command(Command::LoopRecord, 120.0);
    render(&mut looper, &input, 120.0);
    looper.command(Command::LoopRecord, 120.0);
    let tail = looper.tail;
    constant(&mut looper, 0.0, tail);
    looper.command(Command::LoopRecord, 120.0);
    let output = render(&mut looper, &input, 120.0);
    assert!(output.iter().all(|sample| sample.is_finite()));
    for buffer in &looper.buffers {
        assert!(
            buffer
                .iter()
                .all(|sample| sample.is_finite() && sample.abs() <= 4.0)
        );
    }
}

#[test]
fn render_with_the_looper_never_allocates() -> color_eyre::Result<()> {
    let (mut engine, judge, mut mic) = engine()?;
    let mut feed = engine.take_loop_feed().unwrap();
    let mut bounce = Some(vec![0.0; 48_128].into_boxed_slice());
    let mut output = [0.0; BLOCK * 2];
    let input = sine(N * 10, 0);
    let before = assert_no_alloc::violation_count();
    assert_no_alloc::assert_no_alloc(|| {
        for block in 0..3750 {
            assert_eq!(
                mic.push_slice(&input[block * BLOCK..(block + 1) * BLOCK]),
                BLOCK
            );
            match block {
                0 | 376 | 400 | 800 | 850 | 1250 | 1300 | 1700 => {
                    engine.looper.command(Command::LoopRecord, 120.0);
                }
                1800 => engine.looper.command(Command::LoopUndo, 120.0),
                2000 => assert!(feed.bounce_in.try_push(bounce.take().unwrap()).is_ok()),
                2600 => engine.looper.command(Command::LoopClear, 120.0),
                _ => {}
            }
            engine.render(&mut output, 2);
        }
    });
    assert_eq!(assert_no_alloc::violation_count(), before);
    assert!(output.iter().all(|sample| sample.is_finite()));
    assert_eq!(engine.looper.state, State::Blank);
    drop(engine);
    judge.join().map_err(|panic| {
        drop(panic);
        color_eyre::eyre::eyre!("judge panicked")
    })?;
    Ok(())
}

#[test]
fn no_memory_is_a_looper_that_says_so() {
    let (mut looper, feed) = Looper::with_capacity(RATE, 0);
    assert_eq!(looper.state, State::NoTape);
    for command in [
        Command::LoopRecord,
        Command::LoopUndo,
        Command::LoopClear,
        Command::LoopLength(LoopLength::Bars(1)),
        Command::LoopPlay(true),
        Command::LoopVolume(1.0),
    ] {
        looper.command(command, 120.0);
        assert_eq!(looper.state, State::NoTape);
        assert_eq!(feed.gauges.note.load(Ordering::Relaxed), Note::NoTape as u8);
    }
    assert!(
        constant(&mut looper, 1.0, BLOCK)
            .iter()
            .all(|sample| sample.to_bits() == 0)
    );
}
