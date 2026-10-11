//! Handing over the wet signal never allocates on the audio thread: every
//! effect in the catalogue runs `process_wet` (with every knob at its ends,
//! nonsense knob values, poisoned input and slices of every length) under
//! an allocator that fails on any allocation.
//!
//! `assert_no_alloc` only polices allocation in builds with debug
//! assertions (in release it compiles to nothing), so this runs in those
//! builds: `cargo nextest run -p kazoo-fx`, not `--release`.

#![cfg(debug_assertions)]

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use kazoo_fx::{Context, catalogue};

#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

/// Rates to prepare at: the rate sets every buffer's size and every
/// oversampling factor.
const RATES: [f32; 3] = [44_100.0, 48_000.0, 192_000.0];
const BLOCKS: [usize; 4] = [0, 1, 333, 1_024];

#[test]
fn handing_over_the_wet_signal_never_allocates() {
    let context = Context { bpm: 120.0 };
    let mut noise = kazoo_fx::dsp::Noise::new(77);
    let input: Vec<f32> = (0..1_024).map(|_| noise.sample() * 0.5).collect();
    let mut poison = input.clone();
    poison[5] = f32::NAN;
    poison[17] = f32::INFINITY;
    let mut left = vec![0.0f32; 1_024];
    let mut right = vec![0.0f32; 1_024];
    let mut wet_left = vec![0.0f32; 1_024];
    let mut wet_right = vec![0.0f32; 1_024];
    for kind in catalogue() {
        for rate in RATES {
            let mut effect = (kind.build)();
            effect.prepare(rate);
            let params = kind.params;
            assert_no_alloc(|| {
                for (index, spec) in params.iter().enumerate() {
                    for value in [spec.min, spec.max, f32::NAN, spec.default] {
                        effect.set_param(index, value);
                        for block in BLOCKS {
                            effect.process_wet(
                                &context,
                                [&input[..block], &poison[..block]],
                                [&mut left[..block], &mut right[..block]],
                                [&mut wet_left[..block], &mut wet_right[..block]],
                            );
                        }
                    }
                }
                effect.process_wet(
                    &context,
                    [&input[..100], &input[..90]],
                    [&mut left[..80], &mut right[..100]],
                    [&mut wet_left[..70], &mut wet_right[..1_024]],
                );
                effect.reset();
                effect.process_wet(
                    &context,
                    [&input, &input],
                    [&mut left, &mut right],
                    [&mut wet_left, &mut wet_right],
                );
            });
            assert!(
                left.iter()
                    .chain(&right)
                    .chain(&wet_left)
                    .chain(&wet_right)
                    .all(|sample| sample.is_finite()),
                "{} at {rate} Hz",
                kind.id
            );
        }
    }
}
