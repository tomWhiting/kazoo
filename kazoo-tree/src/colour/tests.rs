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

/// A tint of `pairs`' modules, each at its own place (`modules/<kind><i>.toml`)
/// with settings named `settings`.
fn tint(pairs: &[(&str, u16)], settings: &str) -> Tint {
    let mut tint = Tint::default();
    for &(kind, n) in pairs {
        for i in 0..n {
            tint.add(kind, &format!("modules/{kind}{i}.toml"), settings);
        }
    }
    tint
}

fn hue_gap(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
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
        // Enough modules that their shades' turns even out around the
        // family's hue.
        let c = colour(&tint(&[(kind, 64)], "a"), None);
        assert!(
            hue_gap(hue_of(c), HUES[f]) < 20.0,
            "{kind}: {c:?} is not near {}",
            HUES[f]
        );
    }
}

#[test]
fn a_module_shade_stays_within_its_family_spread() {
    for blob in ["a", "b", "c", "d", "e", "f", "0123", "ffff"] {
        let c = colour(&tint(&[("vcf", 1)], blob), None);
        assert!(hue_gap(hue_of(c), HUES[2]) <= SPREAD + 3.0, "{blob}: {c:?}");
    }
}

#[test]
fn plumbing_and_unknown_kinds_have_no_colour() {
    for kind in ["out", "mix", "vca", "scope", "readout", "a-kind-from-later"] {
        assert_eq!(family_of(kind), None, "{kind}");
    }
    let plumbing = tint(&[("out", 2), ("mix", 3)], "a");
    assert_eq!(plumbing, Tint::default());
    assert_eq!(colour(&plumbing, None), PLAIN);
    let voices = tint(&[("vco", 4)], "a");
    assert_eq!(colour(&voices.plus(&plumbing), None), colour(&voices, None));
}

#[test]
fn the_same_settings_give_the_same_colour_and_new_ones_a_near_one() {
    let wall = tint(&[("vco", 20), ("vcf", 20), ("hall", 10), ("lfo", 10)], "a");
    assert_eq!(
        wall,
        tint(&[("vco", 20), ("vcf", 20), ("hall", 10), ("lfo", 10)], "a")
    );
    // One knob turned: one module's settings change.
    let mut turned = wall;
    turned.remove("vcf", "modules/vcf3.toml", "a");
    turned.add("vcf", "modules/vcf3.toml", "b");
    assert_ne!(turned, wall);
    let (before, after) = (hue_of(colour(&wall, None)), hue_of(colour(&turned, None)));
    // One shade of sixty moves by up to twice the spread: the whole moves a
    // few degrees, never a jump to another colour.
    assert!(hue_gap(before, after) < 8.0, "{before} {after}");
    // Turned back, exactly the colour it was.
    turned.remove("vcf", "modules/vcf3.toml", "b");
    turned.add("vcf", "modules/vcf3.toml", "a");
    assert_eq!(turned, wall);
}

#[test]
fn a_wall_of_new_settings_is_a_different_shade() {
    let a = colour(&tint(&[("vco", 3), ("vcf", 3)], "a"), None);
    let b = colour(&tint(&[("vco", 3), ("vcf", 3)], "zzz"), None);
    assert_ne!(a, b);
}

#[test]
fn the_blend_sits_between_its_families() {
    let alive = tint(&[("vco", 40), ("vcf", 40)], "a");
    let hue = hue_of(colour(&alive, None));
    // Between amber (32) and green (125).
    assert!(hue > 32.0 && hue < 125.0, "{hue}");
}

#[test]
fn one_family_is_more_vivid_than_a_mix() {
    let spread = |(r, g, b): (u8, u8, u8)| r.max(g).max(b) - r.min(g).min(b);
    let one = colour(&tint(&[("vcf", 8)], "a"), None);
    let mix = colour(
        &tint(
            &[
                ("vco", 2),
                ("kick", 2),
                ("vcf", 2),
                ("seq", 2),
                ("hall", 2),
                ("lfo", 2),
            ],
            "a",
        ),
        None,
    );
    assert!(spread(one) > spread(mix), "{one:?} {mix:?}");
    assert_ne!(mix, PLAIN, "a mix still has a colour");
}

#[test]
fn louder_is_lighter_and_nonsense_still_draws() {
    let alive = tint(&[("vco", 3)], "a");
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
    let quiet = colour(&alive, Some(&sound(-45.0)));
    let loud = colour(&alive, Some(&sound(-12.0)));
    assert!(sum(loud) > sum(quiet), "{quiet:?} {loud:?}");
    let nonsense = colour(&alive, Some(&sound(f32::NAN)));
    assert!(sum(nonsense) > 0);
    assert_eq!(hsl(f64::NAN, f64::INFINITY, f64::NAN), hsl(0.0, 0.0, 0.5));
}

#[test]
fn a_mean_divides_every_sum_and_rounds_to_nearest() {
    let sum = Tint {
        x: 10,
        y: -10,
        light: 7,
        count: 9,
    };
    assert_eq!(
        sum.mean(4),
        Tint {
            x: 3,
            y: -3,
            light: 2,
            count: 2
        }
    );
    assert_eq!(sum.mean(1), sum);
    assert_eq!(sum.mean(0), sum);
    assert_eq!(sum.minus(&sum), Tint::default());
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
