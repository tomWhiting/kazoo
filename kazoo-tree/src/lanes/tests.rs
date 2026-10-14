use super::*;
use crate::test_support::grown;

fn graphs(history: &History) -> Vec<(usize, String)> {
    let lanes = layout(history);
    let crossing = lanes.crossing(0, lanes.rows.len());
    (0..lanes.rows.len())
        .map(|r| (lanes.rows[r].moment, lanes.graph(history, &crossing, r)))
        .collect()
}

#[test]
fn one_limb_is_one_lane_newest_first() {
    let history = grown(3, &[]);
    assert_eq!(
        graphs(&history),
        [
            (2, "◉".to_owned()),
            (1, "●".to_owned()),
            (0, "●".to_owned())
        ]
    );
}

#[test]
fn a_bud_has_its_own_lane_and_joins_where_it_grew() {
    // Trunk 0..4; a bud of two from moment 1 (moments 4 and 5), made after
    // the trunk's, so newest first it is on top. The trunk keeps the left
    // lane, empty above its tip.
    let history = grown(4, &[(0, 1, 2)]);
    let rows = graphs(&history);
    assert_eq!(
        rows,
        [
            (5, "  ◉".to_owned()),
            (4, "  ●".to_owned()),
            (3, "● │".to_owned()),
            (2, "● │".to_owned()),
            (1, "●─╯".to_owned()),
            (0, "●".to_owned()),
        ],
        "{rows:#?}"
    );
}

#[test]
fn every_moment_on_a_limb_has_one_row() {
    let history = grown(40, &[(0, 5, 10), (0, 20, 7), (1, 3, 4)]);
    let rows = layout(&history).rows;
    assert_eq!(rows.len(), history.moments.len());
    let mut seen: Vec<usize> = rows.iter().map(|row| row.moment).collect();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), history.moments.len());
}

#[test]
fn a_bud_dated_before_its_moment_still_sits_above_it_on_one_lane() {
    let mut history = grown(20, &[(0, 5, 5)]);
    let bud = history.limbs[1].moments.clone();
    for &m in &bud {
        std::sync::Arc::make_mut(&mut history.moments[m]).time = 500;
    }
    let rows = layout(&history).rows;
    let row_of = |m: usize| rows.iter().position(|row| row.moment == m).expect("a row");
    let from = history.limbs[1].from.expect("a bud");
    for &m in &bud {
        assert!(
            row_of(m) < row_of(from),
            "{m} is above the moment it grew from"
        );
    }
    let lanes: Vec<usize> = bud.iter().map(|&m| rows[row_of(m)].lane).collect();
    assert!(
        lanes.windows(2).all(|w| w[0] == w[1]),
        "one lane: {lanes:?}"
    );
    for pair in bud.windows(2) {
        assert!(
            row_of(pair[1]) < row_of(pair[0]),
            "newest first along the bud"
        );
    }
}
