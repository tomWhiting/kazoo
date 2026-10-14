use std::time::Instant;

use crossterm::event::{KeyEvent, KeyEventState, MouseEvent};

use super::*;
use crate::test_support::{batch, grown, mark};

fn app(history: History) -> App {
    let mut app = App {
        dir: PathBuf::from("/nonexistent"),
        history: History::default(),
        growth: Growth::default(),
        gestures: Vec::new(),
        days: Vec::new(),
        line: Vec::new(),
        lanes: Lanes::default(),
        shown: Shown::Tree,
        lane_top: 0,
        view: None,
        selected: None,
        help: false,
        help_top: 0,
        yank: None,
        said: String::new(),
        area: (80, 30),
        drag: None,
        quit: false,
        wants_reading: false,
        go: Go::Idle,
        follow: Follow::Off,
    };
    app.take(history);
    app.selected = app.history.here;
    app
}

fn press(app: &mut App, code: KeyCode) {
    app.key(KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: crossterm::event::KeyEventKind::Press,
        state: KeyEventState::NONE,
    });
}

fn mouse(app: &mut App, kind: MouseEventKind, column: u16, row: u16) {
    app.mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    });
}

#[test]
fn keys_pan_and_zoom_and_zero_fits_again() {
    let mut app = app(grown(200, &[(0, 50, 60)]));
    let fitted = app.view(80, 30);
    press(&mut app, KeyCode::Char('+'));
    let zoomed = app.view.expect("a view");
    assert!((zoomed.scale / fitted.scale - ZOOM).abs() < 1e-4);
    press(&mut app, KeyCode::Char('l'));
    let panned = app.view.expect("a view");
    assert!(panned.centre.0 > zoomed.centre.0);
    press(&mut app, KeyCode::Char('k'));
    assert!(app.view.expect("a view").centre.1 > panned.centre.1);
    press(&mut app, KeyCode::Char('0'));
    assert_eq!(app.view, Some(fitted));
}

#[test]
fn tab_walks_the_marks_and_the_walls_place() {
    let mut history = grown(100, &[]);
    mark(&mut history, 20, "one", false);
    mark(&mut history, 60, "two", true);
    let mut app = app(history);
    app.selected = Some(0);
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.selected, Some(20));
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.selected, Some(60));
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.selected, Some(99), "the wall's place");
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.selected, Some(20), "round again");
    press(&mut app, KeyCode::BackTab);
    assert_eq!(app.selected, Some(99));
}

#[test]
fn brackets_walk_back_through_where_a_moment_grew_from() {
    let mut app = app(grown(30, &[(0, 10, 5)]));
    let bud_start = app.history.limbs[1].moments[0];
    app.selected = Some(bud_start);
    press(&mut app, KeyCode::Char('['));
    assert_eq!(
        app.selected,
        Some(10),
        "back onto the trunk where it budded"
    );
    press(&mut app, KeyCode::Char(']'));
    assert_eq!(app.selected, Some(11), "on along the trunk");
    press(&mut app, KeyCode::Char('}'));
    assert_eq!(app.selected, Some(27));
    press(&mut app, KeyCode::Char('}'));
    assert_eq!(app.selected, Some(29), "held at the tip");
    press(&mut app, KeyCode::Char('{'));
    assert_eq!(app.selected, Some(13));
}

#[test]
fn a_click_selects_the_nearest_moment_and_a_drag_pans() {
    let mut app = app(grown(80, &[]));
    // Right in, moments sit ten dots apart: a click on the cell holding
    // moment 40 can only mean moment 40.
    let view = View {
        centre: app.growth.at[40].expect("placed"),
        scale: MAX_SCALE,
    };
    app.view = Some(view);
    let (x, y) = view.to_dots(app.growth.at[40].expect("placed"), 80, 30);
    let (col, row) = ((x / 2.0) as u16, (y / 4.0) as u16);
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), col, row);
    assert_eq!(app.selected, Some(40));
    mouse(
        &mut app,
        MouseEventKind::Drag(MouseButton::Left),
        col + 10,
        row,
    );
    let dragged = app.view.expect("a view");
    assert!(
        dragged.centre.0 < view.centre.0,
        "dragging right moves the world right"
    );
    mouse(
        &mut app,
        MouseEventKind::Up(MouseButton::Left),
        col + 10,
        row,
    );
    assert!(app.drag.is_none());
}

#[test]
fn the_wheel_zooms_about_the_pointer() {
    let mut app = app(grown(80, &[]));
    let view = View {
        centre: (0.0, 100.0),
        scale: 1.0,
    };
    app.view = Some(view);
    let under = view.to_world(10, 5, 80, 30);
    mouse(&mut app, MouseEventKind::ScrollUp, 10, 5);
    let zoomed = app.view.expect("a view");
    assert!(zoomed.scale > view.scale);
    let still = zoomed.to_world(10, 5, 80, 30);
    assert!(
        (still.0 - under.0).abs() < 0.5 && (still.1 - under.1).abs() < 0.5,
        "{under:?} {still:?}"
    );
}

