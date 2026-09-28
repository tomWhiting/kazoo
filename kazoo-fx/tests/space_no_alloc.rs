//! The space family's real-time contract: `process`, `set_param` and
//! `reset` never allocate, whatever they are given.
//!
//! `assert_no_alloc` only polices allocation in builds with debug
//! assertions (in release it compiles to nothing), so this runs in those
//! builds: `cargo test -p kazoo-fx`, not `--release`.

#![cfg(debug_assertions)]

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use kazoo_fx::dsp::Noise;
use kazoo_fx::{Context, space};

#[global_allocator]
static A: AllocDisabler = AllocDisabler;

#[test]
fn space_effects_never_allocate_on_the_audio_thread() {
    let context = Context { bpm: 120.0 };
    let mut noise = Noise::new(99);
    let input: Vec<f32> = (0..1_024).map(|_| noise.sample() * 0.8).collect();
    let mut left = vec![0.0f32; 1_024];
    let mut right = vec![0.0f32; 1_024];
    for (kind, rate) in space::KINDS
        .iter()
        .flat_map(|kind| [44_100.0, 48_000.0, 192_000.0].map(|rate| (kind, rate)))
    {
        let mut effect = (kind.build)();
        effect.prepare(rate);
        let params = kind.params;
        assert_no_alloc(|| {
            for round in 0..35 {
                // Walk every knob across its range, with poison mixed in.
                // Knob positions (every 7 rounds) and block sizes (every 5)
                // run out of step, so every pairing turns up.
                for (index, spec) in params.iter().enumerate() {
                    let travel = ((round + index) % 7) as f32 / 6.0;
                    effect.set_param(index, (spec.max - spec.min).mul_add(travel, spec.min));
                }
                effect.set_param(round % params.len(), f32::NAN);
                effect.set_param(params.len() + round, 1.0);
                let frames = [0, 1, 17, 256, 1_024][round % 5];
                effect.process(
                    &context,
                    [&input[..frames], &input[..frames]],
                    [&mut left[..frames], &mut right[..frames]],
                );
                // Mismatched lengths.
                effect.process(
                    &context,
                    [&input[..], &input[..100]],
                    [&mut left[..], &mut right[..50]],
                );
                if round == 12 {
                    effect.reset();
                }
            }
        });
        assert!(
            left.iter().chain(&right).all(|x| x.is_finite()),
            "{}",
            kind.id
        );
    }
}
