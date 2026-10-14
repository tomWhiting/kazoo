use super::*;
use crate::test_support::{bud, grown, lengthen};

#[test]
fn the_same_history_grows_the_same_tree() {
    let history = grown(300, &[(0, 40, 90), (0, 120, 60), (1, 30, 40)]);
    assert_eq!(Growth::grow(&history), Growth::grow(&history));
}

#[test]
fn growing_never_moves_the_past() {
    let mut history = grown(200, &[(0, 50, 70), (1, 20, 33)]);
    let before = Growth::grow(&history);
    // More on the trunk, more on a bud mid-step, and two new buds, one
    // from a moment in the bud's part-grown last step.
    lengthen(&mut history, 0, 150);
    lengthen(&mut history, 2, 20);
    bud(&mut history, 2, 30, 45);
    bud(&mut history, 0, 300, 25);
    let after = Growth::grow(&history);
    for (m, place) in before.at.iter().enumerate() {
        assert_eq!(after.at[m], *place, "moment {m} moved");
    }
    for (old, new) in before.steps.iter().zip(&after.steps) {
        assert_eq!(
            (old.limb, old.first, old.start, old.end, old.heading),
            (new.limb, new.first, new.start, new.end, new.heading)
        );
    }
    for (m, look) in before.look.iter().enumerate() {
        assert_eq!(after.look[m], *look, "moment {m}'s stretch is redrawn");
    }
    assert_eq!(after.roots[..before.roots.len()], before.roots[..]);
}

#[test]
fn every_moment_on_a_limb_has_a_place() {
    let history = grown(97, &[(0, 10, 17), (1, 16, 1)]);
    let growth = Growth::grow(&history);
    for limb in &history.limbs {
        for &m in &limb.moments {
            assert!(growth.at[m].is_some(), "moment {m}");
        }
    }
    assert_eq!(growth.limb_steps[0].len(), 97_usize.div_ceil(GROWTH_STEP));
    assert_eq!(growth.limb_steps[2].len(), 1);
}

#[test]
fn a_bud_grows_from_the_moment_it_budded_from() {
    let history = grown(64, &[(0, 20, 32)]);
    let growth = Growth::grow(&history);
    let from = history.limbs[1].from.expect("a bud");
    let first = growth.limb_steps[1][0];
    assert_eq!(Some(growth.steps[first].start), growth.at[from]);
    let parent = growth.step_of[from].expect("its step");
    let turned = (growth.steps[first].heading - growth.steps[parent].heading).abs();
    assert!(turned > 0.2, "a bud leaves its limb at an angle: {turned}");
}

#[test]
fn the_trunk_rises_and_nothing_grows_far_below_ground() {
    let history = grown(400, &[(0, 100, 200), (0, 250, 120), (1, 50, 80)]);
    let growth = Growth::grow(&history);
    let tip = growth.steps[*growth.limb_steps[0].last().expect("a trunk")].end;
    let reach = 400_usize.div_ceil(GROWTH_STEP) as f64 * STEP_LENGTH;
    assert!(tip.1 > reach * 0.6, "the trunk rises: {tip:?} of {reach}");
    for step in &growth.steps {
        assert!(step.end.1 > -STEP_LENGTH, "below ground: {step:?}");
    }
}

#[test]
fn thicker_where_more_lived() {
    assert!(width_for(14) < width_for(85));
    assert!(width_for(85) < width_for(141));
    assert!(width_for(0) >= 1.0);
    assert!(width_for(100_000) <= 8.0);
}

#[test]
fn a_tree_with_nothing_grows_nothing() {
    let growth = Growth::grow(&History::default());
    assert!(growth.steps.is_empty());
    assert_eq!(growth.bounds, (0.0, 0.0, 0.0, 0.0));
}

#[test]
fn branches_spread_out_rather_than_pile_up() {
    let history = grown(
        300,
        &[(0, 60, 120), (0, 61, 120), (0, 62, 120), (0, 63, 120)],
    );
    let growth = Growth::grow(&history);
    let tips: Vec<(f64, f64)> = (1..5)
        .map(|l| growth.steps[*growth.limb_steps[l].last().expect("a bud")].end)
        .collect();
    for (i, a) in tips.iter().enumerate() {
        for b in &tips[i + 1..] {
            assert!(
                libm::hypot(a.0 - b.0, a.1 - b.1) > CELL,
                "tips {a:?} and {b:?} pile up"
            );
        }
    }
}

