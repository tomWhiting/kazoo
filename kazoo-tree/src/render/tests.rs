use super::*;
use crate::growth::Growth;
use crate::test_support::{batch, grown, mark};

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
    // A bud grown on the day it started says its day too (Tom, 15 Oct:
    // "this new branch was on the 15th too").
    let mut same_day = history.clone();
    let fork = same_day.limbs[1].from.expect("a bud");
    let bud_start = same_day.limbs[1].moments[0];
    let fork_time = same_day.moments[fork].time;
    std::sync::Arc::make_mut(&mut same_day.moments[bud_start]).time = fork_time + 60;
    assert!(
        days(&same_day, &melbourne)
            .iter()
            .any(|d| d.moment == bud_start && d.words == "Wed 14 Oct 2026"),
        "a new branch always says its day"
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

const STEPS: [&str; 3] = [
    "added lfo3",
    "patched lfo3.out to vcf1.cutoff",
    "turned vcf1 res 0.4",
];

#[test]
fn a_batch_is_a_knot_with_its_note_and_opens_close_in() {
    let mut history = grown(60, &[]);
    batch(&mut history, 20, "the brass breathes", &STEPS);
    let growth = Growth::grow(&history);
    let p = growth.at[20].expect("placed");
    let far = View {
        centre: p,
        scale: 0.5,
    };
    let text = picture(&history, None, Some(far), 80, 30).text();
    assert!(text.contains('❖'), "{text}");
    assert!(
        !text.contains("the brass breathes"),
        "far out a knot has no words: {text}"
    );
    let near = View {
        centre: p,
        scale: 1.0,
    };
    let text = picture(&history, None, Some(near), 80, 30).text();
    let knot = text
        .lines()
        .find(|line| line.contains('❖'))
        .expect("the knot");
    assert!(
        knot.contains("the brass breathes"),
        "the note is on the knot's row: {text}"
    );
    assert!(
        !text.contains("added lfo3"),
        "near, the batch stays closed: {text}"
    );
    let every = View {
        centre: p,
        scale: 12.0,
    };
    assert_eq!(Level::of(every.scale), Level::Every);
    let text = picture(&history, None, Some(every), 120, 40).text();
    for step in STEPS {
        assert!(text.contains(&format!("· {step}")), "{step}: {text}");
    }
    let note = text
        .lines()
        .position(|l| l.contains("the brass breathes"))
        .expect("the note");
    let first = text
        .lines()
        .position(|l| l.contains("· added lfo3"))
        .expect("a step");
    assert_eq!(first, note + 1, "the changes hang under the note: {text}");
}

#[test]
fn a_selected_batch_says_its_note_and_its_changes() {
    let mut history = grown(40, &[]);
    batch(&mut history, 10, "the brass breathes", &STEPS);
    let growth = Growth::grow(&history);
    let view = View {
        centre: growth.at[10].expect("placed"),
        scale: 1.0,
    };
    let text = picture(&history, Some(10), Some(view), 80, 30).text();
    assert!(text.contains("the brass breathes"), "{text}");
    assert!(text.contains("· turned vcf1 res 0.4"), "{text}");
}

#[test]
fn a_batch_with_more_changes_than_room_says_exactly_how_many_more() {
    let mut history = grown(40, &[]);
    let steps: Vec<String> = (0..30)
        .map(|i| format!("turned vco{i} pitch 0.{i}"))
        .collect();
    let steps: Vec<&str> = steps.iter().map(String::as_str).collect();
    batch(&mut history, 10, "a long one", &steps);
    let growth = Growth::grow(&history);
    let view = View {
        centre: growth.at[10].expect("placed"),
        scale: 1.0,
    };
    let text = picture(&history, Some(10), Some(view), 80, 12).text();
    let lines: Vec<&str> = text.lines().collect();
    let note = lines
        .iter()
        .position(|l| l.contains("a long one"))
        .expect("the note");
    let below = 12 - note - 1;
    for (i, line) in lines[note + 1..note + below].iter().enumerate() {
        assert!(line.contains(&format!("· turned vco{i} pitch")), "{text}");
    }
    let more = format!("· and {} more", 30 - (below - 1));
    assert!(lines[11].contains(&more), "{more}: {text}");
}

#[test]
fn a_list_that_meets_something_drawn_ends_saying_how_many_more() {
    let mut drawn = Picture::new(60, 10);
    let mut taken = vec![false; drawn.cells.len()];
    // Something already drawn three rows under the note.
    for col in 0..60 {
        let i = drawn.index(col, 3).expect("in the picture");
        taken[i] = true;
    }
    let batch = Batch {
        note: "five".to_owned(),
        steps: (1..=5).map(|i| format!("step {i}")).collect(),
        part: None,
        comments: Vec::new(),
    };
    hang_steps(&mut drawn, &batch, (0, 0), &mut taken);
    let text = drawn.text();
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[1].contains("· step 1"), "{text}");
    assert!(lines[2].contains("· and 4 more"), "{text}");
    assert!(!lines[2].contains("step 2"), "{text}");
    // With nowhere for even the count, nothing hangs.
    let mut full = Picture::new(60, 10);
    let mut taken = vec![false; full.cells.len()];
    for col in 0..60 {
        let i = full.index(col, 1).expect("in the picture");
        taken[i] = true;
    }
    hang_steps(&mut full, &batch, (0, 0), &mut taken);
    assert!(!full.text().contains('·'), "{}", full.text());
}

