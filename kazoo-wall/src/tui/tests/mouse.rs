//! The mouse in the rack view, and the exact requests it produces.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use kazoo_wall::daemon::wall::{FLOOD_BURST, FLOOD_REFILL_PER_SECOND};
use kazoo_wall::protocol::{Place, Request};

use super::super::app::{
    App, DRAG_GLIDE, DRAG_INTERVAL, EDGE_COLUMNS, EDGE_STEP, Focus, Mode, Tone, View, dragged_value,
};
use super::super::knob::{Step, Travel};
use super::super::rack::geometry::{Density, Hit, Point};
use super::super::worker::{Job, Reply};
use super::render::render;
use super::{connected, module, press, snapshot};

/// A console in the rack view, drawn once at 160 × 60 so the mouse knows
/// where everything is.
fn rack_app() -> App {
    let mut app = connected();
    press(&mut app, KeyCode::Char('v'));
    assert_eq!(app.view(), View::Rack);
    render(&mut app, 160, 60);
    app
}

fn calls(app: &mut App) -> Vec<Request> {
    app.take_jobs()
        .into_iter()
        .map(|job| match job {
            Job::Call(request) => request,
            Job::Launch => panic!("the mouse started the wall"),
        })
        .collect()
}

fn mouse(app: &mut App, kind: MouseEventKind, cell: (u16, u16), shift: bool, now: Instant) {
    let modifiers = if shift {
        KeyModifiers::SHIFT
    } else {
        KeyModifiers::NONE
    };
    app.handle_mouse(
        MouseEvent {
            kind,
            column: cell.0,
            row: cell.1,
            modifiers,
        },
        now,
    );
}

/// The screen cell of sheet cell `point`.
fn screen(app: &App, point: Point) -> (u16, u16) {
    let area = app.rack.area;
    let x = u16::try_from(point.x - app.rack.pan.x).expect("in view");
    let y = u16::try_from(point.y - app.rack.pan.y).expect("in view");
    assert!(
        x < area.width && y < area.height,
        "{point:?} is out of view"
    );
    (area.x + x, area.y + y)
}

/// The middle of a knob's dial.
fn knob_at(app: &App, module: usize, knob: usize) -> (u16, u16) {
    let faceplate = app.rack.layout.plate(module).expect("a faceplate");
    let place = faceplate
        .knobs
        .iter()
        .find(|place| place.index == knob)
        .expect("the knob");
    screen(
        app,
        Point {
            x: place.at.x + 3,
            y: place.at.y + u32::from(app.rack.layout.density.scale().knob_h / 4),
        },
    )
}

/// A jack's socket.
fn jack_at(app: &App, module: usize, name: &str, output: bool) -> (u16, u16) {
    let plate = app.rack.layout.plate(module).expect("a faceplate");
    let jack = plate
        .jacks
        .iter()
        .find(|jack| jack.name == name && jack.output == output)
        .expect("the jack");
    screen(
        app,
        Point {
            x: jack.at.x + app.rack.layout.density.scale().socket_dx,
            y: jack.at.y,
        },
    )
}

fn cutoff_travel() -> Travel {
    let catalogue = kazoo_wall::daemon::wall::catalogue();
    let info = catalogue
        .kinds
        .iter()
        .find(|kind| kind.kind == "vcf")
        .and_then(|kind| kind.knobs.iter().find(|knob| knob.name == "cutoff"));
    let view = &snapshot().modules[2].knobs[0];
    Travel::of(view, info)
}

const LEFT: MouseButton = MouseButton::Left;

#[test]
fn dragging_a_knob_turns_it_with_a_short_glide_at_most_ten_times_a_second() {
    let mut app = rack_app();
    let travel = cutoff_travel();
    let cell = knob_at(&app, 2, 0);
    let t0 = Instant::now();
    mouse(&mut app, MouseEventKind::Down(LEFT), cell, false, t0);
    assert_eq!(app.selected_index(), Some(2));
    assert_eq!(app.knob_index(), 0);
    // The glide is heading to 800 Hz: the drag starts from there.
    let anchor = travel.position(800.0);

    let at = t0 + Duration::from_millis(10);
    mouse(
        &mut app,
        MouseEventKind::Drag(LEFT),
        (cell.0, cell.1 - 4),
        false,
        at,
    );
    app.tick(at);
    let first = dragged_value(&travel, anchor, cell.1, cell.1 - 4, false);
    assert_eq!(
        calls(&mut app),
        vec![Request::Turn {
            module: "vcf1".to_string(),
            knob: "cutoff".to_string(),
            value: first,
            glide_beats: Some(DRAG_GLIDE),
        }]
    );
    // Within the interval, nothing more goes...
    let at = t0 + Duration::from_millis(50);
    mouse(
        &mut app,
        MouseEventKind::Drag(LEFT),
        (cell.0, cell.1 - 8),
        false,
        at,
    );
    app.tick(at);
    assert!(app.take_jobs().is_empty());
    assert_eq!(
        app.ahead("vcf1", "cutoff"),
        Some(dragged_value(&travel, anchor, cell.1, cell.1 - 8, false)),
        "the dial shows it at once"
    );
    // ...and letting go sends the last value straight away.
    let at = t0 + Duration::from_millis(60);
    mouse(
        &mut app,
        MouseEventKind::Up(LEFT),
        (cell.0, cell.1 - 8),
        false,
        at,
    );
    let last = dragged_value(&travel, anchor, cell.1, cell.1 - 8, false);
    assert!(last > first);
    assert_eq!(
        calls(&mut app),
        vec![Request::Turn {
            module: "vcf1".to_string(),
            knob: "cutoff".to_string(),
            value: last,
            glide_beats: Some(DRAG_GLIDE),
        }]
    );
    assert!(app.rack.drag.is_none());
}

