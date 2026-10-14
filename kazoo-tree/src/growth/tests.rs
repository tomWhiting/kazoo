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
    let reach = 400_usize.div_ceil(GROWTH_STEP) as f32 * STEP_LENGTH;
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
    let tips: Vec<(f32, f32)> = (1..5)
        .map(|l| growth.steps[*growth.limb_steps[l].last().expect("a bud")].end)
        .collect();
    for (i, a) in tips.iter().enumerate() {
        for b in &tips[i + 1..] {
            assert!(
                (a.0 - b.0).hypot(a.1 - b.1) > CELL,
                "tips {a:?} and {b:?} pile up"
            );
        }
    }
}
