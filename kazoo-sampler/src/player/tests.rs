//! Player tests: every mode, pitch, loops, envelopes, stealing, swapping
//! and bad input.

use super::*;
use crate::SampleName;

const RATE: f32 = 48_000.0;

fn sample(frames: usize, f: impl Fn(usize) -> f32) -> Arc<SampleData> {
    let left: Vec<f32> = (0..frames).map(&f).collect();
    Arc::new(
        SampleData::new(
            SampleName::new("t").unwrap(),
            RATE as u32,
            left.clone(),
            left,
        )
        .unwrap(),
    )
}

fn sine(hz: f32, frames: usize) -> Arc<SampleData> {
    sample(frames, |n| {
        (std::f32::consts::TAU * hz * n as f32 / RATE).sin() * 0.5
    })
}

/// A player with `data` loaded and its knobs set.
fn player(data: &Arc<SampleData>, knobs: &[(usize, f32)]) -> (SamplePlayer, SampleReaper) {
    let (mut player, reaper) = SamplePlayer::new();
    player.prepare(RATE);
    for &(knob, value) in knobs {
        player.set_param(knob, value);
    }
    player.load(Arc::clone(data)).unwrap();
    let (mut l, mut r) = ([0.0; 1], [0.0; 1]);
    player.process(&PlayerInputs::default(), [&mut l, &mut r]);
    (player, reaper)
}

/// Render `frames` frames with the gate from `gate(n)` and the pitch
/// input from `pitch(n)`, in blocks of 64.
fn render(
    player: &mut SamplePlayer,
    frames: usize,
    gate: impl Fn(usize) -> f32,
    pitch: impl Fn(usize) -> f32,
) -> (Vec<f32>, Vec<f32>) {
    let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
    let mut gates = [0.0f32; 64];
    let mut pitches = [0.0f32; 64];
    for (block, (l, r)) in left.chunks_mut(64).zip(right.chunks_mut(64)).enumerate() {
        for i in 0..l.len() {
            gates[i] = gate(block * 64 + i);
            pitches[i] = pitch(block * 64 + i);
        }
        let inputs = PlayerInputs {
            gate: &gates[..l.len()],
            pitch: &pitches[..l.len()],
            velocity: &[],
        };
        player.process(&inputs, [l, r]);
    }
    (left, right)
}

fn held(_: usize) -> f32 {
    1.0
}

fn flat(_: usize) -> f32 {
    0.0
}

fn frequency(signal: &[f32]) -> f32 {
    let mut crossings = Vec::new();
    for n in 1..signal.len() {
        let (a, b) = (signal[n - 1], signal[n]);
        if a < 0.0 && b >= 0.0 {
            crossings.push((n - 1) as f32 + a / (a - b));
        }
    }
    let span = crossings[crossings.len() - 1] - crossings[0];
    (crossings.len() - 1) as f32 * RATE / span
}

fn peak(signal: &[f32]) -> f32 {
    signal.iter().fold(0.0, |m, x| m.max(x.abs()))
}

fn biggest_step(signal: &[f32]) -> f32 {
    signal
        .windows(2)
        .fold(0.0, |m, w| m.max((w[1] - w[0]).abs()))
}

#[test]
fn every_mode_renders_finite_and_bounded() {
    let data = sine(220.0, 24_000);
    for mode in 0..6 {
        for reverse in [0.0, 1.0] {
            let (mut p, _reaper) = player(
                &data,
                &[(index::MODE, mode as f32), (index::REVERSE, reverse)],
            );
            let (l, r) = render(&mut p, 48_000, |n| if n < 30_000 { 1.0 } else { 0.0 }, flat);
            assert!(l.iter().chain(&r).all(|x| x.is_finite()), "mode {mode}");
            assert!(peak(&l) < 2.0 && peak(&r) < 2.0, "mode {mode}");
            assert!(peak(&l) > 0.05, "mode {mode} made no sound");
        }
    }
}

