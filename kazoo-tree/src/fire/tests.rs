use std::path::PathBuf;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::field::{MAX_H, MAX_W, Shape};
use super::flicks::MAX_FLICKS;
use super::place::shape_of;
use super::*;
use crate::app::{App, Grown, PANEL, Shown};
use crate::growth::Growth;
use crate::history::History;
use crate::render::{Level, Scene, View};
use crate::test_support::grown;

fn assert_sound(field: &Field) {
    let shape = field.shape();
    assert!(shape.w <= MAX_W && shape.h <= MAX_H);
    for &v in field.seen() {
        assert!(v.is_finite(), "a dot is not a number: {v}");
        assert!(
            (0.0..=1.0).contains(&v),
            "a dot's heat is out of bounds: {v}"
        );
    }
    assert!(field.spark_count() <= MAX_FLICKS);
    for (x, y) in field.spark_places() {
        assert!(x.is_finite() && y.is_finite());
        assert!(
            x >= 0.0 && x < shape.w as f32,
            "a spark left the field: {x}"
        );
        assert!(
            y >= 0.0 && y < shape.h as f32,
            "a spark left the field: {y}"
        );
    }
}

#[test]
fn the_field_stays_bounded_and_finite_whatever_the_steps() {
    let mut field = Field::new(shape_of(Level::Every, 4.0, 200, 60));
    let steps = [
        1.0 / 120.0,
        1.0 / 60.0,
        0.0,
        1.0e-7,
        0.05,
        3.0,
        -1.0,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
    ];
    let mut hottest = 0.0f32;
    for i in 0..1_000 {
        field.step(steps[i % steps.len()]);
        assert_sound(&field);
        hottest = field.seen().iter().fold(hottest, |a, &b| a.max(b));
    }
    // It burns: the control that the bounds above were not met by a cold
    // field.
    assert!(hottest > 0.6, "the fire never caught: {hottest}");
    assert!(field.frame() >= 1_000);
}

#[test]
fn a_reshaped_field_stays_bounded_at_every_level_and_beyond() {
    let mut field = Field::new(shape_of(Level::Far, 1.0, 80, 24));
    let shapes = [
        shape_of(Level::Near, 3.0, 80, 24),
        shape_of(Level::Far, 1.0, 80, 24),
        shape_of(Level::Close, 10.0, 200, 60),
        shape_of(Level::Every, 10.0, 200, 60),
        shape_of(Level::Every, 10.0, 3, 2),
        Shape {
            w: usize::MAX,
            h: 0,
            base: usize::MAX,
            reach: f32::NAN,
            thick: f32::NEG_INFINITY,
            tall: f32::INFINITY,
            detail: u8::MAX,
            sparks: f32::INFINITY,
            wisps: f32::NAN,
        },
    ];
    for _ in 0..60 {
        field.step(1.0 / 60.0);
    }
    for shape in shapes {
        let hot = field.seen().iter().fold(0.0f32, |a, &b| a.max(b));
        field.reshape(shape);
        assert_sound(&field);
        if hot > 0.5 {
            // What was burning is carried into the new size, not put out.
            let still = field.seen().iter().fold(0.0f32, |a, &b| a.max(b));
            assert!(
                still > 0.3,
                "the fire went out on reshaping: {hot} -> {still}"
            );
        }
        for _ in 0..120 {
            field.step(1.0 / 60.0);
            assert_sound(&field);
        }
    }
}

#[test]
fn the_fire_grows_with_the_zoom_and_far_out_is_a_small_ember() {
    let far = shape_of(Level::Far, 1.0, 120, 40);
    let near = shape_of(Level::Near, 2.0, 120, 40);
    let close = shape_of(Level::Close, 6.0, 120, 40);
    let every = shape_of(Level::Every, 10.0, 120, 40);
    // Far out: a few cells, never over the tree.
    assert!(far.w <= 20 && far.h <= 32, "{far:?}");
    assert!(far.w <= near.w && near.w <= close.w && close.w <= every.w);
    assert!(far.h <= near.h && near.h <= close.h && close.h <= every.h);
    // Never taller than most of the picture.
    assert!(every.h <= 40 * 4 * 9 / 10);
    // Close in, the flames follow the limb's width.
    let (thin, thick) = (
        shape_of(Level::Close, 1.0, 120, 40),
        shape_of(Level::Close, 9.0, 120, 40),
    );
    assert!(thick.reach > thin.reach && thick.thick > thin.thick);
    // The finer detail comes as the view comes in.
    assert!(far.detail < near.detail && near.detail < close.detail && close.detail < every.detail);
}

fn tree() -> (History, Growth) {
    let history = grown(200, &[(0, 50, 60), (0, 120, 40)]);
    let growth = Growth::grow(&history);
    (history, growth)
}

#[test]
fn the_fire_stands_on_the_walls_place_at_every_zoom() {
    let (history, growth) = tree();
    stands_on_here(&history, &growth, false);
}

#[test]
fn the_fire_stands_on_the_walls_place_on_a_trunk() {
    let history = grown(200, &[]);
    let growth = Growth::grow(&history);
    stands_on_here(&history, &growth, true);
}

