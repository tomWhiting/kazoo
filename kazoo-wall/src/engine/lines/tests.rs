//! Delay line tests: delays land where asked, and a change of delay never
//! breaks the signal.

use super::*;

const RATE: f32 = 48_000.0;

/// A route on line 3, owned by tag 7, with `delay` frames.
fn route(delay: u16, gate: bool) -> Route {
    Route {
        delay,
        gate,
        ..Route::new(0, 0, 1.0, 3, 7)
    }
}

/// Run `signal` through `lines`, a sub-block at a time, with the route
/// `route_at(frame)` for each sub-block starting at `frame`.
fn run(
    lines: &mut Lines,
    frames: usize,
    signal: impl Fn(usize) -> f32,
    route_at: impl Fn(usize) -> Route,
) -> Vec<f32> {
    let mut out = Vec::with_capacity(frames);
    let mut frame = 0;
    while frame < frames {
        let mut source = [0.0; SUB_BLOCK];
        for (i, sample) in source.iter_mut().enumerate() {
            *sample = signal(frame + i);
        }
        let mut block = [0.0; SUB_BLOCK];
        lines.carry(&route_at(frame), &source, &mut block);
        out.extend_from_slice(&block);
        frame += SUB_BLOCK;
    }
    out
}

#[test]
fn a_delay_holds_the_signal_back_exactly() {
    for delay in [0, 1, 31, 32, 43, 70, 1_024] {
        let mut lines = Lines::new(RATE);
        let out = run(
            &mut lines,
            4_096,
            |i| if i == 100 { 1.0 } else { 0.0 },
            |_| route(delay, false),
        );
        let at = out.iter().position(|s| *s != 0.0).unwrap();
        assert_eq!(at, 100 + usize::from(delay), "delay {delay}");
        assert_eq!(out.iter().filter(|s| **s != 0.0).count(), 1);
    }
}

#[test]
fn the_amount_scales_what_arrives() {
    let mut lines = Lines::new(RATE);
    let half = Route {
        amount: -0.5,
        ..route(0, false)
    };
    let out = run(&mut lines, 32, |_| 1.0, |_| half);
    assert!(out.iter().all(|s| (*s + 0.5).abs() < 1e-7));
}

fn sine(hz: f32, rate: f32) -> impl Fn(usize) -> f32 {
    move |i| (std::f32::consts::TAU * hz * i as f32 / rate).sin() * 0.8
}

fn largest_step(samples: &[f32]) -> f32 {
    samples
        .windows(2)
        .fold(0.0_f32, |m, w| m.max((w[1] - w[0]).abs()))
}

/// RMS over every `window` frames (a whole number of the tone's cycles),
/// in dB against a 0.8 sine's.
fn levels_db(samples: &[f32], window: usize) -> Vec<f32> {
    let reference = 0.8 / 2.0_f32.sqrt();
    samples
        .windows(window)
        .step_by(8)
        .map(|w| {
            let rms = (w.iter().map(|s| s * s).sum::<f32>() / w.len() as f32).sqrt();
            20.0 * (rms / reference).log10()
        })
        .collect()
}

/// Move a tone's delay from `from` to `to` frames a second in, at `rate`,
/// and check it never steps further than the tone's own step played 2%
/// fast, and holds its level within half a decibel throughout. `window`
/// frames hold a whole number of the tone's cycles.
fn moves_smoothly(from: u16, to: u16, hz: f32, window: usize, rate: f32) {
    let tone = sine(hz, rate);
    let mut lines = Lines::new(rate);
    let second = rate as usize;
    // Long enough for the longest glide (51 200 frames for 1 024) to end.
    let frames = second + 60_000;
    let out = run(&mut lines, frames, &tone, |frame| {
        route(if frame < second { from } else { to }, false)
    });
    let own: Vec<f32> = (0..frames).map(&tone).collect();
    let bound = largest_step(&own).mul_add(1.02, 1e-5);
    let heard = largest_step(&out[second / 2..]);
    assert!(
        heard <= bound,
        "{from} → {to} at {hz} Hz: {heard} > {bound}"
    );
    for level in levels_db(&out[second / 2..], window) {
        assert!(level.abs() < 0.5, "{from} → {to} at {hz} Hz: {level} dB");
    }
}

