use super::*;
use crate::catalogue::Kind;
use crate::dsp::testing::{Bench, RATE};
use std::sync::atomic::Ordering;

const BAR: usize = 96_000;
const TAIL: usize = 384;

fn bench(bars: f32) -> (Bench, LoopFeed) {
    let mut bench = Bench::seeded(Kind::LOOP, RATE, 0);
    let (module, feed) = build_loop(RATE);
    bench.module = module;
    bench.knob("bars", bars).knob("level", 1.0);
    bench.beat = -32.0 / 24_000.0;
    (bench, feed)
}

fn render(bench: &mut Bench, frames: usize, input: impl Fn(usize) -> f32) -> Vec<f32> {
    let mut result = Vec::with_capacity(frames);
    for base in (0..frames).step_by(crate::SUB_BLOCK) {
        for f in 0..crate::SUB_BLOCK {
            bench.inputs[0][f] = input(base + f);
        }
        bench.connected[0] = true;
        assert_no_alloc::assert_no_alloc(|| bench.step());
        result.extend_from_slice(&bench.outputs[0]);
    }
    result
}

fn recorded(bars: f32, input: impl Fn(usize) -> f32) -> (Bench, LoopFeed) {
    let (mut b, feed) = bench(bars);
    b.knob("record", 1.0);
    render(&mut b, 32, |_| 0.0);
    render(&mut b, BAR * bars as usize, input);
    b.knob("record", 0.0);
    render(&mut b, TAIL, |_| 0.0);
    (b, feed)
}

fn snap(b: &mut Bench, feed: &mut LoopFeed) -> Box<[f32]> {
    feed.snapshot(vec![0.0; 2 * (MAX_LOOP_FRAMES + TAIL + 1)].into_boxed_slice())
        .unwrap();
    loop {
        render(b, 32, |_| 0.0);
        for item in feed.collect() {
            if let LoopOut::Snapped { tape, .. } = item {
                return tape;
            }
        }
    }
}

#[test]
fn nothing_is_recorded_until_the_bar_line() {
    let (mut b, mut feed) = bench(1.0);
    b.beat = 1.0;
    b.knob("record", 1.0);
    assert!(render(&mut b, 72_000, |_| 0.75).iter().all(|x| *x == 0.0));
    render(&mut b, BAR, |f| f as f32 / BAR as f32);
    b.knob("record", 0.0);
    render(&mut b, TAIL, |_| 0.0);
    let tape = snap(&mut b, &mut feed);
    assert!((tape[2 * 48_000] - 0.5).abs() < 1e-6);
}

#[test]
fn a_loop_plays_back_bit_exact_at_its_own_tempo() {
    let signal = |f: usize| ((f % 127) as f32 - 63.0) / 128.0;
    let (mut first, _) = recorded(1.0, signal);
    first.beat = 8.0;
    let output = render(&mut first, BAR, |_| 0.0);
    // Floating beat arithmetic leaves fractional offsets near 1e-9 cells.
    for (f, sample) in output.iter().enumerate().take(BAR - 2).skip(TAIL) {
        assert!((*sample - signal(f)).abs() < 1e-6, "frame {f}");
    }
    let (mut second, _) = recorded(1.0, signal);
    second.beat = 8.0;
    let again = render(&mut second, BAR, |_| 0.0);
    assert_eq!(
        output.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
        again.iter().map(|x| x.to_bits()).collect::<Vec<_>>()
    );
}

#[test]
fn the_seam_does_not_click() {
    let sine = |f: usize| (std::f32::consts::TAU * 220.0 * f as f32 / RATE + 0.7).sin();
    let (mut b, _) = bench(1.0);
    b.knob("record", 1.0);
    render(&mut b, 32, |_| 0.0);
    render(&mut b, BAR, sine);
    b.knob("record", 0.0);
    render(&mut b, TAIL, |f| sine(BAR + f));
    b.beat = 7.99;
    let output = render(&mut b, 4 * BAR, |_| 0.0);
    let largest = output
        .windows(2)
        .map(|p| (p[1] - p[0]).abs())
        .fold(0.0, f32::max);
    assert!(
        largest <= 1.5 * std::f32::consts::TAU * 220.0 / RATE,
        "step {largest}"
    );
}