#[test]
fn a_long_drag_is_coalesced_and_the_final_value_always_lands() {
    let mut app = rack_app();
    let travel = cutoff_travel();
    let cell = knob_at(&app, 2, 0);
    let t0 = Instant::now();
    mouse(&mut app, MouseEventKind::Down(LEFT), cell, false, t0);
    let mut sent: Vec<(Duration, f64)> = Vec::new();
    let mut row = cell.1;
    // Twenty seconds of wiggling, an event every 20 ms.
    for step in 1..=1000_u32 {
        let at = t0 + Duration::from_millis(u64::from(step) * 20);
        row = if step % 2 == 0 {
            cell.1 - 1
        } else {
            cell.1 - 2
        };
        mouse(
            &mut app,
            MouseEventKind::Drag(LEFT),
            (cell.0, row),
            false,
            at,
        );
        app.tick(at);
        for request in calls(&mut app) {
            let Request::Turn { value, .. } = request else {
                panic!("only turns: {request:?}");
            };
            sent.push((at - t0, value));
        }
    }
    // Never more than ten a second...
    for window in sent.windows(2) {
        assert!(
            window[1].0.checked_sub(window[0].0) >= Some(DRAG_INTERVAL),
            "{window:?}"
        );
    }
    // ...and never more than the wall's flood guard allows.
    let seconds = 20.0;
    let allowed = FLOOD_REFILL_PER_SECOND.mul_add(seconds, FLOOD_BURST) as usize;
    assert!(sent.len() <= allowed, "{} turns", sent.len());
    // Letting go sends the last value, as soon as the guard allows.
    let end = t0 + Duration::from_millis(20_020);
    mouse(
        &mut app,
        MouseEventKind::Up(LEFT),
        (cell.0, row),
        false,
        end,
    );
    let mut last = calls(&mut app);
    if last.is_empty() {
        app.tick(end + Duration::from_secs(1));
        last = calls(&mut app);
    }
    let final_value = dragged_value(&travel, travel.position(800.0), cell.1, row, false);
    assert!(
        matches!(&last[..], [Request::Turn { value, .. }] if (*value - final_value).abs() < 1e-9)
            || sent
                .last()
                .is_some_and(|(_, value)| (*value - final_value).abs() < 1e-9),
        "the final value lands: {last:?}"
    );
}

#[test]
fn shift_drags_finely_and_changing_mid_drag_never_jumps() {
    let mut app = rack_app();
    let travel = cutoff_travel();
    let cell = knob_at(&app, 2, 0);
    let t0 = Instant::now();
    mouse(&mut app, MouseEventKind::Down(LEFT), cell, false, t0);
    mouse(
        &mut app,
        MouseEventKind::Drag(LEFT),
        (cell.0, cell.1 - 4),
        true,
        t0,
    );
    let fine = app.ahead("vcf1", "cutoff").expect("held");
    let anchor = travel.position(800.0);
    let coarse = dragged_value(&travel, anchor, cell.1, cell.1 - 4, false);
    // Shift was down from the first move: re-measured from the grab, fine.
    assert!(fine < coarse);
    assert!(fine > 800.0);
    // Letting go of shift carries on from where it was.
    mouse(
        &mut app,
        MouseEventKind::Drag(LEFT),
        (cell.0, cell.1 - 4),
        false,
        t0,
    );
    assert!(travel.same(app.ahead("vcf1", "cutoff").expect("held"), fine));
}

