//! Nothing on the audio thread allocates: every voice and every rhythm
//! generator is struck, turned, choked, reset and run under an allocator
//! that aborts on any allocation.

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use kazoo_perc::{MAX_OUTPUTS, Rhythm, Voice, catalogue, rhythms};

#[global_allocator]
static A: AllocDisabler = AllocDisabler;

const RATE: f32 = 48_000.0;

fn exercise_voice(voice: &mut dyn Voice, params: usize, out: &mut [f32]) {
    for index in 0..=params {
        voice.set_param(index, 0.3);
        voice.set_param(index, f32::NAN);
        voice.set_param(index, 1.0e9);
    }
    voice.trigger(1.0, true);
    for block in out.chunks_mut(64) {
        voice.process(block);
    }
    voice.trigger(0.7, false);
    voice.process(&mut out[..17]);
    voice.trigger(0.5, false);
    voice.process(out);
    voice.trigger(0.0, false);
    voice.process(out);
    voice.trigger(f32::NAN, false);
    voice.reset();
    voice.process(out);
    voice.process(&mut []);
}

#[test]
fn voices_never_allocate() {
    let mut out = vec![0.0f32; 4_096];
    for kind in catalogue() {
        let mut voice = (kind.build)();
        voice.prepare(RATE);
        assert_no_alloc(|| exercise_voice(voice.as_mut(), kind.params.len(), &mut out));
        assert!(out.iter().all(|sample| sample.is_finite()), "{}", kind.id);
    }
}

fn exercise_rhythm(
    rhythm: &mut dyn Rhythm,
    params: usize,
    clock: &[f32],
    reset: &[f32],
    outs: &mut [Vec<f32>],
) {
    for index in 0..=params {
        rhythm.set_param(index, 1.0);
        rhythm.set_param(index, f32::NAN);
    }
    for round in 0..3 {
        let [a, b, c, d] = outs else {
            return;
        };
        let mut slices: [&mut [f32]; MAX_OUTPUTS] = [a, b, c, d];
        rhythm.process(clock, reset, &mut slices);
        if round == 1 {
            rhythm.reset();
        }
    }
    rhythm.process(&[], &[], &mut []);
}

#[test]
fn rhythm_generators_never_allocate() {
    let length = 4_800;
    let clock: Vec<f32> = (0..length)
        .map(|n| if n % 300 < 40 { 1.0 } else { 0.0 })
        .collect();
    let reset: Vec<f32> = (0..length)
        .map(|n| {
            if (2_000..2_010).contains(&n) {
                1.0
            } else {
                0.0
            }
        })
        .collect();
    let mut outs = vec![vec![0.0f32; length]; MAX_OUTPUTS];
    for kind in rhythms() {
        let mut rhythm = (kind.build)();
        rhythm.prepare(RATE);
        rhythm.set_param(0, 4.0);
        assert_no_alloc(|| {
            exercise_rhythm(
                rhythm.as_mut(),
                kind.params.len(),
                &clock,
                &reset,
                &mut outs,
            );
        });
        assert!(
            outs.iter().flatten().any(|&level| level > 0.5),
            "{}",
            kind.id
        );
    }
}