fn stands_on_here(history: &History, growth: &Growth, trunk: bool) {
    let here = history.here.expect("the wall has a place");
    let at = growth.at[here].expect("its place is grown");
    let width = growth.look[here].expect("its place is drawn").width;
    let limb = history.place[here].expect("on a limb").0;
    assert_eq!(history.limbs[limb].from.is_none(), trunk);
    let scene = Scene {
        history,
        growth,
        gestures: &[],
        days: &[],
        line: &[],
        selected: None,
        now: 0,
    };
    let (cols, rows) = (100, 40);
    let mut widths = Vec::new();
    for scale in [0.3, 1.5, 4.0, 10.0] {
        for off in [(0.0, 0.0), (7.5, -3.25), (-20.0, 11.0)] {
            let view = View {
                centre: (at.0 + off.0 / scale, at.1 + off.1 / scale),
                scale,
            };
            let place = Placement::of(&scene, &view, cols, rows, here).expect("placed");
            let (x, y) = view.to_dots(at, cols, rows);
            let cell = ((x / 2.0).floor() as i32, (y / 4.0).floor() as i32);
            assert_eq!(
                place.anchor,
                (cell.0 * 2 + 1, cell.1 * 4 + 2),
                "at scale {scale}"
            );
            // The field stands on the anchor: centred over it, its base
            // row on it, room under it for the branch.
            let w = i32::try_from(place.shape.w).expect("a small field");
            let base = i32::try_from(place.shape.base).expect("a small field");
            assert_eq!(place.origin.0 + w / 2, place.anchor.0);
            assert_eq!(place.origin.1 + base, place.anchor.1);
            assert!(place.shape.base + 3 <= place.shape.h);
            // It burns along the branch through the moment, from its dot.
            let middle = ((place.shape.w / 2) as f32, place.shape.base as f32);
            assert_eq!(place.fuel.moment(), middle);
            assert!(place.fuel.points().len() > 1, "only the moment burns");
            // As wide as the limb is drawn under it.
            let limb = crate::render::limb_dots(width, trunk, scale) as f32;
            assert_eq!(place.shape, shape_of(Level::of(scale), limb, cols, rows));
            assert!(place.visible(cols, rows));
        }
        let view = View { centre: at, scale };
        widths.push(
            Placement::of(&scene, &view, cols, rows, here)
                .expect("placed")
                .shape
                .w,
        );
    }
    assert!(
        widths.windows(2).all(|pair| pair[0] <= pair[1]),
        "{widths:?}"
    );
    // Far off screen, it is not in view.
    let away = View {
        centre: (at.0 + 1.0e4, at.1),
        scale: 1.5,
    };
    let place = Placement::of(&scene, &away, cols, rows, here).expect("placed");
    assert!(!place.visible(cols, rows));
}

fn app() -> App {
    let (history, _) = tree();
    let mut app = App::showing(PathBuf::from("/nonexistent"), Grown::of(history));
    app.said.clear();
    app
}

fn frame(app: &mut App, area: Rect) -> Buffer {
    let mut buf = Buffer::empty(area);
    crate::frame_of(app, area, &mut buf);
    buf
}

/// Light `app`'s fire and let it burn a second.
fn burning(app: &mut App, area: Rect) {
    frame(app, area);
    for _ in 0..60 {
        app.fire.burn(1.0 / 60.0);
    }
}

#[test]
fn the_fire_is_drawn_on_the_tree_and_never_on_the_panel_or_footer() {
    let area = Rect::new(0, 0, 140, 40);
    let mut cold = app();
    cold.fire.on = false;
    let plain = frame(&mut cold, area);
    let mut lit = app();
    burning(&mut lit, area);
    let ablaze = frame(&mut lit, area);
    assert!(lit.fire.live(), "the fire is in view");
    let tree_width = area.width - PANEL;
    let (mut changed, mut letters, mut reversed) = (0, 0, 0);
    for y in 0..area.height {
        for x in 0..area.width {
            let (was, now) = (&plain[(x, y)], &ablaze[(x, y)]);
            if x >= tree_width || y == area.height - 1 {
                assert_eq!(was, now, "the fire drew on the panel or footer at {x}, {y}");
                continue;
            }
            if was != now {
                changed += 1;
            }
            if was.modifier.contains(ratatui::style::Modifier::REVERSED) {
                // The selected moment's own cell is left whole.
                assert_eq!(was, now);
                reversed += 1;
            }
            let braille = was
                .symbol()
                .chars()
                .all(|c| c == ' ' || ('\u{2800}'..='\u{28ff}').contains(&c));
            if !braille {
                // A glyph or a label's letter keeps its letter and colour.
                assert_eq!(
                    (was.symbol(), was.fg),
                    (now.symbol(), now.fg),
                    "at {x}, {y}"
                );
                letters += usize::from(was.bg != now.bg);
            }
        }
    }
    assert!(changed > 0, "the fire drew nothing");
    assert!(
        reversed > 0,
        "the control: the selected cell was in the picture"
    );
    assert!(
        letters > 0,
        "the control: the fire glowed behind some letter"
    );
}