#[test]
fn punch_out_runs_to_the_bar_line_and_fades() {
    let (mut b, mut feed) = bench(4.0);
    b.knob("record", 1.0);
    render(&mut b, 32, |_| 0.0);
    render(&mut b, 156_000, |_| 0.5);
    b.knob("record", 0.0);
    render(&mut b, 36_000, |_| 0.5);
    assert!(feed.collect().is_empty());
    b.beat = 8.0;
    render(&mut b, TAIL, |_| 0.5);
    let marks = feed.collect();
    let mark = marks
        .iter()
        .find_map(|item| match item {
            LoopOut::Took(mark) => Some(mark),
            _ => None,
        })
        .unwrap();
    assert!(mark.bars[0] && mark.bars[1]);
    assert!(!mark.bars[3]);
    assert_eq!(mark.frame, 32 + 192_000 + 383);
    let tape = snap(&mut b, &mut feed);
    assert_eq!(tape[2 * 180_000].to_bits(), 0.5_f32.to_bits());
    assert!(tape[2 * (192_000 + 100)] > tape[2 * (192_000 + 300)]);
}

#[test]
fn overdub_keeps_keep_of_what_was_there() {
    let (mut b, _) = recorded(1.0, |_| 0.5);
    b.knob("record", 1.0).knob("keep", 0.5);
    b.beat = 7.99;
    render(&mut b, BAR + 256, |_| 0.25);
    b.knob("record", 0.0);
    render(&mut b, BAR + TAIL, |_| 0.25);
    b.beat = 16.0 + 2.0;
    assert!((render(&mut b, 32, |_| 0.0)[0] - 0.5).abs() < 1e-6);
}

#[test]
fn wear_darkens_and_quietens_each_pass_and_zero_wear_is_exact() {
    let (mut b, _) = recorded(1.0, |_| 0.5);
    b.knob("wear", 1.0);
    let mut rms = Vec::new();
    for pass in 1..=3 {
        b.beat = f64::from(pass).mul_add(4.0, 1.0);
        let out = render(&mut b, 24_000, |_| 0.0);
        rms.push((out[1000..].iter().map(|x| x * x).sum::<f32>() / 23_000.0).sqrt());
    }
    for pair in rms.windows(2) {
        assert!((20.0_f32.mul_add((pair[1] / pair[0]).log10(), 3.0)).abs() < 0.2);
    }
    b.knob("wear", 0.0);
    b.beat = 20.0 + 1.0;
    let first = render(&mut b, 32, |_| 0.0);
    b.beat = 24.0 + 1.0;
    assert_eq!(first, render(&mut b, 32, |_| 0.0));
    let high = |f: usize| (std::f32::consts::TAU * 12_000.0 * f as f32 / RATE).sin() * 0.5;
    let (mut dark, _) = recorded(1.0, high);
    dark.beat = 5.0;
    let fresh = render(&mut dark, 24_000, |_| 0.0);
    dark.knob("wear", 1.0);
    dark.beat = 13.0;
    let worn = render(&mut dark, 24_000, |_| 0.0);
    let rms = |samples: &[f32]| {
        (samples[1000..]
            .iter()
            .map(|sample| sample * sample)
            .sum::<f32>()
            / 23_000.0)
            .sqrt()
    };
    // Remove the specified 6 dB level loss after the post-roll marks pass one to measure the additional top loss.
    assert!(rms(&worn) * 10.0_f32.powf(6.0 / 20.0) < rms(&fresh) * 0.95);
}

#[test]
fn the_loop_follows_the_beat_not_the_clock() {
    let sine = |f: usize| (std::f32::consts::TAU * 1000.0 * f as f32 / RATE).sin();
    let (mut b, _) = recorded(2.0, sine);
    b.beat = 8.0 + 1.0;
    let first = render(&mut b, 1024, |_| 0.0);
    render(&mut b, BAR - 1024, |_| 0.0);
    b.beat = 8.0 + 1.0;
    assert_eq!(first, render(&mut b, 1024, |_| 0.0));
    b.bpm = 240.0;
    b.beat = 16.0 + 1.0;
    let out = render(&mut b, 24_000, |_| 0.0);
    let crossings = out.windows(2).filter(|p| p[0] <= 0.0 && p[1] > 0.0).count();
    assert!((990..=1010).contains(&crossings), "crossings {crossings}");
    b.bpm = 0.0;
    let stopped = render(&mut b, 64, |_| 0.0);
    assert!(stopped.iter().all(|x| x.to_bits() == stopped[0].to_bits()));
}