#[test]
fn the_details_say_who_when_what_and_how_it_sounded() {
    let mut history = grown(20, &[]);
    mark(&mut history, 19, "the rock 100", true);
    let app = app(history);
    let details = app.details();
    let text: Vec<String> = details.iter().map(|(k, v)| format!("{k} {v}")).collect();
    let text = text.join("\n");
    assert!(text.contains("who Tom"), "{text}");
    assert!(text.contains("alive"), "{text}");
    assert!(text.contains("tempo 100.0 BPM"), "{text}");
    assert!(text.contains("not kept"), "{text}");
    assert!(text.contains("save the rock 100"), "{text}");
    assert!(text.contains("the wall is here"), "{text}");
    assert!(text.contains("limb trunk (20 of 20)"), "{text}");
}

#[test]
fn a_batchs_details_lead_with_its_note_and_list_its_changes() {
    let mut history = grown(20, &[]);
    batch(
        &mut history,
        19,
        "the brass breathes",
        &["added lfo3", "turned vcf1 res 0.4"],
    );
    let app = app(history);
    let details = app.details();
    assert_eq!(details[0].1, "the brass breathes");
    assert_eq!(
        details[1],
        ("batch".to_owned(), "2 changes, sent at once".to_owned())
    );
    assert_eq!(details[2], ("1".to_owned(), "added lfo3".to_owned()));
    assert_eq!(
        details[3],
        ("2".to_owned(), "turned vcf1 res 0.4".to_owned())
    );
    assert_eq!(details[4].0, "who");
}

#[test]
fn the_help_is_short_and_q_closes_it_before_quitting() {
    for (heading, keys) in HELP {
        assert!(heading.chars().count() <= 50);
        for (key, what) in *keys {
            assert!(
                format!("{key}: {what}").chars().count() <= 50,
                "{key}: {what}"
            );
        }
    }
    let mut app = app(grown(5, &[]));
    press(&mut app, KeyCode::Char('?'));
    assert!(app.help);
    press(&mut app, KeyCode::Char('q'));
    assert!(!app.help && !app.quit);
    press(&mut app, KeyCode::Char('q'));
    assert!(app.quit);
}

#[test]
fn taking_a_grown_tree_keeps_the_selection_by_commit() {
    let mut app = app(grown(30, &[]));
    app.selected = Some(12);
    let id = app.history.moments[12].id.clone();
    let mut more = grown(30, &[]);
    crate::test_support::lengthen(&mut more, 0, 10);
    app.take(more);
    assert_eq!(
        app.selected.map(|m| app.history.moments[m].id.clone()),
        Some(id)
    );
}

#[test]
fn v_turns_to_the_lanes_where_up_and_down_walk_the_rows() {
    let mut app = app(grown(10, &[(0, 3, 4)]));
    press(&mut app, KeyCode::Char('v'));
    assert_eq!(app.shown, Shown::Lanes);
    // Newest first: the wall's place (the bud's tip, 13) is the top row.
    assert_eq!(app.selected_row(), Some(0));
    press(&mut app, KeyCode::Down);
    assert_eq!(app.selected, Some(app.lanes.rows[1].moment));
    press(&mut app, KeyCode::End);
    assert_eq!(app.selected, Some(0), "the oldest moment is the last row");
    assert_eq!(
        app.lanes_from(5),
        app.lanes.rows.len() - 5,
        "scrolled to it"
    );
    press(&mut app, KeyCode::Home);
    assert_eq!(app.lanes_from(5), 0);
    press(&mut app, KeyCode::Char('v'));
    assert_eq!(app.shown, Shown::Tree);
}

#[test]
fn a_click_off_the_lanes_selects_nothing() {
    let mut app = app(grown(100, &[]));
    app.shown = Shown::Lanes;
    app.area = (80, 30);
    let before = app.selected;
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), 10, 30);
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), 85, 3);
    mouse(&mut app, MouseEventKind::ScrollDown, 90, 3);
    assert_eq!(app.selected, before);
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), 10, 3);
    assert_ne!(app.selected, before);
}

#[test]
fn y_copies_the_whole_commit() {
    let mut app = app(grown(10, &[]));
    press(&mut app, KeyCode::Char('y'));
    let id = app.history.moments[app.selected.expect("selected")]
        .id
        .to_string();
    assert_eq!(app.yank.as_deref(), Some(id.as_str()));
    assert_eq!(id.len(), 40);
}