#[test]
fn a_fire_put_out_draws_nothing_extra() {
    let area = Rect::new(0, 0, 120, 40);
    let mut cold = app();
    cold.fire.on = false;
    let plain = frame(&mut cold, area);
    let mut lit = app();
    burning(&mut lit, area);
    assert_ne!(frame(&mut lit, area), plain, "the lit fire draws");
    lit.key(crossterm::event::KeyEvent::from(
        crossterm::event::KeyCode::Char('b'),
    ));
    assert!(!lit.fire.on);
    assert!(!lit.fire.live());
    lit.said.clear();
    assert_eq!(frame(&mut lit, area), plain);
    lit.key(crossterm::event::KeyEvent::from(
        crossterm::event::KeyCode::Char('b'),
    ));
    assert!(lit.fire.on, "b lights it again");
}

#[test]
fn burning_never_changes_the_tree_the_view_or_the_selection() {
    let area = Rect::new(0, 0, 120, 40);
    let mut app = app();
    frame(&mut app, area);
    let history = app.history.clone();
    let growth = app.growth.clone();
    let (view, selected, line) = (app.view, app.selected, app.line.clone());
    for scale in [0.3, 1.5, 4.0, 10.0] {
        app.view = view.map(|v| View { scale, ..v });
        let before = app.view;
        for _ in 0..40 {
            app.fire.burn(1.0 / 120.0);
            app.fire.fire_only();
            frame(&mut app, area);
        }
        assert_eq!(app.view, before);
    }
    assert_eq!(app.history, history);
    assert_eq!(app.growth, growth);
    assert_eq!(app.selected, selected);
    assert_eq!(app.line, line);
}

#[test]
fn a_kept_picture_draws_the_same_tree_as_a_fresh_one() {
    let area = Rect::new(0, 0, 120, 40);
    let mut app = app();
    burning(&mut app, area);
    let fresh = frame(&mut app, area);
    app.fire.fire_only();
    assert_eq!(frame(&mut app, area), fresh);
    // A kept picture is never shown for a different view.
    app.fire.fire_only();
    app.view = app.view.map(|v| View {
        scale: v.scale * 2.0,
        ..v
    });
    let zoomed = frame(&mut app, area);
    assert_eq!(frame(&mut app, area), zoomed);
}

#[test]
fn the_lanes_burn_no_frames() {
    let area = Rect::new(0, 0, 120, 40);
    let mut app = app();
    frame(&mut app, area);
    assert!(app.fire.live());
    app.shown = Shown::Lanes;
    frame(&mut app, area);
    assert!(!app.fire.live());
}

#[test]
fn far_out_the_ember_keeps_to_the_walls_place() {
    let area = Rect::new(0, 0, 120, 40);
    let mut cold = app();
    cold.fire.on = false;
    let mut lit = app();
    frame(&mut cold, area);
    frame(&mut lit, area);
    let view = cold.view.map(|v| View { scale: 0.3, ..v });
    (cold.view, lit.view) = (view, view);
    let plain = frame(&mut cold, area);
    burning(&mut lit, area);
    let ablaze = frame(&mut lit, area);
    let place = lit.fire.placed().expect("the ember is in view");
    assert_eq!(Level::of(0.3), Level::Far);
    let (col, row) = (place.anchor.0 / 2, place.anchor.1 / 4);
    let mut changed = 0;
    for y in 0..area.height {
        for x in 0..area.width {
            if plain[(x, y)] != ablaze[(x, y)] {
                changed += 1;
                let (dx, dy) = (i32::from(x) - col, i32::from(y) - row);
                assert!(
                    dx.abs() <= 6 && (-9..=3).contains(&dy),
                    "the ember spread to {dx}, {dy}"
                );
            }
        }
    }
    assert!(changed > 0, "the ember drew nothing");
}

#[test]
fn a_new_tree_or_selection_is_never_drawn_from_a_kept_picture() {
    let area = Rect::new(0, 0, 120, 40);
    let mut app = app();
    burning(&mut app, area);
    frame(&mut app, area);
    // A tree read again, with more grown: the kept picture is forgotten.
    app.show(Grown::of(grown(
        260,
        &[(0, 50, 60), (0, 120, 40), (0, 200, 30)],
    )));
    app.fire.fire_only();
    let after_show = frame(&mut app, area);
    app.fire.forget();
    assert_eq!(frame(&mut app, area), after_show, "drawn afresh");
    // Another moment selected.
    app.selected = Some(3);
    app.fire.fire_only();
    let moved = frame(&mut app, area);
    app.fire.forget();
    assert_eq!(frame(&mut app, area), moved, "drawn afresh");
}

#[test]
fn frames_keep_the_beat_and_never_come_back_to_back() {
    let start = std::time::Instant::now();
    let period = blaze::period();
    // On time: the next is a period on.
    assert_eq!(blaze::next_frame(start, start), start + period);
    // Late by less than a period: still on the beat.
    assert_eq!(blaze::next_frame(start, start + period / 2), start + period);
    // After a gap: a period from now, not at once.
    let gap = start + period * 10;
    assert_eq!(blaze::next_frame(start, gap), gap + period);
}