#[test]
fn an_octave_up_doubles_the_frequency() {
    let data = sine(440.0, 48_000);
    let (mut p, _reaper) = player(&data, &[(index::PITCH, 12.0)]);
    let (l, _) = render(&mut p, 20_000, held, flat);
    let hz = frequency(&l[1_000..20_000]);
    assert!((hz - 880.0).abs() < 0.5, "{hz}");

    // The V/oct input does the same, and fine tune adds cents.
    let (mut p, _reaper) = player(&data, &[(index::FINE, 100.0)]);
    let (l, _) = render(&mut p, 20_000, held, |_| -1.0);
    let hz = frequency(&l[1_000..20_000]);
    let wanted = 220.0 * 2f32.powf(1.0 / 12.0);
    assert!((hz - wanted).abs() < 0.5, "{hz}");
}

#[test]
fn the_held_voice_follows_the_pitch_input() {
    let data = sine(440.0, 96_000);
    let (mut p, _reaper) = player(&data, &[(index::MODE, 1.0)]);
    let (l, _) = render(&mut p, 30_000, held, |n| if n < 10_000 { 0.0 } else { 1.0 });
    let hz = frequency(&l[12_000..30_000]);
    assert!((hz - 880.0).abs() < 1.0, "{hz}");
}

#[test]
fn a_loop_crossfade_leaves_no_discontinuity() {
    // A loop that is not a whole number of cycles clicks at the wrap
    // without a crossfade.
    let data = sine(220.0, 48_000);
    let knobs = |fade: f32| {
        [
            (index::MODE, 2.0),
            (index::LOOP_START, 0.3),
            (index::LOOP_END, 0.4123),
            (index::CROSSFADE, fade),
        ]
    };
    let natural = 0.5 * std::f32::consts::TAU * 220.0 / RATE;
    let (mut p, _reaper) = player(&data, &knobs(0.0));
    let (hard, _) = render(&mut p, 48_000, held, flat);
    assert!(biggest_step(&hard[2_000..]) > 10.0 * natural);
    let (mut p, _reaper) = player(&data, &knobs(0.02));
    let (soft, _) = render(&mut p, 48_000, held, flat);
    assert!(
        biggest_step(&soft[2_000..]) < 1.5 * natural,
        "{}",
        biggest_step(&soft[2_000..])
    );

    // Looping from the very start fades using what follows the loop end.
    let (mut p, _reaper) = player(
        &data,
        &[
            (index::MODE, 2.0),
            (index::LOOP_START, 0.0),
            (index::LOOP_END, 0.1123),
            (index::CROSSFADE, 0.02),
        ],
    );
    let (soft, _) = render(&mut p, 48_000, held, flat);
    assert!(
        biggest_step(&soft[2_000..]) < 1.5 * natural,
        "{}",
        biggest_step(&soft[2_000..])
    );
}

#[test]
fn ping_pong_turns_without_jumping() {
    let data = sine(220.0, 48_000);
    let (mut p, _reaper) = player(
        &data,
        &[
            (index::MODE, 3.0),
            (index::LOOP_START, 0.2),
            (index::LOOP_END, 0.25),
        ],
    );
    let (l, _) = render(&mut p, 48_000, held, flat);
    let natural = 0.5 * std::f32::consts::TAU * 220.0 / RATE;
    assert!(biggest_step(&l[2_000..]) < 1.5 * natural);
    assert!(peak(&l[40_000..]) > 0.1, "still playing after many turns");
}

#[test]
fn a_released_gate_fades_out() {
    let data = sample(96_000, |_| 0.5);
    let (mut p, _reaper) = player(&data, &[(index::MODE, 1.0), (index::RELEASE, 0.1)]);
    let (l, _) = render(&mut p, 30_000, |n| if n < 10_000 { 1.0 } else { 0.0 }, flat);
    let at_release = l[9_999];
    assert!(at_release > 0.3);
    assert!((l[10_000] - at_release).abs() < 0.01, "no jump at release");
    assert!(l[10_000..15_000].windows(2).all(|w| w[1] <= w[0] + 1e-6));
    assert!(peak(&l[15_000..]) < 1e-3, "silent after the release time");
    assert_eq!(p.active_voices(), 0);
}

#[test]
fn one_shot_plays_through_a_short_gate() {
    let data = sample(4_800, |_| 0.5);
    let (mut p, _reaper) = player(&data, &[]);
    let (l, _) = render(&mut p, 9_600, |n| if n < 10 { 1.0 } else { 0.0 }, flat);
    assert!(l[4_000] > 0.3);
    assert!(l[4_799].abs() < 0.05, "fades into the end");
    assert!(peak(&l[4_800..]) < 1e-6);
}