#[test]
fn g_puts_the_wall_at_the_selected_moment_at_once() {
    let mut app = app(grown(10, &[]));
    app.follow = Follow::Off;
    // Where the wall is already: nothing to do.
    press(&mut app, KeyCode::Char('g'));
    assert_eq!(app.go, Go::Idle);
    assert_eq!(app.said, "the wall is at this moment already");
    press(&mut app, KeyCode::Char('['));
    let there = app.history.moments[app.selected.expect("selected")]
        .id
        .clone();
    press(&mut app, KeyCode::Char('g'));
    assert_eq!(app.go, Go::Send(there.clone()));
    // The loop takes it, and nothing more is due until the wall answers.
    let now = Instant::now();
    assert_eq!(app.go_due(now), Some(there));
    assert_eq!(app.go, Go::Sent);
    assert_eq!(app.go_due(now + REST * 4), None);
}

#[test]
fn following_goes_where_the_selection_rests_once() {
    let mut app = app(grown(40, &[]));
    // Off from the start: f turns it on.
    press(&mut app, KeyCode::Char('f'));
    assert_eq!(app.follow, Follow::On(None));
    let start = Instant::now();
    // Nothing moved by the user: nothing to follow.
    assert_eq!(app.go_due(start + REST * 2), None);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    let rest = app.history.moments[app.selected.expect("selected")]
        .id
        .clone();
    assert!(app.resting());
    // Still running through: not yet.
    assert_eq!(app.go_due(Instant::now()), None);
    let later = Instant::now() + REST;
    assert_eq!(app.go_due(later), Some(rest));
    assert_eq!(app.go, Go::Sent);
    assert!(!app.resting());
    // Answered: nothing more until the user moves again.
    app.go = Go::Idle;
    assert_eq!(app.go_due(later + REST * 4), None);
    // Back onto the wall's own place: no go.
    press(&mut app, KeyCode::Char('.'));
    assert_eq!(app.go_due(Instant::now() + REST * 2), None);
    // f stops following.
    press(&mut app, KeyCode::Char('f'));
    assert_eq!(app.follow, Follow::Off);
    press(&mut app, KeyCode::Down);
    assert_eq!(app.go_due(Instant::now() + REST * 2), None);
}

#[test]
fn right_steps_into_a_branch_and_left_back_out() {
    // A bud grows from trunk moment 20, another from moment 30.
    let mut app = app(grown(60, &[(0, 20, 5), (0, 30, 5)]));
    app.follow = Follow::Off;
    let trunk_at = |app: &App, place: usize| app.history.limbs[0].moments[place];
    app.selected = Some(trunk_at(&app, 10));
    press(&mut app, KeyCode::Right);
    let first_bud = app.history.limbs[1].moments[0];
    assert_eq!(app.selected, Some(first_bud));
    // Up walks along the bud.
    press(&mut app, KeyCode::Up);
    assert_eq!(app.selected, Some(app.history.limbs[1].moments[1]));
    // Left goes back to where it grew from.
    press(&mut app, KeyCode::Left);
    assert_eq!(app.selected, Some(trunk_at(&app, 20)));
    // Past the first bud's moment, Right finds the next one ahead.
    app.selected = Some(trunk_at(&app, 25));
    press(&mut app, KeyCode::Right);
    assert_eq!(app.selected, Some(app.history.limbs[2].moments[0]));
    // Beyond the last bud: said, and the selection stays.
    app.selected = Some(trunk_at(&app, 50));
    press(&mut app, KeyCode::Right);
    assert_eq!(app.selected, Some(trunk_at(&app, 50)));
    assert_eq!(app.said, "no branch grows from here on");
    // On the trunk, Left has nowhere to go.
    press(&mut app, KeyCode::Left);
    assert_eq!(app.said, "this is the trunk: nothing to step back out to");
}

#[test]
fn right_on_a_buds_first_moment_moves_to_its_sister() {
    let mut app = app(grown(40, &[(0, 20, 4), (0, 20, 4)]));
    app.follow = Follow::Off;
    app.selected = Some(app.history.limbs[0].moments[20]);
    press(&mut app, KeyCode::Right);
    assert_eq!(app.selected, Some(app.history.limbs[1].moments[0]));
    press(&mut app, KeyCode::Right);
    assert_eq!(app.selected, Some(app.history.limbs[2].moments[0]));
}

#[test]
fn g_with_nothing_selected_says_so() {
    let mut app = app(grown(3, &[]));
    app.selected = None;
    press(&mut app, KeyCode::Char('g'));
    assert_eq!(app.go, Go::Idle);
    assert_eq!(app.said, "select a moment to put the wall at first");
}

#[test]
fn a_short_id_is_twelve_characters_or_all_of_a_shorter_one() {
    assert_eq!(short("0123456789abcdef"), "0123456789ab");
    assert_eq!(short("abc"), "abc");
}

