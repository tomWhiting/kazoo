//! Fingerprint tests: the flow maths on small graphs, replugging, loops,
//! and that nothing moves unless something changes.

use super::*;

fn shares(pairs: &[(&str, f64)]) -> Shares {
    pairs
        .iter()
        .map(|(seat, share)| ((*seat).to_string(), *share))
        .collect()
}

/// lfo1 → vcf1 (amount 1), vcf1 → vca1 (amount 0.5).
fn chain() -> Patch {
    let mut patch = Patch::empty(120.0);
    for kind in ["lfo", "vcf", "vca"] {
        patch.add(kind, None, None).unwrap();
    }
    patch.plug("lfo1.out", "vcf1.in", Some(1.0), None).unwrap();
    patch.plug("vcf1.out", "vca1.in", Some(0.5), None).unwrap();
    patch
}

/// Apply a turn and record its dye.
fn turn(patch: &mut Patch, dye: &mut Dye, seat: &str, module: &str, knob: &str, value: f64) {
    let what = patch.turn(module, knob, value, Some(0.0)).unwrap();
    dye.touch(seat, &what, patch);
}

#[test]
fn dye_flows_downstream_thinning_with_every_hop() {
    let mut patch = chain();
    let mut dye = Dye::new();
    // Tom moves the lfo's depth across its whole range: 1 unit into lfo1.
    turn(&mut patch, &mut dye, "Tom", "lfo1", "depth", 0.0);
    // Waffles moves the vca's gain a quarter of its range: 0.25 into vca1.
    turn(&mut patch, &mut dye, "Waffles", "vca1", "gain", 0.75);
    let prints = dye.flow(&patch);
    assert_eq!(prints.modules["lfo1"], shares(&[("Tom", 1.0)]));
    assert_eq!(prints.modules["vcf1"], shares(&[("Tom", 1.0)]));
    // Tom's unit reaches vca1 over two hops: 0.5 × 1 × 0.5 × 0.5 = 0.125,
    // against Waffles' own 0.25.
    assert_eq!(
        prints.modules["vca1"],
        shares(&[("Tom", 0.333_333), ("Waffles", 0.666_667)])
    );
    // A cable carries its source's mix.
    assert_eq!(prints.cables["1"], shares(&[("Tom", 1.0)]));
    assert_eq!(prints.cables["2"], shares(&[("Tom", 1.0)]));
}

#[test]
fn replugging_reroutes_the_dye() {
    let mut patch = chain();
    let mut dye = Dye::new();
    turn(&mut patch, &mut dye, "Tom", "lfo1", "depth", 0.0);
    turn(&mut patch, &mut dye, "Waffles", "vca1", "gain", 0.75);
    let before = dye.flow(&patch);

    let unplugged = patch.unplug(1, None).unwrap();
    dye.touch("Cassio", &unplugged, &patch);
    let after = dye.flow(&patch);
    // Unplugging leaves no dye of its own: Cassio is nowhere.
    assert!(after.modules.values().all(|s| !s.contains_key("Cassio")));
    assert!(!after.modules.contains_key("vcf1"));
    assert_eq!(after.modules["vca1"], shares(&[("Waffles", 1.0)]));
    let moved = after.changed_since(&before);
    assert_eq!(moved.modules["vcf1"], Shares::new());
    assert_eq!(moved.cables["1"], Shares::new());
    assert!(!moved.modules.contains_key("lfo1"));

    // Plugging the lfo straight into the vca, fully: Tom's unit arrives
    // at half strength against Waffles' quarter. The plug is Cassio's
    // touch on vca1: a whole unit of her dye.
    let plugged = patch.plug("lfo1.out", "vca1.cv", Some(1.0), None).unwrap();
    dye.touch("Cassio", &plugged, &patch);
    let replugged = dye.flow(&patch);
    assert_eq!(
        replugged.modules["vca1"],
        shares(&[
            ("Cassio", 0.571_429),
            ("Tom", 0.285_714),
            ("Waffles", 0.142_857)
        ])
    );
}

#[test]
fn feedback_loops_only_thin_the_dye() {
    let mut patch = chain();
    patch
        .plug("vca1.out", "lfo1.depth", Some(1.0), None)
        .unwrap();
    let mut dye = Dye::new();
    turn(&mut patch, &mut dye, "Tom", "lfo1", "depth", 0.0);
    turn(&mut patch, &mut dye, "Vesper", "vca1", "gain", 0.0);
    let prints = dye.flow(&patch);
    for shares in prints.modules.values() {
        let total: f64 = shares.values().sum();
        assert!((total - 1.0).abs() < 1e-5, "{shares:?}");
    }
    // Vesper's dye comes back round to the lfo, thinned: one hop at 0.5.
    assert_eq!(
        prints.modules["lfo1"],
        shares(&[("Tom", 0.666_667), ("Vesper", 0.333_333)])
    );
}

