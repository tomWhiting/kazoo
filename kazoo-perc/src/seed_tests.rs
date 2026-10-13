use super::*;

const RATE: f32 = 48_000.0;
const FRAMES: usize = 4_800;

/// What every voice and rhythm renders when it is never seeded, as
/// [`fingerprint`]s in catalogue order, pinned at main `42664d7`, before
/// `set_seed` existed. The instruments outside the wall never seed, so none
/// of these may ever move.
const UNSEEDED: [(&str, u64); 20] = [
    ("kick", 0xAA7F_4FBF_8743_2A5E),
    ("snare", 0x23C5_E0BF_43C6_F3AF),
    ("clap", 0xDD88_1750_F39B_2C34),
    ("hat", 0x48A8_9A01_90DD_14A2),
    ("cymbal", 0x1FD5_3071_2586_B5EE),
    ("tom", 0x3411_F058_9128_AE73),
    ("conga", 0xEF15_9718_1F90_C5CC),
    ("rim", 0x0A00_6C6D_41A8_EA8C),
    ("cowbell", 0x7D2F_E8EC_7239_E315),
    ("shaker", 0x7E0A_C34F_556B_2FF6),
    ("membrane", 0x8394_9FB5_3814_2315),
    ("modal", 0x796A_0023_BB83_648D),
    ("metal", 0xF79E_DE57_8308_1CC5),
    ("noiseperc", 0x90E8_789E_E032_5C12),
    ("euclid", 0x8CAF_7C3B_E5F4_1725),
    ("prob", 0x652F_2DB1_2874_F225),
    ("prob locked", 0x344A_09C4_18E4_1725),
    ("grids", 0xB378_0876_0593_BB25),
    ("poly", 0x42CD_A568_B12F_2725),
    ("burst", 0x2042_97D7_84C7_2325),
];

/// The 64-bit FNV-1a of a render's sample bits: one number that moves if
/// any sample moves by so much as one bit.
fn fingerprint(bits: &[u32]) -> u64 {
    bits.iter()
        .flat_map(|bits| bits.to_le_bytes())
        .fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
        })
}

/// `kind` struck at full velocity, reset and struck again, seeded with
/// `seed` when there is one and never seeded otherwise. The kick's click is
/// turned up so its noise is heard.
fn voice_samples(kind: &VoiceKind, seed: Option<u64>) -> Vec<u32> {
    let mut voice = (kind.build)();
    voice.prepare(RATE);
    if kind.id == "kick" {
        voice.set_param(0, 1.0);
    }
    if let Some(seed) = seed {
        voice.set_seed(seed);
    }
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

/// `kind` clocked for four cycles of sixteen steps (chaos up, density at
/// 0.5, no ratchets, and locked when `locked`), seeded with `seed` when
/// there is one and never seeded otherwise.
fn rhythm_samples(kind: &RhythmKind, seed: Option<u64>, locked: bool) -> Vec<u32> {
    let mut rhythm = (kind.build)();
    rhythm.prepare(RATE);
    if let Some(seed) = seed {
        rhythm.set_seed(seed);
    }
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
fn unseeded_voices_are_unchanged() {
    let mut renders = Vec::new();
    for kind in catalogue() {
        let samples = voice_samples(kind, None);
        assert!(
            samples == voice_samples(kind, None),
            "{} rendered twice differs",
            kind.id
        );
        renders.push((kind.id.to_owned(), fingerprint(&samples)));
    }
    for kind in rhythms() {
        let lockable = kind.params.iter().any(|param| param.name == "lock");
        for locked in [false, true] {
            if locked && !lockable {
                continue;
            }
            let samples = rhythm_samples(kind, None, locked);
            assert!(
                samples == rhythm_samples(kind, None, locked),
                "{} rendered twice differs",
                kind.id
            );
            let name = if locked {
                format!("{} locked", kind.id)
            } else {
                kind.id.to_owned()
            };
            renders.push((name, fingerprint(&samples)));
        }
    }
    let pinned: Vec<(String, u64)> = UNSEEDED
        .iter()
        .map(|&(name, print)| (name.to_owned(), print))
        .collect();
    assert_eq!(renders, pinned, "an unseeded render moved");
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
        let first = voice_samples(kind, Some(1));
        assert!(
            first == voice_samples(kind, Some(1)),
            "{id} changed with same seed"
        );
        assert!(first != voice_samples(kind, Some(2)), "{id} ignored seed");
    }
    for id in ["prob", "grids"] {
        let kind = find_rhythm(id).unwrap();
        let first = rhythm_samples(kind, Some(1), false);
        assert!(
            first == rhythm_samples(kind, Some(1), false),
            "{id} changed with same seed"
        );
        assert!(
            first != rhythm_samples(kind, Some(2), false),
            "{id} ignored seed"
        );
    }
}

#[test]
fn a_locked_prob_repeats_its_own_seeds_pattern() {
    let kind = find_rhythm("prob").unwrap();
    let first = rhythm_samples(kind, Some(1), true);
    let second = rhythm_samples(kind, Some(2), true);
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
