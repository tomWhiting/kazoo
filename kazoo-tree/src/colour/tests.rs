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

/// A tint of `pairs`' modules, each at its own place
/// (`modules/<kind><i>.toml`), every one leaning `lean` by its settings.
fn tint(pairs: &[(&str, u16)], lean: f64) -> Tint {
    let mut tint = Tint::default();
    for &(kind, n) in pairs {
        for i in 0..n {
            tint.add(
                kind,
                &format!("modules/{kind}{i}.toml"),
                Some(Settings {
                    turn: lean,
                    light: 0.0,
                }),
            );
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
        let c = colour(&tint(&[(kind, 64)], 0.0), None);
        assert!(
            hue_gap(hue_of(c), HUES[f]) < 20.0,
            "{kind}: {c:?} is not near {}",
            HUES[f]
        );
    }
}

#[test]
fn a_module_shade_stays_within_its_family_spread() {
    for lean in [-1.0, -0.5, 0.0, 0.3, 1.0] {
        for place in [
            "modules/vcf1.toml",
            "modules/vcf2.toml",
            "modules/vcf77.toml",
        ] {
            let mut one = Tint::default();
            one.add(
                "vcf",
                place,
                Some(Settings {
                    turn: lean,
                    light: lean,
                }),
            );
            let c = colour(&one, None);
            assert!(
                hue_gap(hue_of(c), HUES[2]) <= SPREAD + 3.0,
                "{lean} {place}: {c:?}"
            );
        }
    }
}

#[test]
fn plumbing_and_unknown_kinds_have_no_colour() {
    for kind in ["out", "mix", "vca", "scope", "readout", "a-kind-from-later"] {
        assert_eq!(family_of(kind), None, "{kind}");
    }
    let plumbing = tint(&[("out", 2), ("mix", 3)], 0.0);
    assert_eq!(plumbing, Tint::default());
    assert_eq!(colour(&plumbing, None), PLAIN);
    let voices = tint(&[("vco", 4)], 0.0);
    assert_eq!(colour(&voices.plus(&plumbing), None), colour(&voices, None));
}

#[test]
fn a_small_turn_is_a_small_move_and_turning_back_is_exact() {
    let wall = tint(&[("vco", 20), ("vcf", 20), ("hall", 10), ("lfo", 10)], 0.0);
    let set = |lean: f64| {
        Some(Settings {
            turn: lean,
            light: 0.0,
        })
    };
    let mut turned = wall;
    turned.remove("vcf", "modules/vcf3.toml", set(0.0));
    turned.add("vcf", "modules/vcf3.toml", set(0.05));
    assert_ne!(turned, wall);
    let (before, after) = (hue_of(colour(&wall, None)), hue_of(colour(&turned, None)));
    assert!(hue_gap(before, after) < 1.0, "{before} {after}");
    turned.remove("vcf", "modules/vcf3.toml", set(0.05));
    turned.add("vcf", "modules/vcf3.toml", set(0.0));
    assert_eq!(turned, wall);
}

#[test]
fn settings_are_the_knobs_and_cables_and_nothing_else() {
    let file = |cutoff: &str, name: &str, birth: &str, cable: u32, amount: &str, dye: &str| {
        format!(
            "kind = \"vcf\"\nname = \"{name}\"\nbirth = \"{birth}\"\n\n[knobs]\ncutoff = {cutoff}\nresonance = 0.2\nmode = 0\n\n[inputs]\ncutoff = {{ cable = {cable}, from = \"lfo1.out\", amount = {amount} }}\n\n[dye]\nCassio = {dye}\n"
        )
    };
    let read = |text: &str| settings_of(text).expect("a module's file");
    let base = read(&file("1000.0", "low", "b1", 150, "-0.4", "3.21"));
    // Its name, birth, cable number and dye are not its sound.
    assert_eq!(
        read(&file("1000.0", "high", "b2", 999, "-0.4", "9.0")),
        base
    );
    // A small turn of one knob is a small lean; a big one, further.
    let small = read(&file("1020.0", "low", "b1", 150, "-0.4", "3.21"));
    let big = read(&file("9000.0", "low", "b1", 150, "-0.4", "3.21"));
    let gap = |a: Settings| (a.turn - base.turn).abs() + (a.light - base.light).abs();
    assert!(gap(small) > 0.0 && gap(small) < 0.05, "{small:?} {base:?}");
    assert!(gap(big) > gap(small) * 5.0, "{big:?} {small:?}");
    // A cable's amount leans it too.
    assert_ne!(read(&file("1000.0", "low", "b1", 150, "0.4", "3.21")), base);
    // A kind this tree does not know still has settings; nonsense has none.
    assert!(settings_of("kind = \"later\"\n\n[knobs]\nwarp = 3.0\n").is_some());
    assert_eq!(settings_of("not a module ["), None);
    assert_eq!(settings_of("name = \"no kind\"\n"), None);
    for lean in [base, small, big] {
        assert!(
            lean.turn.abs() <= 1.0 && lean.light.abs() <= 1.0,
            "{lean:?}"
        );
    }
}

#[test]
fn a_wall_of_other_settings_is_another_shade() {
    let a = colour(&tint(&[("vco", 3), ("vcf", 3)], 0.0), None);
    let b = colour(&tint(&[("vco", 3), ("vcf", 3)], 0.8), None);
    assert_ne!(a, b);
}

#[test]
fn the_blend_sits_between_its_families() {
    let alive = tint(&[("vco", 40), ("vcf", 40)], 0.0);
    let hue = hue_of(colour(&alive, None));
    // Between amber (32) and green (125).
    assert!(hue > 32.0 && hue < 125.0, "{hue}");
}

#[test]
fn one_family_is_more_vivid_than_a_mix() {
    let spread = |(r, g, b): (u8, u8, u8)| r.max(g).max(b) - r.min(g).min(b);
    let one = colour(&tint(&[("vcf", 8)], 0.0), None);
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
            0.0,
        ),
        None,
    );
    assert!(spread(one) > spread(mix), "{one:?} {mix:?}");
    assert_ne!(mix, PLAIN, "a mix still has a colour");
}

#[test]
fn louder_is_lighter_and_nonsense_still_draws() {
    let alive = tint(&[("vco", 3)], 0.0);
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
fn a_blend_is_the_mean_and_a_few_modules_among_many_moments_still_count() {
    let one = tint(&[("vco", 6), ("hall", 2)], 0.2);
    let mut three = Tint::default();
    for _ in 0..3 {
        three = three.plus(&one);
    }
    assert_eq!(blend(&three, 3.0, None), colour(&one, None));
    assert_eq!(three.minus(&three), Tint::default());
    // Two moments with nothing coloured and one with a single voice: a
    // colour, not grey, and the voice's hue.
    let voice = tint(&[("vco", 1)], 0.0);
    let thin = blend(&voice, 3.0, None);
    assert_ne!(thin, PLAIN);
    assert!(hue_gap(hue_of(thin), hue_of(colour(&voice, None))) < 3.0);
    assert_eq!(blend(&Tint::default(), 3.0, None), PLAIN);
    assert_eq!(blend(&voice, 0.0, None), PLAIN);
    assert_eq!(blend(&voice, f64::NAN, None), PLAIN);
}

#[test]
fn a_moment_of_few_modules_is_quieter_than_a_full_wall() {
    let spread = |(r, g, b): (u8, u8, u8)| r.max(g).max(b) - r.min(g).min(b);
    let few = colour(&tint(&[("vcf", 1)], 0.0), None);
    let full = colour(&tint(&[("vcf", 12)], 0.0), None);
    assert!(spread(full) > spread(few), "{few:?} {full:?}");
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
