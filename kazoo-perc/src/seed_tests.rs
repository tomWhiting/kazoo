use super::*;

const RATE: f32 = 48_000.0;
const FRAMES: usize = 4_800;

fn voice_samples(kind: &VoiceKind) -> Vec<u32> {
    let mut voice = (kind.build)();
    voice.prepare(RATE);
    let mut samples = vec![0.0; FRAMES];
    voice.trigger(1.0, false);
    voice.process(&mut samples);
    assert!(
        samples.iter().all(|sample| sample.is_finite()),
        "{}",
        kind.id
    );
    samples.into_iter().map(f32::to_bits).collect()
}

#[test]
fn unseeded_voices_are_unchanged() {
    for kind in catalogue() {
        assert_eq!(voice_samples(kind), voice_samples(kind), "{}", kind.id);
    }
}

fn seeded_voice_samples(kind: &VoiceKind, seed: u64) -> Vec<u32> {
    let mut voice = (kind.build)();
    voice.prepare(RATE);
    if kind.id == "kick" {
        voice.set_param(0, 1.0);
    }
    voice.set_seed(seed);
    voice.prepare(RATE);
    let mut samples = vec![0.0; FRAMES * 2];
    voice.trigger(1.0, false);
    voice.process(&mut samples[..FRAMES]);
    voice.reset();
    voice.trigger(1.0, false);
    voice.process(&mut samples[FRAMES..]);
    assert!(
        samples.iter().all(|sample| sample.is_finite()),
        "{}",
        kind.id
    );
    assert!(
        samples.iter().any(|sample| sample.abs() > f32::EPSILON),
        "{}",
        kind.id
    );
    samples.into_iter().map(f32::to_bits).collect()
}

fn rhythm_samples(kind: &RhythmKind, seed: u64, locked: bool) -> Vec<u32> {
    let mut rhythm = (kind.build)();
    rhythm.prepare(RATE);
    rhythm.set_seed(seed);
    rhythm.prepare(RATE);
    for (index, param) in kind.params.iter().enumerate() {
        match param.name {
            "chaos" => rhythm.set_param(index, 1.0),
            "density" => rhythm.set_param(index, 0.5),
            "ratchet" => rhythm.set_param(index, 0.0),
            "lock" if locked => rhythm.set_param(index, 1.0),
            _ => {}
        }
    }
    let clock: Vec<f32> = (0..16 * 4 * 64)
        .map(|frame| if frame % 64 < 32 { 1.0 } else { 0.0 })
        .collect();
    let reset = vec![0.0; clock.len()];
    let mut lanes = vec![vec![0.0; clock.len()]; kind.outputs.len()];
    let mut outputs: Vec<&mut [f32]> = lanes.iter_mut().map(Vec::as_mut_slice).collect();
    rhythm.process(&clock, &reset, &mut outputs);
    assert!(lanes.iter().flatten().all(|sample| sample.is_finite()));
    lanes.into_iter().flatten().map(f32::to_bits).collect()
}

#[test]
fn random_voices_and_rhythms_follow_their_seed() {
    for id in [
        "conga",
        "cymbal",
        "hat",
        "tom",
        "rim",
        "noiseperc",
        "kick",
        "clap",
        "shaker",
        "modal",
        "snare",
    ] {
        let kind = find(id).unwrap();
        let first = seeded_voice_samples(kind, 1);
        assert!(
            first == seeded_voice_samples(kind, 1),
            "{id} changed with same seed"
        );
        assert!(first != seeded_voice_samples(kind, 2), "{id} ignored seed");
    }
    for id in ["prob", "grids"] {
        let kind = find_rhythm(id).unwrap();
        let first = rhythm_samples(kind, 1, false);
        assert!(
            first == rhythm_samples(kind, 1, false),
            "{id} changed with same seed"
        );
        assert!(first != rhythm_samples(kind, 2, false), "{id} ignored seed");
    }
}

#[test]
fn a_locked_prob_repeats_its_own_seeds_pattern() {
    let kind = find_rhythm("prob").unwrap();
    let first = rhythm_samples(kind, 1, true);
    let second = rhythm_samples(kind, 2, true);
    for samples in [&first, &second] {
        for lane in samples.chunks_exact(16 * 4 * 64) {
            let mut cycles = lane.chunks_exact(16 * 64);
            let first_cycle = cycles.next().unwrap();
            for cycle in cycles {
                assert!(first_cycle == cycle, "locked cycle changed");
            }
        }
    }
    assert!(first != second, "locked prob ignored its own seed");
}