#[test]
fn a_double_click_sends_a_knob_home() {
    let mut app = rack_app();
    let cell = knob_at(&app, 2, 0);
    let t0 = Instant::now();
    mouse(&mut app, MouseEventKind::Down(LEFT), cell, false, t0);
    mouse(&mut app, MouseEventKind::Up(LEFT), cell, false, t0);
    assert!(app.take_jobs().is_empty(), "a click turns nothing");
    let again = t0 + Duration::from_millis(200);
    mouse(&mut app, MouseEventKind::Down(LEFT), cell, false, again);
    let default = kazoo_wall::daemon::wall::catalogue()
        .kinds
        .iter()
        .find(|kind| kind.kind == "vcf")
        .and_then(|kind| kind.knobs.iter().find(|knob| knob.name == "cutoff"))
        .expect("a cutoff")
        .default;
    assert_eq!(
        calls(&mut app),
        vec![Request::Turn {
            module: "vcf1".to_string(),
            knob: "cutoff".to_string(),
            value: default,
            glide_beats: Some(DRAG_GLIDE),
        }]
    );
    // Two clicks too far apart are not a double-click.
    let slow = again + Duration::from_secs(1);
    mouse(&mut app, MouseEventKind::Up(LEFT), cell, false, slow);
    mouse(
        &mut app,
        MouseEventKind::Down(LEFT),
        cell,
        false,
        slow + Duration::from_secs(1),
    );
    assert!(app.take_jobs().is_empty());
}

#[test]
fn dragging_between_jacks_patches_either_way_round() {
    let mut app = rack_app();
    let t0 = Instant::now();
    // From the LFO's output onto the filter's resonance knob.
    let from = jack_at(&app, 1, "out", true);
    mouse(&mut app, MouseEventKind::Down(LEFT), from, false, t0);
    let target = knob_at(&app, 2, 1);
    mouse(&mut app, MouseEventKind::Drag(LEFT), target, false, t0);
    mouse(&mut app, MouseEventKind::Up(LEFT), target, false, t0);
    assert_eq!(
        calls(&mut app),
        vec![Request::Patch {
            from: "lfo1.out".to_string(),
            to: "vcf1.resonance".to_string(),
            amount: None,
        }]
    );
    // From an input to an output.
    let input = jack_at(&app, 3, "left", false);
    mouse(&mut app, MouseEventKind::Down(LEFT), input, false, t0);
    let output = jack_at(&app, 0, "out", true);
    mouse(&mut app, MouseEventKind::Up(LEFT), output, false, t0);
    assert_eq!(
        calls(&mut app),
        vec![Request::Patch {
            from: "vco1.out".to_string(),
            to: "out1.left".to_string(),
            amount: None,
        }]
    );
    // Output to output, or onto the bare rack, plugs nothing.
    mouse(&mut app, MouseEventKind::Down(LEFT), from, false, t0);
    mouse(&mut app, MouseEventKind::Up(LEFT), output, false, t0);
    mouse(&mut app, MouseEventKind::Down(LEFT), from, false, t0);
    let area = app.rack.area;
    mouse(
        &mut app,
        MouseEventKind::Up(LEFT),
        (area.x, area.y),
        false,
        t0,
    );
    assert!(app.take_jobs().is_empty());
    assert!(
        app.status(t0)
            .is_some_and(|status| status.text.contains("nothing was plugged"))
    );
}

#[test]
fn right_clicking_a_cable_or_its_input_unplugs_it() {
    let mut app = rack_app();
    let t0 = Instant::now();
    let area = app.rack.area;
    let visible = |point: &Point| {
        point.x >= app.rack.pan.x
            && point.y >= app.rack.pan.y
            && point.x - app.rack.pan.x < u32::from(area.width)
            && point.y - app.rack.pan.y < u32::from(area.height)
    };
    let cell = app
        .rack
        .cables
        .iter()
        .find(|(point, id)| **id == 2 && visible(point))
        .map(|(point, _)| *point)
        .expect("cable 2 is on screen");
    let cell = screen(&app, cell);
    mouse(
        &mut app,
        MouseEventKind::Down(MouseButton::Right),
        cell,
        false,
        t0,
    );
    assert_eq!(
        calls(&mut app),
        vec![Request::Unpatch {
            cable: Some(2),
            to: None
        }]
    );
    let input = jack_at(&app, 2, "in", false);
    mouse(
        &mut app,
        MouseEventKind::Down(MouseButton::Right),
        input,
        false,
        t0,
    );
    assert_eq!(
        calls(&mut app),
        vec![Request::Unpatch {
            cable: Some(1),
            to: None
        }]
    );
    // An output can feed many cables: it says to pick the cable.
    let output = jack_at(&app, 0, "out", true);
    mouse(
        &mut app,
        MouseEventKind::Down(MouseButton::Right),
        output,
        false,
        t0,
    );
    assert!(app.take_jobs().is_empty());
    assert_eq!(app.status(t0).map(|status| status.tone), Some(Tone::Info));
}

