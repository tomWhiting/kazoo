//! The drive family never allocates on the audio thread: processing,
//! turning knobs (sensible values and nonsense alike) and resetting all run
//! with allocation forbidden.
//!
//! `assert_no_alloc` only polices allocation in builds with debug
//! assertions (in release it compiles to nothing and has no allocator to
//! install), so the check lives in those builds: the gate runs it with
//! `cargo test -p kazoo-fx`, not `--release`. Every effect is checked at
//! each common host rate, since the rate decides how much each one
//! oversamples.

#![cfg(debug_assertions)]

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use kazoo_fx::{Context, drive};

#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

#[test]
fn drive_effects_never_allocate_on_the_audio_thread() {
    let context = Context { bpm: 120.0 };
    let input: Vec<f32> = (0..1_024).map(|i| 0.5 * (i as f32 * 0.07).sin()).collect();
    let mut left = vec![0.0f32; 1_024];
    let mut right = vec![0.0f32; 1_024];
    for (kind, rate) in drive::KINDS
        .iter()
        .flat_map(|kind| [44_100.0, 48_000.0, 96_000.0, 192_000.0].map(|rate| (kind, rate)))
    {
        let mut effect = (kind.build)();
        effect.prepare(rate);
        assert_no_alloc(|| {
            effect.process(&context, [&input, &input], [&mut left, &mut right]);
            for (index, spec) in kind.params.iter().enumerate() {
                effect.set_param(index, spec.max);
                effect.process(&context, [&input, &input], [&mut left, &mut right]);
                effect.set_param(index, spec.min);
                effect.set_param(index, f32::NAN);
                effect.set_param(index, f32::INFINITY);
            }
            effect.set_param(kind.params.len(), 1.0);
            effect.process(
                &context,
                [&input[..7], &input],
                [&mut left[..3], &mut right],
            );
            effect.process(&context, [&[], &[]], [&mut [], &mut []]);
            effect.reset();
            effect.process(&context, [&input, &input], [&mut left, &mut right]);
        });
        assert!(
            left.iter().chain(&right).all(|s| s.is_finite()),
            "{} at {rate} Hz",
            kind.id
        );
    }
}