#[test]
fn unequal_loops_drift_and_meet_again() {
    let (mut a, _) = bench(5.0);
    let (mut b, _) = bench(7.0);
    a.knob("record", 1.0);
    b.knob("record", 1.0);
    render(&mut a, 32, |_| 0.0);
    render(&mut b, 32, |_| 0.0);
    let mut together = Vec::new();
    let mut previous = [false; 2];
    for frame in (0..36 * BAR).step_by(32) {
        if frame == BAR {
            a.knob("record", 0.0);
            b.knob("record", 0.0);
        }
        render(&mut a, 32, |_| 0.5);
        render(&mut b, 32, |_| 0.5);
        for f in 0..32 {
            let pulses = [a.outputs[2][f] > 0.5, b.outputs[2][f] > 0.5];
            if pulses[0] && pulses[1] && !previous[0] && !previous[1] && frame + f > 0 {
                together.push(frame + f);
            }
            previous = pulses;
        }
    }
    assert_eq!(together.len(), 1);
    assert!(together[0].abs_diff(35 * BAR) <= 1);
    let mut first = None;
    for bar in 1..=391 {
        let beat = f64::from(bar) * 4.0;
        let a = head(beat, 0.0, 68.0).unwrap();
        let b = head(beat, 0.0, 92.0).unwrap();
        if a.phase == 0.0 && b.phase == 0.0 {
            first = Some(bar);
            break;
        }
    }
    assert_eq!(first, Some(391));
}

#[test]
fn too_long_a_loop_is_shortened_to_whole_bars_that_fit() {
    let (mut b, feed) = bench(64.0);
    b.bpm = 40.0;
    b.beat = -32.0 / 72_000.0;
    b.knob("record", 1.0);
    render(&mut b, 64, |_| 0.5);
    assert_eq!(
        feed.gauges().beats.load(Ordering::Relaxed),
        64.0_f64.to_bits()
    );
    assert!(feed.gauges().shortened.load(Ordering::Relaxed));
}

#[test]
fn a_snapshot_is_the_tape_at_the_frame_it_began() {
    let (mut b, mut feed) = bench(1.0);
    let (mut reference, mut reference_feed) = bench(1.0);
    for target in [&mut b, &mut reference] {
        target.knob("record", 1.0).knob("keep", 0.7);
        render(target, 32, |_| 0.0);
        render(target, 3 * BAR + BAR / 2, |f| (f % 97) as f32 / 100.0);
    }
    reference.module.reset();
    let expected = snap(&mut reference, &mut reference_feed);
    feed.snapshot(vec![0.0; expected.len()].into_boxed_slice())
        .unwrap();
    // A fast transport revisits uncopied cells repeatedly during the copy.
    b.bpm = 120_000.0;
    let actual = loop {
        render(&mut b, 32, |_| 0.9);
        if let Some(tape) = feed.collect().into_iter().find_map(|item| match item {
            LoopOut::Snapped { tape, .. } => Some(tape),
            _ => None,
        }) {
            break tape;
        }
    };
    assert_eq!(
        &actual[..2 * (BAR + TAIL + 1)],
        &expected[..2 * (BAR + TAIL + 1)]
    );
}

#[test]
fn install_hands_the_old_tape_back_and_never_drops_it() {
    let (mut b, mut feed) = bench(1.0);
    for _ in 0..FEED_SLOTS {
        feed.install(
            Vec::new().into_boxed_slice(),
            None,
            [i64::MIN; MAX_LOOP_BARS],
            [0; LOOP_BINS],
            false,
        )
        .unwrap();
        render(&mut b, 32, |_| 0.0);
    }
    let replacement = vec![0.0; 2 * (BAR + TAIL + 1)].into_boxed_slice();
    let pointer = replacement.as_ptr();
    feed.install(
        replacement,
        None,
        [i64::MIN; MAX_LOOP_BARS],
        [0; LOOP_BINS],
        false,
    )
    .unwrap();
    render(&mut b, 32, |_| 0.0);
    assert_eq!(feed.collect().len(), FEED_SLOTS);
    render(&mut b, 32, |_| 0.0);
    assert!(!feed.collect().is_empty());
    feed.install(
        Vec::new().into_boxed_slice(),
        None,
        [i64::MIN; MAX_LOOP_BARS],
        [0; LOOP_BINS],
        false,
    )
    .unwrap();
    render(&mut b, 32, |_| 0.0);
    assert!(
        feed.collect()
            .iter()
            .any(|item| matches!(item, LoopOut::Retired(tape) if tape.as_ptr() == pointer))
    );
}