#[test]
fn reverse_plays_backwards() {
    let data = sample(4_800, |n| n as f32 / 4_800.0);
    let (mut p, _reaper) = player(&data, &[(index::REVERSE, 1.0), (index::ATTACK, 0.0005)]);
    let (l, _) = render(&mut p, 4_800, held, flat);
    assert!(
        l[200] > l[2_000] && l[2_000] > l[4_000],
        "{} {} {}",
        l[200],
        l[2_000],
        l[4_000]
    );
}

#[test]
fn slices_pick_the_right_part() {
    let data = sample(48_000, |n| (n / 12_000) as f32 * 0.2 + 0.1);
    for (slice, level) in [(0.0, 0.1), (0.3, 0.3), (0.6, 0.5), (1.0, 0.7)] {
        let (mut p, _reaper) = player(
            &data,
            &[
                (index::MODE, 4.0),
                (index::SLICES, 4.0),
                (index::SLICE, slice),
                (index::LEVEL, 1.0),
            ],
        );
        let (l, _) = render(&mut p, 24_000, held, flat);
        assert!(
            (l[6_000] - level).abs() < 0.01,
            "slice {slice}: {}",
            l[6_000]
        );
        assert!(peak(&l[12_100..]) < 1e-6, "one slice only");
    }
}

#[test]
fn onset_slices_start_at_the_hits() {
    // Silence with a burst at 0.25 s and 0.6 s.
    let data = sample(48_000, |n| {
        let burst = |at: usize| {
            if n >= at && n < at + 2_000 {
                0.8 * (-((n - at) as f32) / 400.0).exp()
            } else {
                0.0
            }
        };
        burst(0) + burst(12_000) + burst(28_800)
    });
    assert_eq!(data.onsets().len(), 3, "{:?}", data.onsets());
    let (mut p, _reaper) = player(
        &data,
        &[(index::MODE, 4.0), (index::SLICE, 0.5), (index::LEVEL, 1.0)],
    );
    let (l, _) = render(&mut p, 2_000, held, flat);
    assert!(
        l[300] > 0.3,
        "the second slice starts on its hit: {}",
        l[300]
    );
}

#[test]
fn granular_makes_a_cloud_and_freezes() {
    let data = sine(330.0, 48_000);
    let (mut p, _reaper) = player(
        &data,
        &[
            (index::MODE, 5.0),
            (index::DENSITY, 50.0),
            (index::SIZE, 0.05),
            (index::SPREAD, 7.0),
            (index::FREEZE, 1.0),
            (index::POSITION, 0.5),
        ],
    );
    let (l, r) = render(&mut p, 48_000, held, flat);
    assert!(peak(&l[4_800..]) > 0.05 && peak(&r[4_800..]) > 0.05);
    assert!(peak(&l) < 2.0);
    assert!(l.iter().all(|x| x.is_finite()));
}

#[test]
fn polyphony_is_capped_and_stealing_is_smooth() {
    let data = sample(96_000, |_| 0.1);
    let (mut p, _reaper) = player(
        &data,
        &[
            (index::MODE, 1.0),
            (index::LEVEL, 1.0),
            (index::VELOCITY, 0.0),
        ],
    );
    let mut out = Vec::new();
    for _ in 0..20 {
        for key in 0..12 {
            p.note_on(48 + key, 1.0);
            assert!(p.active_voices() <= SLOTS);
            let playing = p
                .bank
                .voices
                .iter()
                .filter(|v| v.active && !v.is_fading())
                .count();
            assert!(playing <= VOICES);
            let (mut l, mut r) = ([0.0; 64], [0.0; 64]);
            p.process(&PlayerInputs::default(), [&mut l, &mut r]);
            out.extend_from_slice(&l);
        }
    }
    // Eight voices of 0.1 each, plus the stolen ones still fading (a
    // steal every 64 frames against a 240-frame fade leaves remainders of
    // about 0.73, 0.47 and 0.2): never more, and never a hard step.
    assert!(peak(&out) <= 0.95, "{}", peak(&out));
    assert!(
        biggest_step(&out[1_000..]) < 0.05,
        "{}",
        biggest_step(&out[1_000..])
    );
    p.all_notes_off();
    for key in 0..128 {
        p.note_off(key);
    }
}