#[test]
fn the_wheel_turns_a_knob_finely_and_pans_the_rack() {
    let mut app = rack_app();
    let cell = knob_at(&app, 2, 0);
    let t0 = Instant::now();
    mouse(&mut app, MouseEventKind::ScrollUp, cell, false, t0);
    app.tick(t0 + Duration::from_millis(200));
    let expected = cutoff_travel().turned(800.0, true, Step::Fine);
    assert_eq!(
        calls(&mut app),
        vec![Request::Turn {
            module: "vcf1".to_string(),
            knob: "cutoff".to_string(),
            value: expected,
            glide_beats: Some(2.0),
        }]
    );

    // A long wall on a small screen pans.
    let mut long = snapshot();
    long.modules
        .extend((1..=8).map(|n| module(&format!("seq{n}"), "seq", None)));
    app.on_reply(Reply::Snapshot(Box::new(long)), t0);
    render(&mut app, 100, 30);
    assert_eq!(app.rack.pan, Point { x: 0, y: 0 });
    let bare = (app.rack.area.x, app.rack.area.y);
    mouse(&mut app, MouseEventKind::ScrollRight, bare, false, t0);
    assert_eq!(app.rack.pan.x, 8);
    mouse(&mut app, MouseEventKind::Down(LEFT), bare, false, t0);
    assert!(matches!(
        app.rack.layout.hit(Point { x: 8, y: 0 }),
        Hit::Background | Hit::Plate { .. } | Hit::Title { .. }
    ));
    mouse(
        &mut app,
        MouseEventKind::Drag(LEFT),
        (bare.0 + 5, bare.1),
        false,
        t0,
    );
    assert_eq!(app.rack.pan.x, 3, "the rack follows the mouse");
    mouse(
        &mut app,
        MouseEventKind::Drag(LEFT),
        (bare.0 + 50, bare.1),
        false,
        t0,
    );
    assert_eq!(app.rack.pan.x, 0, "never past the start");
    mouse(
        &mut app,
        MouseEventKind::Up(LEFT),
        (bare.0 + 50, bare.1),
        false,
        t0,
    );
    // Far past the end is held to the end.
    for _ in 0..200 {
        mouse(&mut app, MouseEventKind::ScrollRight, bare, false, t0);
    }
    render(&mut app, 100, 30);
    let end = app.rack.layout.width - u32::from(app.rack.area.width);
    assert_eq!(app.rack.pan.x, end);
    assert!(
        app.take_jobs().is_empty(),
        "panning changes nothing on the wall"
    );

    // A key brings the selection back into view.
    press(&mut app, KeyCode::Char('h'));
    render(&mut app, 100, 30);
    let selected = app.selected_index().expect("a selection");
    let plate = app.rack.layout.plate(selected).expect("its faceplate");
    assert!(plate.at.x >= app.rack.pan.x);
}

#[test]
fn the_list_view_and_open_prompts_leave_the_mouse_alone() {
    let mut app = connected();
    let t0 = Instant::now();
    mouse(&mut app, MouseEventKind::Down(LEFT), (10, 10), false, t0);
    assert!(app.take_jobs().is_empty());
    assert!(
        app.status(t0)
            .is_some_and(|status| status.text.contains("press v"))
    );
    let mut app = rack_app();
    press(&mut app, KeyCode::Char('t'));
    let cell = knob_at(&app, 2, 0);
    mouse(&mut app, MouseEventKind::Down(LEFT), cell, false, t0);
    assert!(app.rack.drag.is_none());
    assert!(app.take_jobs().is_empty());
}

#[test]
fn the_view_switches_and_tab_skips_the_hidden_cable_list() {
    let mut app = connected();
    press(&mut app, KeyCode::Char('v'));
    assert_eq!(app.view(), View::Rack);
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.focus(), Focus::Log);
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.focus(), Focus::Wall);
    press(&mut app, KeyCode::Char('v'));
    assert_eq!(app.view(), View::List);
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.focus(), Focus::Cables);
    // The view stays through a lost and found wall.
    press(&mut app, KeyCode::Char('v'));
    app.on_reply(
        Reply::Link(super::super::link::LinkState::Down {
            reason: "gone".to_string(),
            attempts: 1,
            was_up: true,
        }),
        Instant::now(),
    );
    assert_eq!(app.view(), View::Rack);
    assert_eq!(
        app.focus(),
        Focus::Wall,
        "the hidden cable list lost the focus"
    );
}

