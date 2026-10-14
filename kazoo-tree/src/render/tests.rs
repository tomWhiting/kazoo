use super::*;
use crate::growth::Growth;
use crate::test_support::{grown, mark};

fn scene_parts(history: &History) -> (Growth, Vec<Gesture>, Vec<bool>) {
    (
        Growth::grow(history),
        gestures(history),
        line_to(history, history.here),
    )
}

fn picture(
    history: &History,
    selected: Option<usize>,
    view: Option<View>,
    cols: u16,
    rows: u16,
) -> Picture {
    let (growth, gestures, line) = scene_parts(history);
    let view = view.unwrap_or_else(|| View::fit(growth.bounds, cols, rows));
    let scene = Scene {
        history,
        growth: &growth,
        gestures: &gestures,
        days: &[],
        line: &line,
        selected,
        now: 0,
    };
    draw(&scene, &view, cols, rows)
}

#[test]
fn the_whole_tree_draws_in_braille_with_the_wall_on_it() {
    let mut history = grown(200, &[(0, 50, 80)]);
    mark(&mut history, 30, "before the rock", false);
    let growth = Growth::grow(&history);
    let far = View {
        scale: 0.5,
        ..View::fit(growth.bounds, 80, 30)
    };
    assert_eq!(Level::of(far.scale), Level::Far);
    let text = picture(&history, None, Some(far), 80, 30).text();
    assert!(
        text.chars().any(|c| ('\u{2801}'..='\u{28ff}').contains(&c)),
        "{text}"
    );
    assert!(text.contains('◉'), "{text}");
    assert!(text.contains('◆'), "{text}");
    assert!(text.contains("the wall is here"), "{text}");
    // Far out, marks hang without their names.
    assert!(!text.contains("before the rock"), "{text}");
}

#[test]
fn names_come_with_zooming_in() {
    let mut history = grown(60, &[]);
    mark(&mut history, 20, "before the rock", false);
    let growth = Growth::grow(&history);
    let p = growth.at[20].expect("placed");
    let near = View {
        centre: p,
        scale: 1.0,
    };
    assert_eq!(Level::of(near.scale), Level::Near);
    let text = picture(&history, None, Some(near), 80, 30).text();
    assert!(text.contains("before the rock"), "{text}");
    let close = View {
        centre: p,
        scale: 8.0,
    };
    let text = picture(&history, None, Some(close), 100, 40).text();
    assert!(
        text.contains("Tom turned"),
        "every moment says its words close in: {text}"
    );
}

#[test]
fn labels_never_cover_a_glyph_or_leave_the_picture() {
    let mut history = grown(120, &[(0, 30, 60), (0, 31, 60)]);
    for m in (0..history.moments.len()).step_by(7) {
        mark(
            &mut history,
            m,
            &format!("mark number {m} with a long name"),
            m % 2 == 0,
        );
    }
    let (growth, gestures, line) = scene_parts(&history);
    let view = View {
        centre: growth.at[40].expect("placed"),
        scale: 2.0,
    };
    let scene = Scene {
        history: &history,
        growth: &growth,
        gestures: &gestures,
        days: &[],
        line: &line,
        selected: Some(40),
        now: 0,
    };
    let drawn = draw(&scene, &view, 60, 20);
    assert_eq!(drawn.cells.len(), 60 * 20);
    let glyphs = drawn
        .cells
        .iter()
        .filter(|c| matches!(c.text, Some('◉' | '◆' | '▪' | '•')))
        .count();
    let mut expected = 0;
    for (m, at) in growth.at.iter().enumerate() {
        let Some(p) = at else { continue };
        let (x, y) = view.to_dots(*p, 60, 20);
        let shown =
            history.marks.iter().any(|mark| mark.moment == m) || history.here == Some(m) || m == 40;
        if shown && (0.0..120.0).contains(&x) && (0.0..80.0).contains(&y) {
            expected += 1;
        }
    }
    // Several marks can share a cell; every cell that should hold a glyph
    // still holds one (none was overwritten by a label).
    assert!(glyphs >= 1 && glyphs <= expected, "{glyphs} of {expected}");
    for line in drawn.text().lines() {
        assert!(line.chars().count() <= 60);
    }
}

#[test]
fn the_selected_moment_is_marked_and_says_itself() {
    let history = grown(40, &[]);
    let growth = Growth::grow(&history);
    let view = View {
        centre: growth.at[10].expect("placed"),
        scale: 1.0,
    };
    let drawn = picture(&history, Some(10), Some(view), 80, 30);
    assert_eq!(drawn.cells.iter().filter(|c| c.selected).count(), 1);
    assert!(drawn.text().contains("cutoff 10"), "{}", drawn.text());
}

#[test]
fn levels_follow_the_scale() {
    assert_eq!(Level::of(0.1), Level::Far);
    assert_eq!(Level::of(1.0), Level::Near);
    assert_eq!(Level::of(3.0), Level::Close);
    assert_eq!(Level::of(10.0), Level::Every);
}

