use super::*;
use crate::daemon::wall::Wall;
use crate::engine::{EngineConfig, engine};
use crate::patch::Patch;
use crate::{migrate, store};

#[test]
fn seeds_are_pinned() {
    use kazoo_fx::dsp::{fold32, seeded, splitmix64, stream_seed};
    assert_eq!(splitmix64(0), 0xE220_A839_7B1D_CDAF);
    assert_eq!(fold32(0x1234_5678_9ABC_DEF0), 0x8888_8888);
    assert_eq!(fnv1a64(b"lfo6"), 0x1DB7_19AD_9E8B_B9DA);
    assert_eq!(module_seed(0, "lfo6"), 0x9532_2FC4_F4B2_B9B8);
    assert_eq!(module_seed(0x5EED, "lfo6"), 0xF7ED_B033_1822_2FF1);
    assert_eq!(module_seed(0x5EED, "noise1"), 0x4B85_3CA8_7D1E_9ABD);
    assert_eq!(stream_seed(module_seed(0x5EED, "lfo6"), 0), 0xDEE0_7630);
    assert_eq!(seeded(None, 0x1234), 0x1234);
    assert_eq!(seeded(Some(0x5EED), 0), stream_seed(0x5EED, 0));
}

fn patch(seed: u64) -> Patch {
    let mut patch = Patch::empty(120.0);
    patch.seed = seed;
    for kind in [
        "clock", "seq", "vco", "lfo", "lfo", "sh", "noise", "branch", "env", "mix", "vcf", "vca",
        "tape", "out",
    ] {
        patch.add(kind, None, None).unwrap();
    }
    for (module, knob, value) in [
        ("seq1", "chance", 0.5),
        ("seq1", "step2", 7.0),
        ("vco1", "shape", 3.0),
        ("lfo1", "shape", 5.0),
        ("lfo1", "rate", 8.0),
        ("lfo2", "shape", 4.0),
        ("lfo2", "rate", 7.0),
        ("tape1", "wow", 0.8),
        ("vcf1", "cutoff", 2000.0),
        ("vca1", "gain", 1.0),
    ] {
        patch.turn(module, knob, value, Some(0.0)).unwrap();
    }
    for (from, to, amount) in [
        ("clock1.out", "seq1.clock", 1.0),
        ("seq1.pitch", "vco1.pitch", 1.0),
        ("lfo1.out", "vco1.width", 0.3),
        ("lfo2.out", "vcf1.cutoff", 0.3),
        ("clock1.out", "sh1.trig", 1.0),
        ("sh1.out", "vcf1.resonance", 0.2),
        ("noise1.out", "mix1.b", 0.3),
        ("clock1.out", "branch1.in", 1.0),
        ("branch1.a", "env1.gate", 1.0),
        ("vco1.out", "mix1.a", 1.0),
        ("mix1.out", "vcf1.in", 1.0),
        ("vcf1.out", "vca1.in", 1.0),
        ("env1.out", "vca1.cv", 1.0),
        ("vca1.out", "tape1.left", 1.0),
        ("tape1.left", "out1.left", 1.0),
        ("tape1.right", "out1.right", 1.0),
    ] {
        patch.plug(from, to, Some(amount), None).unwrap();
    }
    patch
}

fn reload(bytes: &[u8]) -> Patch {
    let (mut file, notes) = migrate::read(bytes).unwrap();
    assert!(notes.is_empty(), "{notes:?}");
    assert!(migrate::migrate(&mut file).unwrap().is_empty());
    let (patch, notes) = Patch::load(&file).unwrap();
    assert!(notes.is_empty(), "{notes:?}");
    patch
}

