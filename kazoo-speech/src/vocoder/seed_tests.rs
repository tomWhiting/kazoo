use super::{Noise, Vocoder, param};

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
    assert!(render(None) == render(None), "unseeded vocoder changed");
}