#[test]
fn a_selected_batchs_changes_never_take_the_walls_words() {
    let mut history = grown(40, &[]);
    // The wall gone back to moment 20; the batch grew after it, so it is
    // drawn above, with the wall's place under its note.
    let here = 20;
    history.here = Some(here);
    let steps: Vec<String> = (0..20).map(|i| format!("turned vco{i} pitch")).collect();
    let steps: Vec<&str> = steps.iter().map(String::as_str).collect();
    batch(&mut history, here + 3, "above the wall", &steps);
    let growth = Growth::grow(&history);
    // Close enough in that the two are on rows of their own, the wall's
    // under the batch's note, where its changes would hang.
    let view = View {
        centre: growth.at[here].expect("placed"),
        scale: 4.0,
    };
    let text = picture(&history, Some(here + 3), Some(view), 80, 30).text();
    let lines: Vec<&str> = text.lines().collect();
    let note = lines
        .iter()
        .position(|l| l.contains("above the wall"))
        .expect("the note");
    let wall = lines
        .iter()
        .position(|l| l.contains('◉'))
        .expect("the wall's place");
    assert!(wall > note, "the wall is drawn under the note: {text}");
    assert!(lines[wall].contains("the wall is here"), "{text}");
    // No room under the note once the wall's words are hung: the list
    // stays closed rather than shown cut short (the details list it all).
    assert!(!text.contains("· turned"), "{text}");
}

#[test]
fn close_in_a_batch_says_its_note() {
    let mut history = grown(60, &[]);
    batch(&mut history, 30, "the brass breathes", &STEPS);
    let growth = Growth::grow(&history);
    let close = View {
        centre: growth.at[30].expect("placed"),
        scale: 4.0,
    };
    assert_eq!(Level::of(close.scale), Level::Close);
    let text = picture(&history, None, Some(close), 100, 40).text();
    let knot = text
        .lines()
        .find(|line| line.contains('❖'))
        .expect("the knot");
    assert!(knot.contains("the brass breathes"), "{text}");
    assert!(
        !text.contains("added lfo3"),
        "close, the batch stays closed: {text}"
    );
}

/// A tree whose moments alternate a voice and a hall, with a bud of
/// halls from the trunk's tenth moment.
fn striped() -> History {
    let mut history = grown(40, &[(0, 10, 20)]);
    for l in 0..history.limbs.len() {
        for (i, &m) in history.limbs[l].moments.clone().iter().enumerate() {
            let (kind, path) = if l == 0 && i % 2 == 0 {
                ("vco", "modules/vco1.toml")
            } else {
                ("hall", "modules/hall1.toml")
            };
            std::sync::Arc::make_mut(&mut history.moments[m])
                .tint
                .add(kind, path, None);
        }
    }
    history
}

