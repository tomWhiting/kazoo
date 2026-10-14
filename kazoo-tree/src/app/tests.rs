use crossterm::event::{KeyEvent, KeyEventState, MouseEvent};

use super::*;
use crate::test_support::{grown, mark};

fn app(history: History) -> App {
    let mut app = App {
        dir: PathBuf::from("/nonexistent"),
        history: History::default(),
        growth: Growth::default(),
        gestures: Vec::new(),
        days: Vec::new(),
        line: Vec::new(),
        rows: Vec::new(),
        shown: Shown::Tree,
        lane_top: 0,
        view: None,
        selected: None,
        help: false,
        said: String::new(),
        area: (80, 30),
        drag: None,
        quit: false,
        wants_reading: false,
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
    press(&mut app, KeyCode::Right);
    let panned = app.view.expect("a view");
    assert!(panned.centre.0 > zoomed.centre.0);
    press(&mut app, KeyCode::Up);
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
    assert_eq!(app.selected, Some(app.rows[1].moment));
    press(&mut app, KeyCode::End);
    assert_eq!(app.selected, Some(0), "the oldest moment is the last row");
    assert_eq!(app.lanes_from(5), app.rows.len() - 5, "scrolled to it");
    press(&mut app, KeyCode::Home);
    assert_eq!(app.lanes_from(5), 0);
    press(&mut app, KeyCode::Char('v'));
    assert_eq!(app.shown, Shown::Tree);
}