#[test]
fn the_arrows_only_look_and_say_how_far_and_enter_goes() {
    // Tom, 15 Oct: "the arrow keys... probably shouldn't move you straight
    // away... arrow keys plus enter... you get a little bit of information".
    let mut app = app(grown(40, &[(0, 20, 6)]));
    assert_eq!(app.follow, Follow::Off, "nothing follows from the start");
    let start = Instant::now();
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    assert!(!app.resting());
    assert_eq!(app.go_due(start + REST * 4), None, "looking never goes");
    assert_eq!(app.go, Go::Idle);
    assert_eq!(
        wall_line(&app).as_deref(),
        Some("2 changes back from the wall · Enter puts the wall here")
    );
    press(&mut app, KeyCode::Up);
    assert_eq!(
        wall_line(&app).as_deref(),
        Some("1 change back from the wall · Enter puts the wall here")
    );
    // Said on its own line, never in what the footer says, so a read of
    // the tree or a move of the wall never leaves it stale.
    assert!(app.said.is_empty(), "{}", app.said);
    let here = app.history.here;
    app.history.here = app.selected;
    assert_eq!(wall_line(&app), None, "the wall moved here: ◉ says it");
    app.history.here = here;
    press(&mut app, KeyCode::Up);
    assert_eq!(wall_line(&app), None);
    // The wall is at the bud's tip (6 moments from trunk moment 20): a
    // trunk moment further on is back to the fork, then on.
    app.selected = Some(app.history.limbs[0].moments[29]);
    press(&mut app, KeyCode::Up);
    assert_eq!(
        wall_line(&app).as_deref(),
        Some(
            "16 changes from the wall: 6 back to where they part, 10 on · Enter puts the wall here"
        )
    );
    // Into the bud from the fork: its first moment, on the wall's line.
    app.selected = Some(app.history.limbs[0].moments[20]);
    press(&mut app, KeyCode::Right);
    let bud_first = app.history.limbs[1].moments[0];
    assert_eq!(app.selected, Some(bud_first));
    assert_eq!(
        wall_line(&app).as_deref(),
        Some("5 changes back from the wall · Enter puts the wall here")
    );
    // Enter goes there; c only centres.
    press(&mut app, KeyCode::Char('c'));
    assert_eq!(app.go, Go::Idle);
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.go, Go::Send(app.history.moments[bud_first].id.clone()));
}

#[test]
fn the_path_between_two_moments_goes_back_to_where_they_part_then_on() {
    let history = grown(30, &[(0, 10, 4), (0, 20, 3)]);
    let trunk = |p: usize| history.limbs[0].moments[p];
    let first = history.limbs[1].moments[3];
    let second = history.limbs[2].moments[2];
    assert_eq!(history.path_between(trunk(5), trunk(5)), Some((0, 0)));
    assert_eq!(history.path_between(trunk(5), trunk(9)), Some((0, 4)));
    assert_eq!(history.path_between(trunk(9), trunk(5)), Some((4, 0)));
    // Bud to bud: down the first to trunk 10, then up through 20.
    assert_eq!(history.path_between(first, second), Some((4, 13)));
    // Along one bud, both ways.
    let bud = |p: usize| history.limbs[1].moments[p];
    assert_eq!(history.path_between(bud(0), bud(3)), Some((0, 3)));
    assert_eq!(history.path_between(bud(3), bud(1)), Some((2, 0)));
    // From a bud down to the trunk below where it grew, and on past it.
    assert_eq!(history.path_between(first, trunk(4)), Some((10, 0)));
    assert_eq!(history.path_between(first, trunk(29)), Some((4, 19)));
    // A moment that is not in the tree has no path.
    assert_eq!(history.path_between(trunk(5), history.moments.len()), None);
}

/// The details' line saying how far the selected moment is from the wall.
fn wall_line(app: &App) -> Option<String> {
    app.details()
        .into_iter()
        .find(|(key, _)| key == "wall")
        .map(|(_, words)| words)
}

#[test]
fn the_path_between_two_moments_is_walked_by_limbs_not_moments() {
    // Every moment's parent forgotten: only the limbs and where each grew
    // from are left, and they are all the walk needs.
    let mut history = grown(30, &[(0, 10, 4), (0, 20, 3)]);
    for moment in &mut history.moments {
        std::sync::Arc::make_mut(moment).parent = None;
    }
    let first = history.limbs[1].moments[3];
    let second = history.limbs[2].moments[2];
    assert_eq!(history.path_between(first, second), Some((4, 13)));
    assert_eq!(
        history.path_between(history.limbs[0].moments[2], history.limbs[0].moments[7]),
        Some((0, 5))
    );
}