#[test]
fn the_middle_button_pans_from_anywhere_even_over_a_knob() {
    let mut app = rack_app();
    let mut long = snapshot();
    long.modules
        .extend((1..=8).map(|n| module(&format!("seq{n}"), "seq", None)));
    app.on_reply(Reply::Snapshot(Box::new(long)), Instant::now());
    render(&mut app, 100, 30);
    app.rack.pan.x = 20;
    render(&mut app, 100, 30);
    let cell = knob_at(&app, 2, 0);
    let t0 = Instant::now();
    let middle = MouseButton::Middle;
    mouse(&mut app, MouseEventKind::Down(middle), cell, false, t0);
    mouse(
        &mut app,
        MouseEventKind::Drag(middle),
        (cell.0 + 6, cell.1 - 3),
        false,
        t0,
    );
    assert_eq!(app.rack.pan.x, 14, "the rack follows the mouse");
    mouse(
        &mut app,
        MouseEventKind::Up(middle),
        (cell.0 + 6, cell.1 - 3),
        false,
        t0,
    );
    app.tick(t0 + Duration::from_secs(1));
    assert!(app.take_jobs().is_empty(), "the knob was not turned");
    assert!(app.rack.drag.is_none());
    assert_eq!(app.ahead("vcf1", "cutoff"), None);
}

#[test]
fn a_cable_carried_to_the_edge_pans_the_rack_that_way() {
    let mut app = rack_app();
    let mut long = snapshot();
    long.modules
        .extend((1..=8).map(|n| module(&format!("seq{n}"), "seq", None)));
    app.on_reply(Reply::Snapshot(Box::new(long)), Instant::now());
    render(&mut app, 100, 30);
    assert_eq!(app.rack.pan.x, 0);
    let t0 = Instant::now();
    let from = jack_at(&app, 1, "out", true);
    mouse(&mut app, MouseEventKind::Down(LEFT), from, false, t0);
    let area = app.rack.area;
    let edge = (area.x + area.width - 1, from.1);
    mouse(&mut app, MouseEventKind::Drag(LEFT), edge, false, t0);
    app.tick(t0);
    assert_eq!(app.rack.pan.x, 0, "not at once");
    app.tick(t0 + EDGE_STEP * 4);
    assert_eq!(app.rack.pan.x, 4 * EDGE_COLUMNS);
    app.tick(t0 + EDGE_STEP * 5);
    assert_eq!(app.rack.pan.x, 5 * EDGE_COLUMNS);
    // Away from the edge it stops.
    let inside = (area.x + area.width / 2, from.1);
    mouse(&mut app, MouseEventKind::Drag(LEFT), inside, false, t0);
    app.tick(t0 + EDGE_STEP * 20);
    assert_eq!(app.rack.pan.x, 5 * EDGE_COLUMNS);
    // Back at the left edge it pans back, never past the start.
    let left = (area.x, from.1);
    mouse(&mut app, MouseEventKind::Drag(LEFT), left, false, t0);
    let later = t0 + EDGE_STEP * 30;
    app.tick(later);
    app.tick(later + EDGE_STEP * 100);
    assert_eq!(app.rack.pan.x, 0);
    // Let go, and nothing pans any more.
    mouse(&mut app, MouseEventKind::Up(LEFT), left, false, later);
    app.rack.pan.x = 10;
    app.tick(later + EDGE_STEP * 200);
    assert_eq!(app.rack.pan.x, 10);
}

#[test]
fn compact_knobs_turn_and_overview_blocks_zoom_back_to_the_faceplates() {
    let mut app = rack_app();
    press(&mut app, KeyCode::Char('c'));
    render(&mut app, 160, 60);
    assert_eq!(app.rack.layout.density, Density::Compact);
    let cell = knob_at(&app, 2, 0);
    let t0 = Instant::now();
    mouse(&mut app, MouseEventKind::Down(LEFT), cell, false, t0);
    assert_eq!(app.selected_index(), Some(2));
    mouse(
        &mut app,
        MouseEventKind::Drag(LEFT),
        (cell.0, cell.1 - 4),
        false,
        t0,
    );
    mouse(
        &mut app,
        MouseEventKind::Up(LEFT),
        (cell.0, cell.1 - 4),
        false,
        t0,
    );
    let travel = cutoff_travel();
    let expected = dragged_value(&travel, travel.position(800.0), cell.1, cell.1 - 4, false);
    assert_eq!(
        calls(&mut app),
        vec![Request::Turn {
            module: "vcf1".to_string(),
            knob: "cutoff".to_string(),
            value: expected,
            glide_beats: Some(DRAG_GLIDE),
        }]
    );

    // The overview: a click on a block goes back to compact, there.
    press(&mut app, KeyCode::Char('c'));
    render(&mut app, 160, 60);
    assert_eq!(app.rack.layout.density, Density::Overview);
    let block = app.rack.layout.plate(3).expect("the out's block");
    let name = screen(
        &app,
        Point {
            x: block.at.x + 2,
            y: block.at.y + 1,
        },
    );
    mouse(&mut app, MouseEventKind::Down(LEFT), name, false, t0);
    mouse(&mut app, MouseEventKind::Up(LEFT), name, false, t0);
    assert_eq!(app.selected_index(), Some(3));
    assert_eq!(app.rack.density, Density::Compact);
    render(&mut app, 160, 60);
    assert_eq!(app.rack.layout.density, Density::Compact);
    let plate = app.rack.layout.plate(3).expect("the out's faceplate");
    assert!(plate.at.x >= app.rack.pan.x);
    assert!(
        app.take_jobs().is_empty(),
        "zooming changes nothing on the wall"
    );

    // Jacks still patch in the overview.
    press(&mut app, KeyCode::Char('c'));
    render(&mut app, 160, 60);
    assert_eq!(app.rack.layout.density, Density::Overview);
    let from = jack_at(&app, 0, "out", true);
    let to = jack_at(&app, 3, "right", false);
    mouse(&mut app, MouseEventKind::Down(LEFT), from, false, t0);
    mouse(&mut app, MouseEventKind::Drag(LEFT), to, false, t0);
    mouse(&mut app, MouseEventKind::Up(LEFT), to, false, t0);
    assert_eq!(
        calls(&mut app),
        vec![Request::Patch {
            from: "vco1.out".to_string(),
            to: "out1.right".to_string(),
            amount: None,
        }]
    );
    assert_eq!(app.rack.density, Density::Overview, "a jack does not zoom");
}