#[test]
fn nan_in_never_reaches_the_tape_and_reset_keeps_it() {
    let (mut b, mut feed) = bench(1.0);
    b.knob("record", 1.0);
    render(&mut b, 32, |_| 0.0);
    render(&mut b, BAR, |f| {
        if f > 48_000 {
            if f % 2 == 0 { f32::NAN } else { f32::INFINITY }
        } else {
            0.5
        }
    });
    b.module.reset();
    b.beat = 8.0 + 1.0;
    assert_eq!(render(&mut b, 32, |_| 0.0)[0].to_bits(), 0.5_f32.to_bits());
    let tape = snap(&mut b, &mut feed);
    assert!(tape.iter().all(|x| x.is_finite()));
    assert_eq!(tape[2 * 60_000].to_bits(), 0.0_f32.to_bits());
}

#[test]
fn no_memory_is_a_silent_loop_that_says_so() {
    let (mut b, _) = bench(1.0);
    let (module, feed) = LoopModule::with_capacity(0);
    b.module = Box::new(module);
    b.knob("record", 1.0);
    assert!(render(&mut b, 64, |_| 0.5).iter().all(|x| *x == 0.0));
    assert_eq!(
        feed.gauges().state.load(Ordering::Relaxed),
        LoopState::NoTape as u8
    );
}

/// The largest sample-to-sample step of `samples`.
fn largest_step(samples: &[f32]) -> f32 {
    samples
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0, f32::max)
}

/// One bar of a 220 Hz sine, begun mid-cycle, recorded and punched out.
fn sine_bar() -> (Bench, impl Fn(usize) -> f32) {
    let sine = |f: usize| (std::f32::consts::TAU * 220.0 * f as f32 / RATE + 0.7).sin();
    let (mut b, _) = bench(1.0);
    b.knob("record", 1.0);
    render(&mut b, 32, |_| 0.0);
    render(&mut b, BAR, sine);
    b.knob("record", 0.0);
    render(&mut b, TAIL, |f| sine(BAR + f));
    (b, sine)
}

#[test]
fn the_seam_does_not_click_when_the_tempo_moves() {
    // Off the splice tempo the head reads the last cells at fractions: they
    // must lean into the tail, the end's continuation, not into the
    // faded-in start of the loop.
    for bpm in [131.0, 150.0, 173.0] {
        let (mut b, _) = sine_bar();
        b.bpm = bpm;
        b.beat = 7.99;
        // Four passes at this tempo.
        let frames = (4.0 * 4.0 * 60.0 / bpm * f64::from(RATE)) as usize;
        let output = render(&mut b, frames, |_| 0.0);
        let sine_step = std::f32::consts::TAU * 220.0 * (bpm / 120.0) as f32 / RATE;
        let largest = largest_step(&output);
        assert!(
            largest <= 1.5 * sine_step,
            "{bpm} BPM: step {largest}, sine {sine_step}"
        );
    }
}

#[test]
fn the_seam_does_not_click_as_the_tape_wears() {
    let (mut b, _) = sine_bar();
    b.knob("wear", 1.0);
    b.beat = 7.99;
    let output = render(&mut b, 4 * BAR, |_| 0.0);
    let sine_step = std::f32::consts::TAU * 220.0 / RATE;
    let largest = largest_step(&output);
    assert!(
        largest <= 1.5 * sine_step,
        "step {largest}, sine {sine_step}"
    );
}

#[test]
fn the_head_is_always_on_the_tape_and_agrees_with_itself() {
    // A beat a hair before the anchor: `rem_euclid` rounds up to the whole
    // loop, which would put the head one cell past the end (silence).
    let h = head(-1e-17, 0.0, 4.0).unwrap();
    assert!(h.phase < 1.0, "phase {}", h.phase);
    assert_eq!(h.pass, 0);
    // Around every seam of loops of every length, the pass and the phase
    // put the head back where the beat says.
    for beats in 4..=259_u32 {
        let beats = f64::from(beats);
        for pass in [1.0_f64, 2.0, 17.0, 391.0, 100_003.0] {
            let seam = pass * beats;
            for rel in [
                f64::from_bits(seam.to_bits() - 1),
                seam,
                f64::from_bits(seam.to_bits() + 1),
            ] {
                let h = head(rel, 0.0, beats).unwrap();
                assert!((0.0..1.0).contains(&h.phase), "{rel} {beats}");
                let back = h.phase.mul_add(beats, h.pass as f64 * beats);
                assert!(
                    (back - rel).abs() <= 1e-9 * rel.abs().max(1.0),
                    "{rel} {beats}: {h:?}"
                );
            }
        }
    }
}