#[test]
fn far_out_a_limb_blends_its_colours_and_close_in_each_is_its_own() {
    assert_eq!(mellow_window(MAX_SCALE), 1);
    assert_eq!(mellow_window(3.0), 1, "close in, each moment is its own");
    assert_eq!(mellow_window(0.5), 24, "the whole tree: three dots' worth");
    assert_eq!(mellow_window(1.0), 11, "names: easing off");
    assert_eq!(mellow_window(2.49), 1, "nothing left to blend by gestures");
    let mut last = usize::MAX;
    for step in 0..200 {
        let window = mellow_window(f64::from(step).mul_add(0.01, 0.75));
        assert!(window <= last, "zooming in never blends more");
        last = window;
    }
    assert_eq!(mellow_window(0.005), 2_400);
    assert_eq!(mellow_window(f64::NAN), 1);
    let history = striped();
    let (growth, gestures, line) = scene_parts(&history);
    let scene = Scene {
        history: &history,
        growth: &growth,
        gestures: &gestures,
        days: &[],
        line: &line,
        selected: None,
        now: 0,
    };
    let trunk = &history.limbs[0].moments;
    assert_eq!(mellowed(&scene, trunk[20], 1), None);
    // Four back: two voices and two halls, their mean.
    let mut four = crate::colour::Tint::default();
    for &m in &trunk[17..=20] {
        four = four.plus(&history.moments[m].tint);
    }
    assert_eq!(
        mellowed(&scene, trunk[20], 4),
        Some(crate::colour::blend(&four, 4.0, None))
    );
    // Near the tree's start the blend reaches back only to its first moment.
    let mut start = crate::colour::Tint::default();
    for &m in &trunk[..=2] {
        start = start.plus(&history.moments[m].tint);
    }
    assert_eq!(
        mellowed(&scene, trunk[2], 50),
        Some(crate::colour::blend(&start, 3.0, None))
    );
    // Close in, neighbours differ; far out, they are alike.
    let own = |m: usize| growth.look[m].expect("grown").colour;
    assert_ne!(own(trunk[20]), own(trunk[21]));
    assert_eq!(
        mellowed(&scene, trunk[20], 20),
        mellowed(&scene, trunk[22], 20)
    );
}

#[test]
fn a_buds_blend_reaches_back_down_the_limb_it_grew_from() {
    let history = striped();
    let (growth, gestures, line) = scene_parts(&history);
    let scene = Scene {
        history: &history,
        growth: &growth,
        gestures: &gestures,
        days: &[],
        line: &line,
        selected: None,
        now: 0,
    };
    let trunk = &history.limbs[0].moments;
    let bud = &history.limbs[1].moments;
    let from = history.limbs[1].from.expect("a bud");
    let at = history.place[from].expect("on the trunk").1;
    // The bud's second moment, four back: its first, the moment it grew
    // from and the two before that on the trunk.
    let mut four = bud[0..=1]
        .iter()
        .chain(&trunk[at - 1..=at])
        .fold(crate::colour::Tint::default(), |sum, &m| {
            sum.plus(&history.moments[m].tint)
        });
    assert_eq!(
        mellowed(&scene, bud[1], 4),
        Some(crate::colour::blend(&four, 4.0, None))
    );
    // Its running sums carry on from the trunk's.
    let running = |m: usize| growth.look[m].expect("grown").running;
    assert_eq!(running(bud[0]).moments, running(from).moments + 1);
    four = four.minus(&history.moments[bud[1]].tint);
    assert_eq!(
        running(bud[0]).tint.minus(&running(trunk[at - 2]).tint),
        four
    );
}

#[test]
fn a_new_days_words_find_room_beside_a_label_in_their_place() {
    let mut history = grown(30, &[(0, 10, 10)]);
    let bud_first = history.limbs[1].moments[0];
    for (i, moment) in history.moments.iter_mut().enumerate() {
        std::sync::Arc::make_mut(moment).time = if i == bud_first || history_after(i, bud_first) {
            1_760_486_400 + 86_400
        } else {
            1_760_486_400
        };
    }
    let zone = chrono::FixedOffset::east_opt(0).expect("utc");
    let days = days(&history, &zone);
    let day = days
        .iter()
        .position(|day| day.moment == bud_first)
        .expect("the bud's first moment starts a new day");
    let (growth, gestures, line) = scene_parts(&history);
    let from = history.limbs[1].from.expect("a bud");
    let view = View {
        centre: growth.at[from].expect("placed"),
        scale: 1.5,
    };
    let scene = Scene {
        history: &history,
        growth: &growth,
        gestures: &gestures,
        days: &days,
        line: &line,
        selected: Some(from),
        now: 0,
    };
    let picture = draw(&scene, &view, 120, 40);
    assert!(
        picture.text().contains(&days[day].words),
        "{}",
        picture.text()
    );
}

