//! The time family never allocates on the audio thread: processing,
//! setting parameters and resetting all run under an allocator that fails
//! on any allocation.
//!
//! `assert_no_alloc` only polices allocation in builds with debug
//! assertions (in release it compiles to nothing), so this runs in those
//! builds: `cargo test -p kazoo-fx`, not `--release`.

#![cfg(debug_assertions)]

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use kazoo_fx::{Context, time};

#[global_allocator]
static A: AllocDisabler = AllocDisabler;

/// Rates to prepare at: the rates change every buffer's size.
const RATES: [f32; 3] = [44_100.0, 48_000.0, 192_000.0];
const BLOCKS: [usize; 5] = [0, 1, 64, 333, 1_024];

#[test]
fn the_time_family_never_allocates_on_the_audio_thread() {
    let mut noise = kazoo_fx::dsp::Noise::new(61);
    let input: Vec<f32> = (0..1_024).map(|_| noise.sample() * 0.5).collect();
    let mut poison = input.clone();
    poison[3] = f32::NAN;
    poison[9] = f32::INFINITY;
    let mut left = vec![0.0f32; 1_024];
    let mut right = vec![0.0f32; 1_024];
    for (kind, rate) in time::KINDS
        .iter()
        .flat_map(|kind| RATES.iter().map(move |rate| (kind, *rate)))
    {
        let mut effect = (kind.build)();
        effect.prepare(rate);
        let params = kind.params;
        assert_no_alloc(|| {
            for bpm in [20.0, 120.0, 999.0] {
                let context = Context { bpm };
                for (index, spec) in params.iter().enumerate() {
                    for value in [spec.min, spec.max, spec.default, f32::NAN] {
                        effect.set_param(index, value);
                        for block in BLOCKS {
                            effect.process(
                                &context,
                                [&input[..block], &poison[..block]],
                                [&mut left[..block], &mut right[..block]],
                            );
                        }
                    }
                }
                effect.set_param(params.len(), 1.0);
                effect.process(
                    &context,
                    [&input[..100], &input[..90]],
                    [&mut left[..80], &mut right[..100]],
                );
                effect.reset();
            }
        });
        assert!(
            left.iter().chain(&right).all(|sample| sample.is_finite()),
            "{}",
            kind.id
        );
        assert!(effect.latency() < rate as usize, "{} {rate}", kind.id);
    }
}
