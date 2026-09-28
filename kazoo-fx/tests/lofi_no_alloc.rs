//! The lofi family never allocates on the audio thread.
//!
//! Every effect is built and prepared (which may allocate), then processed,
//! reset and turned through every knob's extremes inside
//! `assert_no_alloc`, whose allocator aborts the test on any allocation.
//!
//! `assert_no_alloc` only polices allocation in builds with debug
//! assertions (in release it compiles to nothing), so this runs in those
//! builds: `cargo test -p kazoo-fx`, not `--release`.

#![cfg(debug_assertions)]

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use kazoo_fx::{Context, Effect, lofi};

#[global_allocator]
static A: AllocDisabler = AllocDisabler;

const RATE: f32 = 48_000.0;
const BLOCK: usize = 256;

/// A block of loud tone and poison, made before the audio thread starts.
fn programme() -> Vec<f32> {
    (0..BLOCK)
        .map(|n| match n % 61 {
            0 => f32::NAN,
            1 => f32::INFINITY,
            2 => f32::NEG_INFINITY,
            _ => (std::f32::consts::TAU * 220.0 * n as f32 / RATE).sin(),
        })
        .collect()
}

/// Everything the real-time methods may be asked to do.
fn exercise(effect: &mut dyn Effect, params: &[kazoo_fx::ParamSpec], input: &[f32]) {
    let context = Context { bpm: 120.0 };
    let mut left = [0.0f32; BLOCK];
    let mut right = [0.0f32; BLOCK];
    for pass in 0..40 {
        for (index, spec) in params.iter().enumerate() {
            let value = match pass % 4 {
                0 => spec.min,
                1 => spec.max,
                2 => f32::NAN,
                _ => spec.default,
            };
            effect.set_param(index, value);
        }
        effect.set_param(params.len(), 1.0);
        effect.process(&context, [input, input], [&mut left, &mut right]);
        effect.process(
            &context,
            [&input[..7], &input[..5]],
            [&mut left, &mut right],
        );
        effect.process(&context, [&[], &[]], [&mut [], &mut []]);
        if pass % 10 == 9 {
            effect.reset();
        }
    }
    assert!(left.iter().chain(&right).all(|x| x.is_finite()));
}

#[test]
fn lofi_effects_never_allocate_on_the_audio_thread() {
    let input = programme();
    for kind in lofi::KINDS {
        let mut effect = (kind.build)();
        effect.prepare(RATE);
        assert_no_alloc(|| exercise(effect.as_mut(), kind.params, &input));
    }
}
