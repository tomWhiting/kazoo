use super::*;

fn kinds(pairs: &[(&str, u16)]) -> Kinds {
    let mut kinds = Kinds::default();
    for &(kind, n) in pairs {
        for _ in 0..n {
            kinds.add(kind);
        }
    }
    kinds
}

fn hue_of((red, green, blue): (u8, u8, u8)) -> f64 {
    let (red, green, blue) = (f64::from(red), f64::from(green), f64::from(blue));
    let max = red.max(green).max(blue);
    let spread = max - red.min(green).min(blue);
    assert!(spread > 0.0, "a grey has no hue");
    let sector = if (max - red).abs() < f64::EPSILON {
        ((green - blue) / spread).rem_euclid(6.0)
    } else if (max - green).abs() < f64::EPSILON {
        (blue - red) / spread + 2.0
    } else {
        (red - green) / spread + 4.0
    };
    sector * 60.0
}

fn near(a: f64, b: f64) -> bool {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d) < 6.0
}

#[test]
fn a_kind_is_its_id_without_its_number() {
    assert_eq!(kind_of("noiseperc12"), "noiseperc");
    assert_eq!(kind_of("vcf0"), "vcf");
    assert_eq!(kind_of("hall"), "hall");
    assert_eq!(kind_of("sampler12_3"), "sampler12");
}

#[test]
fn every_family_has_its_own_hue() {
    for (kind, f) in [
        ("vco", 0),
        ("kick", 1),
        ("vcf", 2),
        ("seq", 3),
        ("hall", 4),
        ("lfo", 5),
        ("tape", 6),
        ("fuzz", 7),
    ] {
        assert_eq!(family_of(kind), Some(f), "{kind}");
        let c = colour(&kinds(&[(kind, 3)]), &Kinds::default(), None);
        assert!(
            near(hue_of(c), HUES[f]),
            "{kind}: {c:?} is not near {}",
            HUES[f]
        );
    }
}

#[test]
fn plumbing_and_unknown_kinds_have_no_colour() {
    for kind in ["out", "mix", "vca", "scope", "readout", "a-kind-from-later"] {
        assert_eq!(family_of(kind), None, "{kind}");
    }
    let plumbing = kinds(&[("out", 2), ("mix", 3)]);
    assert_eq!(plumbing.total(), 0);
    assert_eq!(colour(&plumbing, &Kinds::default(), None), PLAIN);
    // Plumbing among the others changes nothing.
    let voices = kinds(&[("vco", 4)]);
    assert_eq!(
        colour(&voices.plus(&plumbing), &Kinds::default(), None),
        colour(&voices, &Kinds::default(), None)
    );
}

#[test]
fn the_blend_sits_between_its_families_and_leans_toward_what_was_played() {
    let alive = kinds(&[("vco", 5), ("vcf", 5)]);
    let still = hue_of(colour(&alive, &Kinds::default(), None));
    // Between amber (32) and green (125).
    assert!(still > 32.0 && still < 125.0, "{still}");
    let filters_played = hue_of(colour(&alive, &kinds(&[("vcf", 4)]), None));
    let voices_played = hue_of(colour(&alive, &kinds(&[("vco", 4)]), None));
    assert!(
        filters_played > still && voices_played < still,
        "{voices_played} {still} {filters_played}"
    );
}

#[test]
fn one_family_is_more_vivid_than_a_mix() {
    let spread = |(r, g, b): (u8, u8, u8)| r.max(g).max(b) - r.min(g).min(b);
    let one = colour(&kinds(&[("vcf", 8)]), &Kinds::default(), None);
    let mix = colour(
        &kinds(&[
            ("vco", 2),
            ("kick", 2),
            ("vcf", 2),
            ("seq", 2),
            ("hall", 2),
            ("lfo", 2),
        ]),
        &Kinds::default(),
        None,
    );
    assert!(spread(one) > spread(mix), "{one:?} {mix:?}");
    assert_ne!(mix, PLAIN, "a mix still has a colour");
}

#[test]
fn louder_is_lighter_and_nonsense_still_draws() {
    let alive = kinds(&[("vco", 3)]);
    let sound = |rms_db: f32| Sound {
        rms_db,
        peak_db: 0.0,
        centroid_hz: 600.0,
        low: 0.3,
        mid: 0.6,
        high: 0.1,
        onsets: 2.0,
    };
    let sum = |(r, g, b): (u8, u8, u8)| u32::from(r) + u32::from(g) + u32::from(b);
    let quiet = colour(&alive, &Kinds::default(), Some(&sound(-45.0)));
    let loud = colour(&alive, &Kinds::default(), Some(&sound(-12.0)));
    assert!(sum(loud) > sum(quiet), "{quiet:?} {loud:?}");
    let nonsense = colour(&alive, &Kinds::default(), Some(&sound(f32::NAN)));
    assert!(sum(nonsense) > 0);
    assert_eq!(hsl(f64::NAN, f64::INFINITY, f64::NAN), hsl(0.0, 0.0, 0.5));
}

#[test]
fn taking_away_what_was_not_there_is_said() {
    let mut alive = kinds(&[("vco", 1)]);
    assert!(alive.remove("vco"));
    assert!(!alive.remove("vco"));
    assert!(
        alive.remove("out"),
        "plumbing is never counted, so never missing"
    );
    assert_eq!(alive.total(), 0);
}

#[test]
fn every_kind_the_wall_has_has_a_family_or_is_plumbing() {
    let plumbing = ["out", "mix", "vca", "scope", "readout"];
    let missing: Vec<(&str, &str)> = kazoo_wall::catalogue::registry()
        .kinds()
        .iter()
        .filter(|spec| family_of(spec.name).is_none() && !plumbing.contains(&spec.name))
        .map(|spec| (spec.name, spec.family))
        .collect();
    assert!(
        missing.is_empty(),
        "kinds with no colour family: {missing:?}"
    );
}