#[test]
fn swapping_samples_fades_and_retires_off_thread() {
    let first = sample(96_000, |_| 0.4);
    let second = sample(96_000, |_| -0.4);
    let (mut p, mut reaper) = player(&first, &[(index::MODE, 1.0), (index::LEVEL, 1.0)]);
    let (a, _) = render(&mut p, 4_800, held, flat);
    p.load(Arc::clone(&second)).unwrap();
    let (b, _) = render(&mut p, 4_800, held, flat);
    assert!(biggest_step(&a[1_000..]).max(biggest_step(&b)) < 0.2);
    assert!(Arc::ptr_eq(p.sample().unwrap(), &second));
    assert_eq!(reaper.collect(), 1);
    assert_eq!(Arc::strong_count(&first), 1);

    p.unload();
    render(&mut p, 1_000, held, flat);
    assert!(p.sample().is_none());
    assert_eq!(reaper.collect(), 1);
    let (quiet, _) = render(&mut p, 1_000, held, flat);
    assert!(peak(&quiet) < 1e-6);
}

#[test]
fn a_full_retire_queue_hands_the_sample_back() {
    let data = sample(1_000, |_| 0.1);
    let (mut p, mut reaper) = SamplePlayer::new();
    let mut accepted = 0;
    let refused = loop {
        match p.load(Arc::clone(&data)) {
            Ok(()) => accepted += 1,
            Err(back) => break back,
        }
        assert!(accepted <= RETIRE_CAPACITY + 1);
    };
    assert!(Arc::ptr_eq(&refused, &data));
    assert_eq!(accepted, RETIRE_CAPACITY + 1);
    assert_eq!(reaper.collect(), RETIRE_CAPACITY);
    assert!(p.load(refused).is_ok());
}

#[test]
fn poison_and_bad_knobs_are_harmless() {
    let data = sine(440.0, 4_800);
    let (mut p, _reaper) = player(&data, &[(index::MODE, 2.0)]);
    let before: Vec<Option<f32>> = (0..PARAM_COUNT).map(|i| p.param(i)).collect();
    p.set_param(index::PITCH, f32::NAN);
    p.set_param(index::PITCH, f32::INFINITY);
    p.set_param(999, 1.0);
    let after: Vec<Option<f32>> = (0..PARAM_COUNT).map(|i| p.param(i)).collect();
    assert_eq!(before, after);
    p.set_param(index::PITCH, 1_000.0);
    assert_eq!(p.param(index::PITCH), Some(48.0));
    p.set_param(index::MODE, 2.6);
    assert_eq!(p.param(index::MODE), Some(3.0));
    assert_eq!(p.param(999), None);

    let bad = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e30];
    let (mut l, mut r) = ([0.0; 4], [0.0; 4]);
    p.process(
        &PlayerInputs {
            gate: &bad,
            pitch: &bad,
            velocity: &bad,
        },
        [&mut l, &mut r],
    );
    let (l, r) = render(
        &mut p,
        4_800,
        |n| if n % 100 < 50 { f32::NAN } else { 1.0 },
        |_| f32::INFINITY,
    );
    assert!(l.iter().chain(&r).all(|x| x.is_finite()));
    p.prepare(f32::NAN);
    p.prepare(-1.0);
    let (l, _) = render(&mut p, 256, held, flat);
    assert!(l.iter().all(|x| x.is_finite()));
    assert_eq!(p.faults(), 0);

    // Mismatched and empty outputs.
    let (mut short, mut long) = ([1.0; 4], [1.0; 8]);
    p.process(&PlayerInputs::default(), [&mut short, &mut long]);
    assert!(long[4..].iter().all(|&x| x.abs() < f32::EPSILON));
    p.process(&PlayerInputs::default(), [&mut [], &mut []]);
}

#[test]
fn no_sample_means_silence() {
    let (mut p, _reaper) = SamplePlayer::new();
    p.note_on(60, 1.0);
    let (l, r) = render(&mut p, 1_000, held, flat);
    assert!(peak(&l) < f32::EPSILON && peak(&r) < f32::EPSILON);
    assert!(p.playhead().is_none());
}

#[test]
fn the_playhead_moves() {
    let data = sample(48_000, |_| 0.2);
    let (mut p, _reaper) = player(&data, &[(index::MODE, 1.0)]);
    render(&mut p, 12_000, held, flat);
    let at = p.playhead().unwrap();
    assert!((at - 0.25).abs() < 0.01, "{at}");
}