#[test]
fn a_loop_built_without_a_feed_keeps_recording_after_many_punches() {
    // The catalogue's builder (what `wall_add loop` puts up) has no feed:
    // nobody will ever collect its marks, and that must not stop it.
    let mut b = Bench::seeded(Kind::LOOP, RATE, 0);
    b.knob("bars", 1.0).knob("level", 1.0);
    b.beat = -32.0 / 24_000.0;
    for _ in 0..13 {
        b.knob("record", 1.0);
        render(&mut b, BAR, |_| 0.0);
        b.knob("record", 0.0);
        render(&mut b, BAR + TAIL + 64, |_| 0.0);
    }
    b.knob("record", 1.0);
    render(&mut b, BAR, |_| 0.5);
    b.knob("record", 0.0);
    render(&mut b, 2 * BAR, |_| 0.5);
    let out = render(&mut b, BAR, |_| 0.0);
    assert!(
        out.iter().any(|x| (*x - 0.5).abs() < 1e-3),
        "the last punch was not recorded"
    );
}

#[test]
fn an_install_ends_the_punch_it_interrupts() {
    let (mut b, mut feed) = bench(4.0);
    b.knob("record", 1.0);
    render(&mut b, 32, |_| 0.0);
    render(&mut b, BAR + BAR / 2, |_| 0.5);
    let splice = Splice {
        beats: 16.0,
        frames: 4 * BAR,
        tail: TAIL,
        anchor: 0.0,
        rate: RATE,
    };
    feed.install(
        vec![0.0; 2 * (4 * BAR + TAIL + 1)].into_boxed_slice(),
        Some(splice),
        [i64::MIN; MAX_LOOP_BARS],
        [0; LOOP_BINS],
        false,
    )
    .unwrap();
    b.knob("record", 0.0);
    render(&mut b, 64, |_| 0.0);
    // Record bar 3 alone on the new tape.
    b.beat = 11.0;
    b.knob("record", 1.0);
    render(&mut b, BAR / 2 + 1024, |_| 0.25);
    b.knob("record", 0.0);
    render(&mut b, BAR + TAIL, |_| 0.25);
    let marks: Vec<TakeMark> = feed
        .collect()
        .into_iter()
        .filter_map(|item| match item {
            LoopOut::Took(mark) => Some(mark),
            _ => None,
        })
        .collect();
    let last = marks.last().expect("a take for bar 3");
    assert!(last.bars[3], "{:?}", &last.bars[..4]);
    assert!(
        !last.bars[0] && !last.bars[1],
        "bars of the retired tape counted in the new take: {:?}",
        &last.bars[..4]
    );
}

#[test]
fn the_post_roll_marks_no_bar_of_its_own() {
    // The 8 ms fade after the bar line belongs to the punch that ended:
    // the take names bars 0 and 1, not bar 2 where the fade fell.
    let (mut b, mut feed) = bench(4.0);
    b.knob("record", 1.0);
    render(&mut b, 32, |_| 0.0);
    render(&mut b, 156_000, |_| 0.5);
    b.knob("record", 0.0);
    render(&mut b, 36_000 + TAIL + 64, |_| 0.5);
    let mark = feed
        .collect()
        .into_iter()
        .find_map(|item| match item {
            LoopOut::Took(mark) => Some(mark),
            _ => None,
        })
        .expect("a take");
    assert_eq!(&mark.bars[..4], &[true, true, false, false]);
}

#[test]
fn a_post_roll_into_the_tail_does_not_make_the_first_bar_younger() {
    // Two bars recorded in one pass are the same age on every later pass,
    // so at full wear they play equally loud: the post-roll that ran into
    // the tail at the next pass must not count as writing bar 0 again.
    let (mut b, _) = recorded(2.0, |_| 0.5);
    b.knob("wear", 1.0);
    let rms = |samples: &[f32]| {
        (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt()
    };
    for pass in 2..=3 {
        b.beat = f64::from(pass) * 8.0 + 1.0;
        let first = render(&mut b, BAR / 2, |_| 0.0);
        b.beat = f64::from(pass) * 8.0 + 5.0;
        let second = render(&mut b, BAR / 2, |_| 0.0);
        let (first, second) = (rms(&first[1000..]), rms(&second[1000..]));
        assert!(
            (first - second).abs() <= 1e-4 * second,
            "pass {pass}: bar 0 {first}, bar 1 {second}"
        );
    }
}