/// Whether moment `i` was made after `first` (the test trees make them in
/// order).
const fn history_after(i: usize, first: usize) -> bool {
    i > first
}

#[test]
fn a_bud_grown_from_a_moment_on_no_limb_blends_from_its_own_start() {
    let mut orphan = striped();
    let from = orphan.limbs[1].from.expect("a bud");
    // The moment it grew from is on no limb (its branch was deleted): off
    // the trunk, and every place after it on the trunk one closer.
    orphan.limbs[0].moments.retain(|&m| m != from);
    orphan.place[from] = None;
    for (p, &m) in orphan.limbs[0].moments.iter().enumerate() {
        orphan.place[m] = Some((0, p));
    }
    let (growth, gestures, line) = scene_parts(&orphan);
    assert_eq!(growth.look[from], None, "off every limb, it never grew");
    let scene = Scene {
        history: &orphan,
        growth: &growth,
        gestures: &gestures,
        days: &[],
        line: &line,
        selected: None,
        now: 0,
    };
    let bud = &orphan.limbs[1].moments;
    let running = |m: usize| growth.look[m].expect("grown").running;
    assert_eq!(running(bud[0]).moments, 1, "the bud's sums start with it");
    for p in [3_usize, 6] {
        for window in [p + 1, p + 2, 40] {
            assert_eq!(
                mellowed(&scene, bud[p], window),
                Some(running(bud[p]).colour_since(&Running::default())),
                "p {p} window {window}"
            );
        }
    }
}

#[test]
fn day_marks_take_no_place_another_glyph_or_word_would_have() {
    // A crown of buds, all grown the same day, so each says its day where
    // it starts, with marks on the trunk beside the forks.
    let buds: Vec<(usize, usize, usize)> = (8..30).step_by(2).map(|at| (0, at, 6)).collect();
    let mut history = grown(40, &buds);
    for moment in &mut history.moments {
        std::sync::Arc::make_mut(moment).time = 1_760_486_400;
    }
    for at in (7..30).step_by(2) {
        let m = history.limbs[0].moments[at];
        mark(&mut history, m, &format!("mark {at}"), false);
    }
    let zone = chrono::FixedOffset::east_opt(0).expect("utc");
    let days = days(&history, &zone);
    assert_eq!(days.len(), 1 + buds.len(), "the trunk and every bud");
    let (growth, gestures, line) = scene_parts(&history);
    for scale in [0.5, 1.0, 1.5, 2.4, 3.0, 8.0] {
        let view = View {
            centre: growth.at[history.limbs[0].moments[18]].expect("placed"),
            scale,
        };
        let scene = |days| Scene {
            history: &history,
            growth: &growth,
            gestures: &gestures,
            days,
            line: &line,
            selected: None,
            now: 0,
        };
        let without = draw(&scene(&[]), &view, 100, 30);
        let with = draw(&scene(&days), &view, 100, 30);
        // Every letter and glyph drawn without days is drawn with them.
        for (i, (a, b)) in without.cells.iter().zip(&with.cells).enumerate() {
            if a.text.is_some() {
                assert_eq!(a.text, b.text, "scale {scale} cell {i}");
            }
        }
        let shown = with.text();
        if Level::of(scale) >= Level::Near {
            assert!(shown.contains("Wed 15 Oct 2025"), "scale {scale}: {shown}");
        } else {
            // Far out a day says nothing, so it draws nothing.
            assert_eq!(shown, without.text(), "scale {scale}");
        }
    }
}

#[test]
fn a_day_hangs_its_words_only_beside_its_own_moments_glyph() {
    // Its own glyph, or the selected mark of itself.
    assert!(day_holds(Some(4), 4, None));
    assert!(day_holds(None, 4, Some(4)));
    // Another moment's glyph, even with the day's moment selected: its
    // words there would read as that moment's day.
    assert!(!day_holds(Some(9), 4, Some(4)));
    assert!(!day_holds(Some(9), 4, None));
    // A cell taken by a label, not a glyph, and the day not selected.
    assert!(!day_holds(None, 4, None));
    assert!(!day_holds(None, 4, Some(9)));
}