fn render(patch: Patch) -> Vec<u32> {
    let (mut engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let seed = patch.seed;
    let wall = Wall::new(patch, control, None, Vec::new(), None);
    assert_eq!(wall.snapshot().seed, seed);
    let mut samples = vec![0.0; 48_000 * 2];
    let before = assert_no_alloc::violation_count();
    assert_no_alloc::assert_no_alloc(|| engine.render(&mut samples, 2));
    assert_eq!(assert_no_alloc::violation_count(), before);
    assert!(samples.iter().all(|sample| sample.is_finite()));
    assert!(samples.iter().any(|sample| sample.abs() > f32::EPSILON));
    let bits = samples.into_iter().map(f32::to_bits).collect();
    drop(wall);
    bits
}

#[test]
fn same_save_renders_the_same_twice() {
    let bytes = store::encode(&patch(0x5EED)).unwrap();
    let first = render(reload(&bytes));
    let second = render(reload(&bytes));
    assert!(
        first == second,
        "the same save rendered different sample bits"
    );
}

#[test]
fn another_module_added_does_not_move_this_ones_randomness() {
    let mut first = Patch::empty(120.0);
    first.seed = 0x5EED;
    first.add("noise", None, None).unwrap();
    first.add("out", None, None).unwrap();
    first.plug("noise1.out", "out1.left", None, None).unwrap();
    let mut second = Patch::empty(120.0);
    second.seed = first.seed;
    for kind in ["lfo", "sh", "vca", "noise", "out"] {
        second.add(kind, None, None).unwrap();
    }
    second.turn("lfo1", "shape", 5.0, Some(0.0)).unwrap();
    second.turn("vca1", "gain", 0.0, Some(0.0)).unwrap();
    for (from, to) in [
        ("lfo1.out", "vca1.in"),
        ("sh1.out", "vca1.cv"),
        ("vca1.out", "out1.left"),
        ("noise1.out", "out1.left"),
    ] {
        second.plug(from, to, None, None).unwrap();
    }
    assert!(
        render(first) == render(second),
        "another module moved the noise stream"
    );
}

#[test]
fn a_different_wall_seed_moves_the_randomness() {
    let first = patch(0x5EED);
    let second = patch(0x5EEE);
    assert_eq!(first.to_file().seed, 0x5EED);
    assert_eq!(second.to_file().seed, 0x5EEE);
    assert_ne!(first.fingerprint(), second.fingerprint());
    assert_ne!(
        first.fingerprint_but_tempo(),
        second.fingerprint_but_tempo()
    );
    assert!(
        render(first) != render(second),
        "the wall seed did not move the randomness"
    );
}

#[test]
fn two_hats_on_one_wall_do_not_hiss_together() {
    let mut patch = Patch::empty(120.0);
    patch.seed = 0x5EED;
    for kind in ["clock", "hat", "hat", "scope", "scope", "out"] {
        patch.add(kind, None, None).unwrap();
    }
    for (module, knob, value) in [
        ("hat1", "metal", 0.0),
        ("hat2", "metal", 0.0),
        ("hat1", "decay", 0.5),
        ("hat2", "decay", 0.5),
        ("scope1", "mode", 1.0),
        ("scope2", "mode", 1.0),
        ("scope1", "time", 0.05),
        ("scope2", "time", 0.05),
    ] {
        patch.turn(module, knob, value, Some(0.0)).unwrap();
    }
    for (from, to) in [
        ("clock1.out", "hat1.trigger"),
        ("clock1.out", "hat2.trigger"),
        ("hat1.out", "scope1.a"),
        ("hat2.out", "scope2.a"),
        ("scope1.a", "out1.left"),
        ("scope2.a", "out1.right"),
    ] {
        patch.plug(from, to, None, None).unwrap();
    }
    let bytes = store::encode(&patch).unwrap();
    let samples = render(reload(&bytes));
    assert!(
        samples == render(reload(&bytes)),
        "hat save changed on reload"
    );
    assert!(
        samples.chunks_exact(2).any(|frame| frame[0] != frame[1]),
        "two hats on one wall hiss in lockstep"
    );
    let (mut engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let mut wall = Wall::new(reload(&bytes), control, None, Vec::new(), None);
    let mut buffer = [0.0; 512];
    for _ in 0..40 {
        assert_no_alloc::assert_no_alloc(|| engine.render(&mut buffer, 2));
        assert!(wall.tick(std::time::Instant::now()).is_empty());
    }
    let snapshot = wall.snapshot();
    let traces: Vec<_> = ["scope1", "scope2"]
        .map(|id| {
            let probe = snapshot
                .probes
                .iter()
                .find(|probe| probe.module == id)
                .unwrap();
            let scope = probe.scope.as_ref().unwrap();
            assert!(!scope.a.is_empty());
            assert!(scope.a.iter().flatten().any(|sample| sample.abs() > 1.0e-6));
            scope.a.clone()
        })
        .into_iter()
        .collect();
    assert!(traces[0] != traces[1], "hat scope traces match");
}