#[test]
fn a_delay_moves_without_a_step_a_dip_or_a_bump() {
    // (tone, frames holding whole cycles of it)
    for (hz, window) in [(50.0, 960), (1_000.0, 48), (5_000.0, 48)] {
        for moved in [5, 24, 43, 480, 1_024] {
            moves_smoothly(0, moved, hz, window, RATE);
            moves_smoothly(moved, 0, hz, window, RATE);
        }
        moves_smoothly(300, 780, hz, window, RATE);
    }
}

#[test]
fn a_low_rate_still_moves_no_more_than_two_percent() {
    // At 8 kHz, 2 s is 16 000 frames: gliding 1 024 frames at 50 per
    // frame takes longer, and does.
    moves_smoothly(0, 1_024, 50.0, 160, 8_000.0);
    moves_smoothly(1_024, 0, 50.0, 160, 8_000.0);
}

/// How many frames after it was asked the reading point reaches a delay
/// moved from `from` to `to`.
fn glide_frames(from: u16, to: u16) -> usize {
    // A steep ramp, never flat: a point still a fraction of a frame from
    // its new delay reads measurably wrong. (A sine's crest would read
    // right a frame early, its slope there being nearly nothing.)
    let tone = |frame: usize| (frame % 256) as f32 / 256.0;
    let mut lines = Lines::new(RATE);
    let start = 48_000;
    let out = run(&mut lines, start + 60_000, tone, |frame| {
        route(if frame < start { from } else { to }, false)
    });
    let arrived = |frame: usize| (out[frame] - tone(frame - usize::from(to))).abs() < 1e-6;
    // The last frame not yet reading the new delay, counted from the ask.
    let last_off = (start..out.len())
        .rev()
        .find(|frame| !arrived(*frame))
        .unwrap_or(start);
    last_off + 1 - start
}

#[test]
fn a_glide_lasts_fifty_frames_for_every_frame_moved() {
    // At least 5 ms (240 frames); 50 frames per frame moved past that.
    for (from, to, frames) in [(0, 1, 240), (1, 0, 240), (0, 43, 2_150), (1_024, 0, 51_200)] {
        let took = glide_frames(from, to);
        assert!(
            took.abs_diff(frames) <= 1,
            "{from} → {to}: {took} frames, not {frames}"
        );
    }
}

#[test]
fn a_change_during_a_glide_turns_the_glide_round() {
    let tone = sine(50.0, RATE);
    let mut lines = Lines::new(RATE);
    // 0 → 40 at one second, then → 80 64 frames later, mid-glide.
    let out = run(&mut lines, 96_000, &tone, |frame| {
        route(
            match frame {
                f if f < 48_000 => 0,
                f if f < 48_064 => 40,
                _ => 80,
            },
            false,
        )
    });
    let own: Vec<f32> = (0..96_000).map(&tone).collect();
    assert!(largest_step(&out[24_000..]) <= largest_step(&own).mul_add(1.02, 1e-5));
    for level in levels_db(&out[24_000..], 960) {
        assert!(level.abs() < 0.5, "{level} dB");
    }
    // It ends reading 80 frames back.
    for (frame, got) in out.iter().enumerate().skip(95_000) {
        assert!((got - tone(frame - 80)).abs() < 1e-6);
    }
}

/// Rising edges of a gate.
fn edges(samples: &[f32]) -> usize {
    samples
        .windows(2)
        .filter(|w| w[0] <= GATE_HIGH && w[1] > GATE_HIGH)
        .count()
}

/// The length of every whole high run (not the one cut off at the end).
fn high_runs(samples: &[f32]) -> Vec<usize> {
    let mut length = 0;
    let mut lengths = Vec::new();
    for sample in samples {
        if *sample > GATE_HIGH {
            length += 1;
        } else if length > 0 {
            lengths.push(length);
            length = 0;
        }
    }
    lengths
}

/// Only 0 and 1 ever come out of a gate: no half-faded gate.
fn only_zeroes_and_ones(samples: &[f32]) -> bool {
    samples
        .iter()
        .all(|s| s.to_bits() == 0.0_f32.to_bits() || s.to_bits() == 1.0_f32.to_bits())
}

/// Run `gate` with its delay moving from `from` to `to` a second in.
fn move_gate(gate: impl Fn(usize) -> f32, from: u16, to: u16) -> Vec<f32> {
    let mut lines = Lines::new(RATE);
    run(&mut lines, 96_000, gate, |frame| {
        route(if frame < 48_016 { from } else { to }, true)
    })
}

