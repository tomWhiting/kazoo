use crate::{Context, Effect, EffectKind, catalogue, dsp::Noise, find};

#[global_allocator]
static ALLOCATOR: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

const RATE: f32 = 48_000.0;
const RANDOM: [&str; 11] = [
    "tape", "cassette", "vinyl", "vhs", "reel", "genloss", "radio", "granular", "spring", "bbd",
    "crush",
];

fn build(kind: &EffectKind, seed: Option<u64>) -> Box<dyn Effect> {
    let mut effect = (kind.build)();
    for (index, spec) in kind.params.iter().enumerate() {
        let value = match spec.name {
            "mix" | "wow" | "flutter" | "hiss" | "noise" | "crackle" | "wear" | "drift"
            | "spread" | "jitter" | "dither" | "detune" | "wobble" | "warp" => spec.max,
            _ => spec.default,
        };
        effect.set_param(index, value);
    }
    effect.prepare(RATE);
    if let Some(seed) = seed {
        effect.set_seed(seed);
    }
    effect
}

fn render(effect: &mut dyn Effect, frames: usize) -> Vec<u32> {
    let mut noise = Noise::new(31);
    let input: Vec<f32> = (0..frames).map(|_| noise.sample() * 0.2).collect();
    let mut left = vec![0.0; frames];
    let mut right = vec![0.0; frames];
    let context = Context { bpm: 120.0 };
    for ((input, left), right) in input
        .chunks(256)
        .zip(left.chunks_mut(256))
        .zip(right.chunks_mut(256))
    {
        assert_no_alloc::assert_no_alloc(|| {
            effect.process(&context, [input, input], [left, right]);
        });
    }
    assert!(left.iter().chain(&right).all(|sample| sample.is_finite()));
    left.into_iter().chain(right).map(f32::to_bits).collect()
}

#[test]
fn every_effect_seeded_alike_renders_alike_and_unseeded_is_unchanged() {
    for kind in catalogue() {
        for seed in [Some(9), None] {
            let first = render(build(kind, seed).as_mut(), 4096);
            let second = render(build(kind, seed).as_mut(), 4096);
            assert!(first == second, "{} with seed {seed:?}", kind.id);
        }
    }
}

#[test]
fn random_effects_follow_their_seed() {
    let mut unchanged = Vec::new();
    for id in RANDOM {
        let kind = find(id).unwrap();
        let first = render(build(kind, Some(1)).as_mut(), 48_000);
        let second = render(build(kind, Some(2)).as_mut(), 48_000);
        if first == second {
            unchanged.push(id);
        }
    }
    assert!(
        unchanged.is_empty(),
        "effects ignored their seeds: {unchanged:?}"
    );
}

#[test]
fn a_reset_returns_to_the_seeded_stream() {
    for id in ["cassette", "radio", "spring"] {
        let kind = find(id).unwrap();
        let mut effect = build(kind, Some(5));
        let original = render(effect.as_mut(), 48_000);
        effect.reset();
        let restarted = render(effect.as_mut(), 48_000);
        let fresh = render(build(kind, Some(5)).as_mut(), 48_000);
        assert!(original == fresh, "{id} fresh stream");
        assert!(restarted == fresh, "{id} reset lost the seeded stream");
        effect.prepare(RATE);
        assert!(
            render(effect.as_mut(), 48_000) == fresh,
            "{id} prepare lost the seed"
        );
    }
}