/// A console in the rack view of a wall that keeps rows: vco1 and lfo1 on
/// the top row, vcf1 and out1 below.
fn rows_app() -> App {
    let mut app = connected();
    press(&mut app, KeyCode::Char('v'));
    let mut wall = snapshot();
    wall.rack = Some(rows(&[&["vco1", "lfo1"], &["vcf1", "out1"]]));
    app.on_reply(Reply::Snapshot(Box::new(wall)), Instant::now());
    render(&mut app, 160, 90);
    app
}

fn rows(rows: &[&[&str]]) -> Vec<Vec<String>> {
    rows.iter()
        .map(|row| row.iter().map(|id| (*id).to_string()).collect())
        .collect()
}

/// The middle of a faceplate's name.
fn title_at(app: &App, module: usize) -> (u16, u16) {
    let plate = app.rack.layout.plate(module).expect("a faceplate");
    screen(
        app,
        Point {
            x: plate.at.x + u32::from(plate.width / 2),
            y: plate.at.y + 1,
        },
    )
}

#[test]
fn the_rack_hangs_in_the_wall_s_own_rows() {
    let app = rows_app();
    let layout = &app.rack.layout;
    let top = |index: usize| layout.plate(index).expect("a faceplate").at.y;
    assert_eq!(top(0), top(1));
    assert_eq!(top(2), top(3));
    assert!(top(2) > top(0));
    assert_eq!(layout.bands.len(), 2);
}

#[test]
fn dragging_a_title_moves_the_faceplate_with_one_arrange() {
    let mut app = rows_app();
    let t0 = Instant::now();
    // vcf1, by its title, onto the left of vco1: in front of it.
    let from = title_at(&app, 2);
    let vco = app.rack.layout.plate(0).expect("vco1").clone();
    let onto = screen(
        &app,
        Point {
            x: vco.at.x + 1,
            y: vco.at.y + 5,
        },
    );
    mouse(&mut app, MouseEventKind::Down(LEFT), from, false, t0);
    assert_eq!(app.selected_index(), Some(2));
    mouse(&mut app, MouseEventKind::Drag(LEFT), onto, false, t0);
    // The rack makes room as it goes.
    let preview = rows(&[&["vcf1", "vco1", "lfo1"], &["out1"]]);
    assert_eq!(app.rack.rows(None), Some(preview.as_slice()));
    render(&mut app, 160, 90);
    let vcf = app.rack.layout.plate(2).expect("vcf1").at;
    let vco = app.rack.layout.plate(0).expect("vco1").at;
    assert_eq!(vcf.y, vco.y);
    assert!(vcf.x < vco.x);
    assert!(
        app.take_jobs().is_empty(),
        "nothing is sent until it is let go"
    );
    mouse(&mut app, MouseEventKind::Up(LEFT), onto, false, t0);
    assert_eq!(
        calls(&mut app),
        vec![Request::Arrange {
            module: "vcf1".to_string(),
            row: 0,
            before: Some("vco1".to_string()),
            own: false,
        }]
    );
    // Shown ahead of the wall, until the wall shows it.
    let mut stale = snapshot();
    stale.rack = Some(rows(&[&["vco1", "lfo1"], &["vcf1", "out1"]]));
    app.on_reply(Reply::Snapshot(Box::new(stale)), t0);
    render(&mut app, 160, 90);
    assert_eq!(app.rack.layout.plate(2).expect("vcf1").at.y, vco.y);
    app.on_reply(
        Reply::Event(kazoo_wall::protocol::Event::Rack {
            rows: preview.clone(),
        }),
        t0,
    );
    assert!(app.rack.sent.is_none(), "the wall shows it");
    assert_eq!(
        app.rack
            .rows(app.snapshot().and_then(|s| s.rack.as_deref())),
        Some(preview.as_slice())
    );
}

