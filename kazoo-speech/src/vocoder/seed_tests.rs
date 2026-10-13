use super::{Noise, Vocoder, param};

/// What a vocoder never seeded renders below, left and right, as
/// [`fingerprint`]s pinned at main `42664d7`, before `set_seed` existed.
/// Nothing outside the wall seeds a vocoder, so these may never move.
const UNSEEDED: [u64; 2] = [0x49C8_C86C_1231_B662, 0x1ADD_0CA3_A8C6_88E5];

/// The 64-bit FNV-1a of a render's sample bits: one number that moves if
/// any sample moves by so much as one bit.
fn fingerprint(bits: &[u32]) -> u64 {
    bits.iter()
        .flat_map(|bits| bits.to_le_bytes())
        .fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
        })
}

fn render(seed: Option<u64>) -> [Vec<u32>; 2] {
    let mut vocoder = Vocoder::new(48_000.0);
    vocoder.set_param(param::UNVOICED, 1.0);
    if let Some(seed) = seed {
        vocoder.set_seed(seed);
    }
    vocoder.reset();
    vocoder.prepare(48_000.0);
    let mut input_noise = Noise::new(73);
    let modulator: Vec<f32> = (0..9_600).map(|_| input_noise.sample() * 0.5).collect();
    let carrier = vec![0.0; modulator.len()];
    let mut left = vec![0.0; modulator.len()];
    let mut right = vec![0.0; modulator.len()];
    vocoder.process([&carrier, &carrier], &modulator, [&mut left, &mut right]);
    assert!(vocoder.unvoiced_level() > 0.5);
    for channel in [&left, &right] {
        assert!(channel.iter().all(|sample| sample.is_finite()));
        assert!(channel.iter().any(|sample| sample.abs() > f32::EPSILON));
    }
    [left, right].map(|channel| channel.into_iter().map(f32::to_bits).collect())
}

#[test]
fn an_unseeded_vocoder_is_unchanged() {
    let unseeded = render(None);
    assert!(unseeded == render(None), "unseeded vocoder changed");
    assert_eq!(
        unseeded.each_ref().map(|channel| fingerprint(channel)),
        UNSEEDED,
        "an unseeded vocoder's noise moved"
    );
}

#[test]
fn vocoder_noise_follows_its_seed() {
    let first = render(Some(1));
    let again = render(Some(1));
    let second = render(Some(2));
    for channel in 0..2 {
        assert!(
            first[channel] == again[channel],
            "channel {channel} changed"
        );
        assert!(
            first[channel] != second[channel],
            "channel {channel} ignored seed"
        );
    }
    assert!(
        first[0] != first[1],
        "the stereo noise sources share a salt"
    );
}