#[test]
fn gestures_fold_one_seats_same_change() {
    let history = grown(12, &[]);
    let folded = gestures(&history);
    // Summaries change subject every five moments: 0-4, 5-9, 10-11.
    let counts: Vec<usize> = folded.iter().map(|g| g.count).collect();
    assert_eq!(counts, [5, 5, 2]);
    assert_eq!(
        folded[0].words(&history),
        "Tom turned vcf0 cutoff 0 → 4 (5 changes)"
    );
    assert_eq!(
        folded[2].words(&history),
        "Tom turned vcf2 cutoff 10 → 11 (2 changes)"
    );
    assert_eq!(folded[0].last, 4);
}

#[test]
fn the_walls_line_runs_from_the_root_to_where_it_is() {
    let history = grown(10, &[(0, 3, 4)]);
    let line = line_to(&history, history.here);
    let on: Vec<usize> = (0..line.len()).filter(|&m| line[m]).collect();
    assert_eq!(on, [0, 1, 2, 3, 10, 11, 12, 13]);
}

#[test]
fn a_view_round_trips_between_cells_and_the_world() {
    let view = View {
        centre: (10.0, 50.0),
        scale: 2.0,
    };
    let world = view.to_world(30, 12, 80, 24);
    let (x, y) = view.to_dots(world, 80, 24);
    assert!(
        (x - 61.0).abs() < 1e-3 && (y - 50.0).abs() < 1e-3,
        "{x} {y}"
    );
}

#[test]
fn a_new_day_on_a_limb_is_marked_in_the_viewers_own_zone() {
    // Moments three hours apart from 20:00 UTC on Tuesday 13 Oct 2026. In
    // Melbourne (UTC+11) that is 07:00 on the 14th, and the 15th begins
    // at moment 6 (01:00 there). The bud grows from moment 2 (13:00 on the
    // 14th there) and its first moment, 10, is 13:00 on the 15th there.
    let start = 1_791_921_600; // 2026-10-13 20:00 UTC
    let mut history = grown(10, &[(0, 2, 3)]);
    for (i, moment) in history
        .moments
        .iter_mut()
        .map(std::sync::Arc::make_mut)
        .enumerate()
    {
        moment.time = start + 3 * 3_600 * i64::try_from(i).expect("small");
    }
    let melbourne = chrono::FixedOffset::east_opt(11 * 3_600).expect("an offset");
    let marked: Vec<(usize, String)> = days(&history, &melbourne)
        .into_iter()
        .map(|d| (d.moment, d.words))
        .collect();
    assert_eq!(
        marked,
        [
            (0, "Wed 14 Oct 2026".to_owned()),
            (6, "Thu 15 Oct 2026".to_owned()),
            (10, "Thu 15 Oct 2026".to_owned()),
        ]
    );
    // In UTC the trunk starts on the 13th and the 14th begins at moment 2
    // (02:00); the bud's first moment is 02:00 on the 15th.
    let utc: Vec<(usize, String)> = days(&history, &chrono::Utc)
        .into_iter()
        .map(|d| (d.moment, d.words))
        .collect();
    assert_eq!(
        utc,
        [
            (0, "Tue 13 Oct 2026".to_owned()),
            (2, "Wed 14 Oct 2026".to_owned()),
            (10, "Thu 15 Oct 2026".to_owned()),
        ]
    );
}

#[test]
fn a_tip_grown_in_the_last_hour_wears_a_light_and_its_limb_is_unchanged() {
    let mut history = grown(64, &[(0, 20, 10)]);
    // The wall's ◉ would sit over the bud's tip's light.
    history.here = None;
    let growth = Growth::grow(&history);
    let (_, gestures, line) = scene_parts(&history);
    let view = View::fit(growth.bounds, 80, 30);
    let at = |now: i64| Scene {
        history: &history,
        growth: &growth,
        gestures: &gestures,
        days: &[],
        line: &line,
        selected: None,
        now,
    };
    let lights = |picture: &Picture| {
        picture
            .cells
            .iter()
            .filter(|c| c.text == Some(Glyph::Growing.char()))
            .count()
    };
    let tip = history
        .moments
        .iter()
        .map(|m| m.time)
        .max()
        .expect("moments");
    let soon = draw(&at(tip + 60), &view, 80, 30);
    let later = draw(&at(tip + 2 * 3_600), &view, 80, 30);
    assert_eq!(lights(&soon), 2, "both tips grew in the last hour");
    assert_eq!(lights(&later), 0, "nothing grew in the last hour");
    // The limbs themselves are drawn the same either way: only the cells
    // the lights sit on differ.
    for (a, b) in soon.cells.iter().zip(&later.cells) {
        if a.text != Some(Glyph::Growing.char()) {
            assert_eq!((a.dots, a.colour, a.text), (b.dots, b.colour, b.text));
        }
    }
}