#[test]
fn nothing_moves_unless_something_changes() {
    let mut patch = chain();
    let mut dye = Dye::new();
    turn(&mut patch, &mut dye, "Tom", "vcf1", "cutoff", 5_000.0);
    let first = dye.flow(&patch);
    for _ in 0..10 {
        assert_eq!(dye.flow(&patch), first);
        assert!(dye.flow(&patch).changed_since(&first).is_empty());
    }
    // A turn to where the knob already is leaves nothing.
    let what = patch.turn("vcf1", "cutoff", 5_000.0, None).unwrap();
    dye.touch("Waffles", &what, &patch);
    assert_eq!(dye.flow(&patch), first);
}

#[test]
fn touches_leave_dye_by_their_size() {
    let mut patch = chain();
    let mut dye = Dye::new();
    let added = patch.add("noise", None, None).unwrap();
    dye.touch("Tom", &added, &patch);
    assert!((dye.deposits()["noise1"]["Tom"] - 1.0).abs() < 1e-12);
    turn(&mut patch, &mut dye, "Tom", "noise1", "colour", 0.5);
    // A quarter of the colour knob's 0..2 range.
    assert!((dye.deposits()["noise1"]["Tom"] - 1.25).abs() < 1e-9);
    let plugged = patch
        .plug("noise1.out", "vcf1.cutoff", Some(-0.4), None)
        .unwrap();
    dye.touch("Waffles", &plugged, &patch);
    assert!((dye.deposits()["vcf1"]["Waffles"] - 0.4).abs() < 1e-6);
    let tempo = What::Tempo {
        from: 90.0,
        to: 100.0,
        desk: false,
    };
    dye.touch("Waffles", &tempo, &patch);
    assert_eq!(dye.deposits().len(), 2);
    let removed = patch.remove("noise1", &[]).unwrap();
    dye.touch("Tom", &removed, &patch);
    assert!(!dye.deposits().contains_key("noise1"));
    assert!(!dye.flow(&patch).modules.contains_key("noise1"));
}

#[test]
fn saved_dye_must_be_positive_numbers() {
    let mut deposits = BTreeMap::new();
    deposits.insert("vcf1".to_string(), shares(&[("Tom", 2.0)]));
    assert!(Dye::from_deposits(deposits.clone()).is_ok());
    deposits.insert("vca1".to_string(), shares(&[("Tom", f64::NAN)]));
    assert!(Dye::from_deposits(deposits.clone()).is_err());
    deposits.insert("vca1".to_string(), shares(&[("Tom", -1.0)]));
    assert!(Dye::from_deposits(deposits).is_err());
    let mut dye = Dye::new();
    dye.deposit("vcf1", "Tom", f64::INFINITY);
    dye.deposit("vcf1", "Tom", 0.0);
    assert!(dye.deposits().is_empty());
}

#[test]
fn fingerprints_survive_the_patch_file() {
    let mut patch = chain();
    let what = patch.turn("vcf1", "cutoff", 5_000.0, None).unwrap();
    let mut dye = std::mem::take(&mut patch.dye);
    dye.touch("Tom", &what, &patch);
    patch.dye = dye;
    let text = serde_json::to_string(&patch.to_file()).unwrap();
    let loaded = Patch::from_file(&serde_json::from_str(&text).unwrap()).unwrap();
    assert_eq!(loaded.dye, patch.dye);
    assert_eq!(loaded.dye.flow(&loaded), patch.dye.flow(&patch));
    // A patch saved before fingerprints loads with no dye.
    let mut old: serde_json::Value = serde_json::to_value(Patch::empty(90.0).to_file()).unwrap();
    old.as_object_mut().unwrap().remove("dye");
    let loaded = Patch::from_file(&serde_json::from_value(old).unwrap()).unwrap();
    assert!(loaded.dye.deposits().is_empty());
    // Dye on a module that is not there is refused.
    let mut file = patch.to_file();
    file.dye
        .insert("ghost1".to_string(), shares(&[("Tom", 1.0)]));
    assert!(Patch::from_file(&file).unwrap_err().contains("ghost1"));
}