#[test]
fn a_tip_filling_never_redraws_what_is_drawn() {
    // A tip step of 5, then filled to 16 with louder, busier moments: the
    // first five stretches keep their width and colour.
    let mut history = grown(5, &[]);
    for moment in &mut history.moments {
        moment.sound = Some(crate::history::Sound {
            rms_db: -30.0,
            peak_db: -12.0,
            centroid_hz: 300.0,
            low: 0.6,
            mid: 0.3,
            high: 0.1,
            onsets: 1.0,
        });
    }
    let before = Growth::grow(&history);
    lengthen(&mut history, 0, 11);
    for moment in &mut history.moments[5..] {
        moment.modules = 90;
        moment.sound = Some(crate::history::Sound {
            rms_db: -6.0,
            peak_db: -1.0,
            centroid_hz: 3_000.0,
            low: 0.1,
            mid: 0.3,
            high: 0.6,
            onsets: 6.0,
        });
    }
    let after = Growth::grow(&history);
    assert_eq!(after.look[..5], before.look[..5]);
    assert!(after.look[15].expect("drawn").width > after.look[0].expect("drawn").width);
}

#[test]
fn time_stepping_back_never_scrambles_a_limb() {
    // The wall dates a gap when it writes it, after changes made earlier:
    // a step's first moment can be older than the step before it. Here
    // every other step of the trunk is dated an hour back, and a bud grows
    // from a moment in one of those.
    let mut history = grown(160, &[(0, 40, 50)]);
    for (i, moment) in history.moments.iter_mut().enumerate() {
        if (i / GROWTH_STEP) % 2 == 1 {
            moment.time -= 3_600;
        }
    }
    let growth = Growth::grow(&history);
    for (l, steps) in growth.limb_steps.iter().enumerate() {
        let firsts: Vec<usize> = steps.iter().map(|&s| growth.steps[s].first).collect();
        let expected: Vec<usize> = (0..steps.len()).map(|i| i * GROWTH_STEP).collect();
        assert_eq!(firsts, expected, "limb {l}'s steps in order");
        for pair in steps.windows(2) {
            assert_eq!(growth.steps[pair[0]].end, growth.steps[pair[1]].start);
        }
    }
    assert_eq!(
        growth.roots.len(),
        1,
        "the bud grows from the trunk, not apart"
    );
    let from = history.limbs[1].from.expect("a bud");
    assert_eq!(
        Some(growth.steps[growth.limb_steps[1][0]].start),
        growth.at[from]
    );
}

#[test]
fn trees_that_share_nothing_stand_apart_and_stay_put() {
    // Two limbs with no moment in common: two trees, the second to the
    // right of everything grown before it.
    let mut history = grown(80, &[]);
    let n = history.limbs.len();
    history.limbs.push(crate::history::Limb {
        name: "by-hand".to_owned(),
        moments: Vec::new(),
        from: None,
    });
    lengthen(&mut history, n, 40);
    // Its first moment grew from nothing.
    let first = history.limbs[n].moments[0];
    history.moments[first].parent = None;
    let growth = Growth::grow(&history);
    assert_eq!(growth.roots.len(), 2);
    let trunk_right = growth.limb_steps[0]
        .iter()
        .map(|&s| growth.steps[s].start.0.max(growth.steps[s].end.0))
        .fold(f64::MIN, f64::max);
    assert!(growth.roots[1].0 - ROOT_REACH > trunk_right, "{growth:?}");
}

/// The tree's shape, pinned: a change to any constant or to how the tree
/// grows fails here, and is made on purpose, with [`GROWTH_VERSION`].
#[test]
fn the_shape_is_pinned() {
    assert_eq!(GROWTH_VERSION, 2);
    let growth = Growth::grow(&grown(64, &[(0, 20, 32)]));
    let pins: Vec<(usize, f64, f64)> = [0_usize, 2, 3, 5]
        .iter()
        .map(|&s| {
            (
                growth.steps[s].limb,
                growth.steps[s].end.0,
                growth.steps[s].end.1,
            )
        })
        .collect();
    let expected: [(usize, f64, f64); 4] = PINNED;
    for (got, want) in pins.iter().zip(expected) {
        assert_eq!(got.0, want.0);
        assert!(
            (got.1 - want.1).abs() < 1e-9 && (got.2 - want.2).abs() < 1e-9,
            "{pins:?}"
        );
    }
}

/// Steps 0, 2, 3 (the trunk) and 5 (the bud): limb, and where each ends.
const PINNED: [(usize, f64, f64); 4] = [
    (0, 0.440_547_486_106_022_9, 3.975_665_719_409_224),
    (0, 1.078_379_782_930_204_9, 11.947_211_355_570_89),
    (0, 1.529_009_326_612_658_3, 15.921_746_927_730_3),
    (1, 6.709_760_337_413_961, 10.253_693_850_296_017),
];