#[test]
fn gates_move_without_making_or_losing_an_edge() {
    // A gate high for 300 frames of every 1 000.
    let gate = |i: usize| if i % 1_000 < 300 { 1.0 } else { 0.0 };
    let whole = edges(&(0..96_000).map(gate).collect::<Vec<_>>());
    for (from, to) in [(0_u16, 43_u16), (43, 0), (0, 700), (700, 0)] {
        let out = move_gate(gate, from, to);
        assert!(only_zeroes_and_ones(&out));
        let runs = high_runs(&out);
        assert!(
            runs.iter().all(|length| *length == 300),
            "{from} → {to}: {runs:?}"
        );
        assert!(edges(&out).abs_diff(whole) <= 1, "{from} → {to}");
        // It ends on the new delay.
        for (frame, sample) in out.iter().enumerate().take(96_000).skip(95_000) {
            assert_eq!(sample.to_bits(), gate(frame - usize::from(to)).to_bits());
        }
    }
}

#[test]
fn a_square_moved_by_half_its_period_still_moves() {
    // Old and new reading points are never both low: the gate is held low
    // until the new one is, skipping a pulse rather than breaking one.
    for (period, from, to) in [(86, 0, 43), (1_000, 0, 500), (86, 43, 0)] {
        let gate = move |i: usize| if i % period < period / 2 { 1.0 } else { 0.0 };
        let out = move_gate(gate, from, to);
        assert!(only_zeroes_and_ones(&out));
        let runs = high_runs(&out);
        assert!(
            runs.iter().all(|length| *length == period / 2),
            "{period}: {runs:?}"
        );
        for (frame, sample) in out.iter().enumerate().take(96_000).skip(95_000) {
            assert_eq!(
                sample.to_bits(),
                gate(frame - usize::from(to)).to_bits(),
                "{period}: {from} → {to} never arrived"
            );
        }
    }
}

#[test]
fn a_move_taken_back_forgets_how_long_it_waited() {
    // High for 3 000 frames, then pulses of 600 in every 1 000. A move
    // asked while it is high and taken back before it switches must not
    // leave a count behind that lets the next move switch mid-pulse.
    let gate = |i: usize| {
        if i < 3_000 || (i >= 4_000 && (i - 4_000) % 1_000 < 600) {
            1.0
        } else {
            0.0
        }
    };
    let mut lines = Lines::new(RATE);
    let out = run(&mut lines, 12_000, gate, |frame| {
        let delay = match frame {
            f if f < 64 => 0,
            // Asked while high, for most of a line's length...
            f if f < 2_016 => 40,
            // ...taken back...
            f if f < 5_216 => 0,
            // ...and a new move during a pulse.
            _ => 300,
        };
        route(delay, true)
    });
    assert!(only_zeroes_and_ones(&out));
    let runs = high_runs(&out);
    assert!(runs[1..].iter().all(|length| *length == 600), "{runs:?}");
}

#[test]
fn a_new_cable_on_a_line_starts_clean() {
    let mut lines = Lines::new(RATE);
    run(&mut lines, 4_096, |_| 1.0, |_| route(500, false));
    // The line changes hands: nothing of the old cable comes through.
    let newcomer = Route {
        tag: 8,
        ..route(500, false)
    };
    let out = run(&mut lines, 400, |_| 0.25, |_| newcomer);
    assert!(out.iter().all(|s| *s == 0.0));
}

#[test]
fn nothing_but_numbers_come_out() {
    let mut lines = Lines::new(RATE);
    let wild = |i: usize| match i % 4 {
        0 => 3e38,
        1 => -3e38,
        2 => f32::NAN,
        _ => f32::INFINITY,
    };
    let out = run(&mut lines, 96_000, wild, |frame| {
        route(if frame < 1_000 { 0 } else { 777 }, false)
    });
    assert!(out.iter().all(|s| s.is_finite()));
}

#[test]
fn carrying_never_allocates() {
    let mut lines = Lines::new(RATE);
    let source = [0.5; SUB_BLOCK];
    let mut out = [0.0; SUB_BLOCK];
    let before = assert_no_alloc::violation_count();
    assert_no_alloc::assert_no_alloc(|| {
        for step in 0..1_000_u16 {
            lines.carry(&route(step % 200, step % 3 == 0), &source, &mut out);
        }
    });
    assert_eq!(assert_no_alloc::violation_count(), before);
}