#[test]
fn a_faceplate_goes_below_the_rows_between_them_or_back_where_it_was() {
    let t0 = Instant::now();
    // Below the last row: a new bottom row.
    let mut app = rows_app();
    let from = title_at(&app, 1);
    let spare = app.rack.layout.blanks.last().expect("the spare row").at;
    let below = screen(
        &app,
        Point {
            x: spare.x + 20,
            y: spare.y + 1,
        },
    );
    mouse(&mut app, MouseEventKind::Down(LEFT), from, false, t0);
    mouse(&mut app, MouseEventKind::Drag(LEFT), below, false, t0);
    mouse(&mut app, MouseEventKind::Up(LEFT), below, false, t0);
    assert_eq!(
        calls(&mut app),
        vec![Request::Arrange {
            module: "lfo1".to_string(),
            row: 2,
            before: None,
            own: true,
        }]
    );

    // On the gap between the rows: a row of its own there.
    let mut app = rows_app();
    let from = title_at(&app, 3);
    let (top, height) = app.rack.layout.bands[0];
    let gap = screen(
        &app,
        Point {
            x: 5,
            y: top + u32::from(height),
        },
    );
    mouse(&mut app, MouseEventKind::Down(LEFT), from, false, t0);
    mouse(&mut app, MouseEventKind::Drag(LEFT), gap, false, t0);
    mouse(&mut app, MouseEventKind::Up(LEFT), gap, false, t0);
    assert_eq!(
        calls(&mut app),
        vec![Request::Arrange {
            module: "out1".to_string(),
            row: 1,
            before: None,
            own: true,
        }]
    );

    // Let go where it hangs: nothing moves, nothing is sent.
    let mut app = rows_app();
    let from = title_at(&app, 1);
    let nearby = (from.0 + 1, from.1);
    mouse(&mut app, MouseEventKind::Down(LEFT), from, false, t0);
    mouse(&mut app, MouseEventKind::Drag(LEFT), nearby, false, t0);
    mouse(&mut app, MouseEventKind::Up(LEFT), nearby, false, t0);
    assert!(app.take_jobs().is_empty());
    assert!(app.rack.moving.is_none());
}

#[test]
fn a_faceplate_s_body_still_pans_and_a_refused_move_goes_back() {
    let mut app = rows_app();
    let t0 = Instant::now();
    render(&mut app, 100, 30);
    assert!(
        app.rack.layout.height > u32::from(app.rack.area.height),
        "two rows run below"
    );
    assert_eq!(app.rack.pan.y, 0);
    // A cell of vco1's panel that is neither title, knob nor jack.
    let vco = app.rack.layout.plate(0).expect("vco1").clone();
    let area = app.rack.area;
    let body = (vco.at.y..vco.at.y + u32::from(vco.height))
        .flat_map(|y| (vco.at.x..vco.at.x + u32::from(vco.width)).map(move |x| Point { x, y }))
        .filter(|point| app.rack.layout.hit(*point) == Hit::Plate { module: 0 })
        .find(|point| {
            point.x < u32::from(area.width) && point.y >= 4 && point.y < u32::from(area.height)
        })
        .map(|point| screen(&app, point))
        .expect("a bare cell of the panel in view");
    mouse(&mut app, MouseEventKind::Down(LEFT), body, false, t0);
    let up = (body.0, body.1 - 4);
    mouse(&mut app, MouseEventKind::Drag(LEFT), up, false, t0);
    assert_eq!(app.rack.pan.y, 4, "the body pans");
    mouse(&mut app, MouseEventKind::Up(LEFT), up, false, t0);
    assert!(app.take_jobs().is_empty());

    // A move the wall refuses goes back to where the wall has it.
    let mut app = rows_app();
    let from = title_at(&app, 3);
    let onto = title_at(&app, 0);
    mouse(&mut app, MouseEventKind::Down(LEFT), from, false, t0);
    mouse(&mut app, MouseEventKind::Drag(LEFT), onto, false, t0);
    mouse(&mut app, MouseEventKind::Up(LEFT), onto, false, t0);
    let sent = calls(&mut app);
    assert_eq!(sent.len(), 1);
    assert!(app.rack.sent.is_some());
    app.on_reply(
        Reply::Answer {
            request: sent[0].clone(),
            outcome: Err(super::super::worker::Failure {
                code: Some(kazoo_wall::protocol::ErrorCode::SlowDown),
                message: "slow down".to_string(),
            }),
        },
        t0,
    );
    assert!(app.rack.sent.is_none() && app.rack.moving.is_none());
    assert!(
        app.status(t0)
            .is_some_and(|status| status.tone == Tone::Trouble)
    );

    // One never confirmed is let go after a while.
    let mut app = rows_app();
    mouse(&mut app, MouseEventKind::Down(LEFT), from, false, t0);
    mouse(&mut app, MouseEventKind::Drag(LEFT), onto, false, t0);
    mouse(&mut app, MouseEventKind::Up(LEFT), onto, false, t0);
    assert_eq!(calls(&mut app).len(), 1);
    app.tick(t0 + Duration::from_secs(1));
    assert!(app.rack.sent.is_some());
    app.tick(t0 + Duration::from_secs(3));
    assert!(app.rack.sent.is_none());
}

#[test]
fn an_older_wall_pans_from_a_title_and_moves_nothing() {
    let mut app = rack_app();
    let t0 = Instant::now();
    let mut long = snapshot();
    long.modules
        .extend((1..=8).map(|n| module(&format!("seq{n}"), "seq", None)));
    app.on_reply(Reply::Snapshot(Box::new(long)), t0);
    app.rack.pan.x = 20;
    render(&mut app, 100, 30);
    let title = title_at(&app, 3);
    mouse(&mut app, MouseEventKind::Down(LEFT), title, false, t0);
    mouse(
        &mut app,
        MouseEventKind::Drag(LEFT),
        (title.0 + 5, title.1),
        false,
        t0,
    );
    mouse(
        &mut app,
        MouseEventKind::Up(LEFT),
        (title.0 + 5, title.1),
        false,
        t0,
    );
    assert_eq!(app.rack.pan.x, 15);
    assert!(app.take_jobs().is_empty());
}

#[test]
fn a_blank_panel_or_a_double_click_on_bare_rack_adds_a_module_there() {
    let mut app = rows_app();
    let t0 = Instant::now();
    let blanks = app.rack.layout.blanks.clone();
    assert_eq!(blanks.len(), 3, "one ending each row, and the spare row");
    // The blank ending row 2.
    let blank = blanks[1];
    let cell = screen(
        &app,
        Point {
            x: blank.at.x + 2,
            y: blank.at.y + 2,
        },
    );
    mouse(&mut app, MouseEventKind::Down(LEFT), cell, false, t0);
    assert!(matches!(
        app.mode(),
        Mode::Add { place: Some(place), .. } if *place == Place::end_of(1)
    ));
    let screen_text = super::render::text(&render(&mut app, 160, 90));
    assert!(
        screen_text.contains("add a module to row 2 after out1"),
        "{screen_text}"
    );
    press(&mut app, KeyCode::Esc);
    // The spare row: a new row.
    let spare = blanks[2];
    let cell = screen(
        &app,
        Point {
            x: spare.at.x + 1,
            y: spare.at.y + 1,
        },
    );
    mouse(&mut app, MouseEventKind::Down(LEFT), cell, false, t0);
    let Mode::Add {
        place: Some(place), ..
    } = app.mode().clone()
    else {
        panic!("no picker");
    };
    assert_eq!(app.place_words(&place), "on a new row");
    press(&mut app, KeyCode::Esc);

    // Double-click on the bare rack between rows: a row of its own there.
    let (top, height) = app.rack.layout.bands[0];
    let vco = app.rack.layout.plate(0).expect("vco1").at;
    let gap = screen(
        &app,
        Point {
            x: vco.x + 2,
            y: top + u32::from(height),
        },
    );
    mouse(&mut app, MouseEventKind::Down(LEFT), gap, false, t0);
    mouse(&mut app, MouseEventKind::Up(LEFT), gap, false, t0);
    assert_eq!(app.mode(), &Mode::Normal, "one click pans");
    mouse(
        &mut app,
        MouseEventKind::Down(LEFT),
        gap,
        false,
        t0 + Duration::from_millis(150),
    );
    let Mode::Add {
        place: Some(place), ..
    } = app.mode().clone()
    else {
        panic!("no picker");
    };
    assert_eq!(
        place,
        Place {
            row: 1,
            before: None,
            own: true
        }
    );
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    assert!(matches!(
        &calls(&mut app)[..],
        [Request::Add { place: Some(sent), .. }] if *sent == place
    ));
}

#[test]
fn blank_panels_are_drawn_in_every_density_and_not_on_an_older_wall() {
    let mut app = rows_app();
    for _ in 0..3 {
        let screen = super::render::text(&render(&mut app, 160, 90));
        assert!(screen.contains("+ add"), "{screen}");
        press(&mut app, KeyCode::Char('c'));
    }
    let mut old = rack_app();
    let screen = super::render::text(&render(&mut old, 160, 60));
    assert!(!screen.contains("+ add"), "{screen}");
    assert!(old.rack.layout.blanks.is_empty());
}
