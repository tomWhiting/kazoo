//! The screen, drawn into ratatui's test backend.

use std::time::Instant;

use crossterm::event::KeyCode;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

use super::super::app::App;
use super::super::draw::{self, MIN_HEIGHT, MIN_WIDTH};
use super::super::link::LinkState;
use super::super::rack::geometry::Point;
use super::super::theme::cable_colour;
use super::super::worker::Reply;
use super::{cable, connected, module, press, snapshot};

/// A fixed "now" for the log's ages: two minutes after the fixtures'
/// changes.
const NOW: i64 = 1_790_424_001 + 120;

pub fn render(app: &mut App, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a test terminal");
    terminal
        .draw(|frame| draw::draw(frame, app, Instant::now(), Some(NOW)))
        .expect("the frame draws");
    terminal.backend().buffer().clone()
}

pub fn text(buf: &Buffer) -> String {
    let mut out = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

/// Where `needle` first appears on screen.
fn find(buf: &Buffer, needle: &str) -> Option<(u16, u16)> {
    let rows: Vec<String> = text(buf).lines().map(str::to_string).collect();
    for (y, row) in rows.iter().enumerate() {
        if let Some(byte) = row.find(needle) {
            let x = row[..byte].chars().count();
            return Some((
                u16::try_from(x).expect("fits"),
                u16::try_from(y).expect("fits"),
            ));
        }
    }
    None
}

#[test]
fn a_snapshot_shows_the_wall_its_cables_and_its_log() {
    let mut app = connected();
    let buf = render(&mut app, 120, 36);
    let screen = text(&buf);
    for expected in [
        "THE WALL",
        "96 BPM",
        "bar 12 · beat 2",
        "clock: own",
        "glide 2 beats",
        "seats: Tom, Waffles",
        "● live",
        "“dark, sparse, slow pulse around A2, quiet”",
        "vco1",
        "lfo1 · slow wobble",
        "vcf1",
        "out1",
        "cutoff",
        "420 Hz → 800 Hz",
        "CABLES",
        "3 of 320",
        "#2",
        "lfo1.out",
        "→ vcf1.cutoff",
        "+0.40",
        "LOG",
        "#57",
        "Waffles turned vcf1 cutoff 420 Hz → 800 Hz over 4 beats",
        "2m",
        "Waffles 70% · Tom 30%",
    ] {
        assert!(
            screen.contains(expected),
            "missing '{expected}' in\n{screen}"
        );
    }

    // The cutoff's jack is filled, in the colour of cable 2.
    let (x, y) = find(&buf, "◆ cutoff").expect("the cutoff row");
    let jack = &buf[(x, y)];
    assert_eq!(jack.symbol(), "◆");
    assert_eq!(jack.fg, cable_colour(2));
    // And the filter's audio input is round, in cable 1's colour.
    let (x, y) = find(&buf, "●in").expect("the filter's input");
    assert_eq!(buf[(x, y)].fg, cable_colour(1));
}

#[test]
fn a_tiny_terminal_asks_for_a_bigger_window() {
    let mut app = connected();
    for (width, height) in [(MIN_WIDTH - 1, 40), (120, MIN_HEIGHT - 1), (20, 5), (1, 1)] {
        let screen = text(&render(&mut app, width, height));
        assert!(!screen.contains("THE WALL"), "{width}×{height}:\n{screen}");
        if height >= 3 && width >= 40 {
            assert!(
                screen.contains("make the window bigger"),
                "{width}×{height}:\n{screen}"
            );
        }
    }
}

#[test]
fn every_size_from_the_smallest_up_draws_without_spilling() {
    let mut app = connected();
    for width in [MIN_WIDTH, 100, 133, 180, 260, 400] {
        for height in [MIN_HEIGHT, 30, 50, 120] {
            let buf = render(&mut app, width, height);
            let screen = text(&buf);
            assert!(screen.contains("THE WALL"), "{width}×{height}");
            assert!(screen.contains("CABLES"), "{width}×{height}");
            assert!(screen.contains("LOG"), "{width}×{height}");
        }
    }
    // Every popup, at the smallest size.
    for key in ['?', 'a', 'x', 'Q', 't', 'p'] {
        let mut app = connected();
        press(&mut app, KeyCode::Char(key));
        let screen = text(&render(&mut app, MIN_WIDTH, MIN_HEIGHT));
        assert!(screen.contains("THE WALL"), "{key}:\n{screen}");
    }
}

#[test]
fn long_names_truncate_inside_their_panel() {
    let mut app = connected();
    let mut wall = snapshot();
    wall.modules = vec![
        module("vco12", "vco", Some("an-extremely-long-namex")),
        module("lfo7", "lfo", None),
    ];
    wall.cables = vec![cable(
        140,
        "vco12.out",
        "lfo7.an_input_with_a_very_long_name_indeed",
        -0.25,
    )];
    app.on_reply(Reply::Snapshot(Box::new(wall)), Instant::now());
    let buf = render(&mut app, 100, 30);
    let screen = text(&buf);
    assert!(screen.contains("vco12 · an-extremely-lon…"), "{screen}");
    assert!(screen.contains("-0.25"), "{screen}");
    assert!(screen.contains("→ lfo7.an_input_with_a_…"), "{screen}");
    // Two columns of panels, 34 wide from column 1: the top edge ends in
    // its corner, not in spilled text.
    let (_, top) = find(&buf, "vco12").expect("the panel's title");
    assert_eq!(buf[(34, top)].symbol(), "╮");
    assert_eq!(buf[(1, top)].symbol(), "╭");
}

#[test]
fn a_lost_wall_says_so_and_keeps_the_last_view() {
    let mut app = connected();
    app.on_reply(
        Reply::Link(LinkState::Down {
            reason: "the wall closed the connection".to_string(),
            attempts: 3,
            was_up: true,
        }),
        Instant::now(),
    );
    let screen = text(&render(&mut app, 140, 40));
    assert!(screen.contains("● lost: reconnecting"), "{screen}");
    assert!(
        screen.contains(
            "the wall is not answering (the wall closed the connection); reconnecting, attempt 3"
        ),
        "{screen}"
    );
    assert!(screen.contains("vcf1"), "{screen}");

    // Before any snapshot, the screen says where it is looking.
    let mut fresh = App::new("Tom", "/tmp/kw/kazoo-wall.sock".to_string());
    fresh.on_reply(
        Reply::Link(LinkState::Down {
            reason: "No such file or directory (os error 2)".to_string(),
            attempts: 2,
            was_up: false,
        }),
        Instant::now(),
    );
    let screen = text(&render(&mut fresh, 120, 36));
    assert!(
        screen.contains("the wall is not answering at /tmp/kw/kazoo-wall.sock"),
        "{screen}"
    );
    assert!(screen.contains("s starts it"), "{screen}");
    assert!(screen.contains("● not playing"), "{screen}");
}

#[test]
fn a_tall_wall_scrolls_to_the_selected_knob() {
    let mut app = connected();
    let mut wall = snapshot();
    wall.modules = (1..=12)
        .map(|n| module(&format!("seq{n}"), "seq", None))
        .collect();
    app.on_reply(Reply::Snapshot(Box::new(wall)), Instant::now());
    // The first frame tells the keys how many panels are across.
    render(&mut app, 100, 30);
    assert_eq!(app.columns, 2);
    // Down through every knob of the first column, into the next row.
    for _ in 0..30 {
        press(&mut app, KeyCode::Char('j'));
    }
    let screen = text(&render(&mut app, 100, 30));
    assert!(screen.contains("▲ more above"), "{screen}");
    assert!(screen.contains("▼ more below"), "{screen}");
    assert!(app.scroll > 0);
    let selected = app.selected_index().expect("a selection");
    assert!(selected > 0, "moved down to another module");
}

/// Whether any cell shows braille in `colour`.
fn braille_in(buf: &Buffer, colour: ratatui::style::Color) -> bool {
    buf.content().iter().any(|cell| {
        cell.fg == colour
            && cell
                .symbol()
                .chars()
                .next()
                .is_some_and(|ch| ('\u{2801}'..='\u{28ff}').contains(&ch))
    })
}

#[test]
fn the_rack_view_draws_faceplates_dials_sockets_and_hanging_cables() {
    let mut app = connected();
    press(&mut app, KeyCode::Char('v'));
    let buf = render(&mut app, 160, 60);
    let screen = text(&buf);
    for expected in [
        "THE WALL",
        " slow wobble ",
        "lfo · lfo1",
        "vcf",
        "cutoff",
        // A name wider than its dial is cut short, and says so.
        "resonan…",
        "(●)",
        "(○)",
        "LOG",
    ] {
        assert!(
            screen.contains(expected),
            "missing '{expected}' in\n{screen}"
        );
    }
    // The dials are braille, and cable 2 hangs in its colour (no dye yet).
    assert!(braille_in(&buf, super::super::theme::KNOB));
    assert!(braille_in(&buf, cable_colour(2)), "{screen}");
    // The out has its meter, marked as not reported.
    assert!(screen.contains("--"), "{screen}");
    // No cable list in the rack view.
    assert!(!screen.contains("CABLES"), "{screen}");
}

#[test]
fn a_cable_with_a_fingerprint_hangs_in_its_dye() {
    let mut app = connected();
    let mut dyed = snapshot();
    dyed.fingerprints.cables.insert(
        "2".to_string(),
        std::collections::BTreeMap::from([("Waffles".to_string(), 1.0)]),
    );
    let dye = super::super::dye::mix(&dyed.fingerprints.cables["2"]).expect("a dye");
    app.on_reply(Reply::Snapshot(Box::new(dyed)), Instant::now());
    press(&mut app, KeyCode::Char('v'));
    let buf = render(&mut app, 160, 60);
    assert!(braille_in(&buf, dye));
}

#[test]
fn the_rack_view_draws_at_every_size_and_under_every_popup() {
    let mut app = connected();
    press(&mut app, KeyCode::Char('v'));
    for width in [MIN_WIDTH, 100, 133, 260] {
        for height in [MIN_HEIGHT, 30, 50, 120] {
            let screen = text(&render(&mut app, width, height));
            assert!(screen.contains("THE WALL"), "{width}×{height}");
        }
    }
    for key in ['?', 'a', 'x', 'Q', 't', 'p'] {
        let mut app = connected();
        press(&mut app, KeyCode::Char('v'));
        press(&mut app, KeyCode::Char(key));
        let screen = text(&render(&mut app, MIN_WIDTH, MIN_HEIGHT));
        assert!(screen.contains("THE WALL"), "{key}:\n{screen}");
    }
    // Lost, and empty.
    app.on_reply(
        Reply::Link(LinkState::Down {
            reason: "gone".to_string(),
            attempts: 2,
            was_up: true,
        }),
        Instant::now(),
    );
    let screen = text(&render(&mut app, 120, 40));
    assert!(screen.contains("not answering"), "{screen}");
    let mut empty = snapshot();
    empty.modules.clear();
    empty.cables.clear();
    app.on_reply(Reply::Snapshot(Box::new(empty)), Instant::now());
    let screen = text(&render(&mut app, 120, 40));
    assert!(screen.contains("the wall is empty"), "{screen}");
    assert!(app.rack.layout.plates.is_empty());
}

#[test]
fn the_header_says_whether_the_wall_is_heard() {
    let mut app = connected();
    let screen = text(&render(&mut app, 120, 30));
    assert!(screen.contains("│  heard  │"), "{screen}");
    let mut silent = snapshot();
    silent.heard = false;
    app.on_reply(Reply::Snapshot(Box::new(silent)), Instant::now());
    let screen = text(&render(&mut app, 120, 30));
    assert!(screen.contains("silent (m to hear)"), "{screen}");
    assert!(!screen.contains("REC"), "{screen}");
}

#[test]
fn the_header_shows_a_recording_under_way() {
    let mut app = connected();
    let mut recording = snapshot();
    recording.heard = false;
    recording.recording = Some(kazoo_wall::protocol::Recording {
        path: "/m/wall-2026-09-27-203001.wav".to_string(),
        seat: "Tom".to_string(),
        seconds: 200.5,
        dropped: 0,
        sample_rate: 48_000,
    });
    app.on_reply(Reply::Snapshot(Box::new(recording.clone())), Instant::now());
    let screen = text(&render(&mut app, 160, 30));
    assert!(screen.contains("silent (m to hear)"), "{screen}");
    assert!(screen.contains(" ● REC 3:20 "), "{screen}");
    if let Some(under_way) = recording.recording.as_mut() {
        under_way.dropped = 128;
    }
    app.on_reply(Reply::Snapshot(Box::new(recording)), Instant::now());
    let screen = text(&render(&mut app, 160, 30));
    assert!(screen.contains(" ● REC 3:20 · 128 lost "), "{screen}");
}

/// A full wall: every module the wall holds, of every sort, and every
/// cable, each into its own knob.
fn full_wall() -> kazoo_wall::protocol::Snapshot {
    const KINDS: [&str; 12] = [
        "vco", "lfo", "vcf", "vca", "env", "seq", "mix", "noise", "sh", "slew", "clock", "out",
    ];
    let mut wall = snapshot();
    wall.modules = (0..kazoo_wall::MAX_MODULES)
        .map(|n| {
            let kind = KINDS[n % KINDS.len()];
            module(&format!("{kind}{n}"), kind, None)
        })
        .collect();
    let sources: Vec<String> = wall
        .modules
        .iter()
        .filter_map(|view| Some(format!("{}.{}", view.id, view.outputs.first()?)))
        .collect();
    let inputs = wall.modules.iter().flat_map(|view| {
        view.knobs
            .iter()
            .map(move |knob| format!("{}.{}", view.id, knob.name))
    });
    wall.cables = inputs
        .take(kazoo_wall::MAX_CABLES)
        .enumerate()
        .map(|(n, to)| {
            let id = u32::try_from(n + 1).expect("few cables");
            cable(id, &sources[(n * 7 + 3) % sources.len()], &to, 1.0)
        })
        .collect();
    assert_eq!(wall.cables.len(), kazoo_wall::MAX_CABLES);
    wall
}

#[test]
fn a_full_wall_is_laid_out_once_and_its_cables_hang_from_frame_to_frame() {
    let mut app = connected();
    press(&mut app, KeyCode::Char('v'));
    app.on_reply(Reply::Snapshot(Box::new(full_wall())), Instant::now());
    let buf = render(&mut app, 200, 60);
    assert_eq!(app.rack.builds.layouts, 1);
    let cables = u64::try_from(kazoo_wall::MAX_CABLES).expect("fits");
    assert_eq!(app.rack.builds.paths, cables, "every cable hangs once");
    assert_eq!(app.rack.paths.len(), kazoo_wall::MAX_CABLES);

    // Cables that run off the screen still hang where they are in view.
    let area = app.rack.area;
    let pan = app.rack.pan;
    let in_view = |point: &Point| {
        point.x >= pan.x
            && point.y >= pan.y
            && point.x - pan.x < u32::from(area.width)
            && point.y - pan.y < u32::from(area.height)
    };
    let crossing: Vec<u32> = app
        .rack
        .paths
        .iter()
        .filter(|(_, path)| {
            path.cells.iter().any(|(cell, _)| in_view(cell))
                && path.cells.iter().any(|(cell, _)| !in_view(cell))
        })
        .map(|((id, ..), _)| *id)
        .collect();
    assert!(!crossing.is_empty(), "some cables run off the screen");
    assert!(
        app.rack
            .cables
            .iter()
            .any(|(point, id)| crossing.contains(id) && in_view(point)),
        "{}",
        text(&buf)
    );

    // The next frames make nothing again: not the layout, not a path;
    // panning moves the view, not the rack.
    render(&mut app, 200, 60);
    press(&mut app, KeyCode::Char('l'));
    render(&mut app, 200, 60);
    app.rack.pan.x += 40;
    render(&mut app, 200, 60);
    assert_eq!(app.rack.builds.layouts, 1);
    assert_eq!(app.rack.builds.paths, cables);

    // A knob moving changes nothing either; a module's shape changing, or
    // a cable moving, is made again (that cable alone).
    let mut turned = full_wall();
    turned.modules[2].knobs[0].value += 1.0;
    turned.modules[2].name = Some("renamed".to_string());
    app.on_reply(Reply::Snapshot(Box::new(turned.clone())), Instant::now());
    render(&mut app, 200, 60);
    assert_eq!(app.rack.builds.layouts, 1);
    assert_eq!(app.rack.builds.paths, cables);
    turned.cables[0].to = "vcf2.resonance".to_string();
    app.on_reply(Reply::Snapshot(Box::new(turned.clone())), Instant::now());
    render(&mut app, 200, 60);
    assert_eq!(app.rack.builds.layouts, 1);
    assert_eq!(app.rack.builds.paths, cables + 1);
    turned.modules.pop();
    turned
        .cables
        .retain(|cable| !cable.from.starts_with("out95.") && !cable.to.starts_with("out95."));
    app.on_reply(Reply::Snapshot(Box::new(turned.clone())), Instant::now());
    render(&mut app, 200, 60);
    assert_eq!(app.rack.builds.layouts, 2);
    assert_eq!(
        app.rack.paths.len(),
        turned.cables.len(),
        "gone cables are forgotten"
    );
    // A new screen height lays it out again.
    render(&mut app, 200, 80);
    assert_eq!(app.rack.builds.layouts, 3);
}

#[test]
fn a_cable_to_a_faceplate_out_of_view_hangs_from_the_one_in_view() {
    let mut app = connected();
    press(&mut app, KeyCode::Char('v'));
    let mut wall = snapshot();
    wall.modules = vec![module("lfo1", "lfo", None)];
    wall.modules
        .extend((1..=10).map(|n| module(&format!("seq{n}"), "seq", None)));
    wall.modules.push(module("vcf1", "vcf", None));
    wall.cables = vec![cable(7, "lfo1.out", "vcf1.cutoff", 1.0)];
    app.on_reply(Reply::Snapshot(Box::new(wall)), Instant::now());
    let buf = render(&mut app, 120, 50);
    let filter = app.rack.layout.plate(11).expect("the filter");
    assert!(
        filter.at.x > app.rack.pan.x + u32::from(app.rack.area.width),
        "the filter is far off to the right"
    );
    assert!(
        app.rack.cables.values().any(|&id| id == 7),
        "the cable hangs from the lfo:\n{}",
        text(&buf)
    );
    assert!(braille_in(&buf, cable_colour(7)));
    // Panned to the filter's end, it hangs there too.
    let far = filter.at.x;
    app.rack.pan.x = far;
    let buf = render(&mut app, 120, 50);
    assert!(app.rack.cables.values().any(|&id| id == 7));
    assert!(braille_in(&buf, cable_colour(7)));
    // Half way along it sags below the screen: nothing of it is drawn.
    app.rack.pan.x = far / 2;
    let buf = render(&mut app, 120, 50);
    assert!(!app.rack.cables.values().any(|&id| id == 7));
    assert!(!braille_in(&buf, cable_colour(7)));
}

#[test]
fn compact_and_overview_keep_the_faceplates_look_and_the_hanging_cables() {
    let mut app = connected();
    press(&mut app, KeyCode::Char('v'));
    press(&mut app, KeyCode::Char('c'));
    let buf = render(&mut app, 160, 50);
    let screen = text(&buf);
    for expected in [
        " slow wobbl…",
        // Every knob shows its value.
        "800 Hz",
        "0.5 Hz",
        "0 oct",
        "(●)in",
        "(○)right",
        "┌◦",
    ] {
        assert!(
            screen.contains(expected),
            "missing '{expected}' in\n{screen}"
        );
    }
    assert!(!screen.contains("lfo · lfo1"), "one title line: {screen}");
    assert!(braille_in(&buf, super::super::theme::KNOB), "small dials");
    assert!(braille_in(&buf, cable_colour(2)), "cables hang: {screen}");
    // The chosen faceplate's edge is brass on cream, as ever.
    let (x, y) = find(&buf, "┌◦").expect("a faceplate");
    assert_eq!(buf[(x, y)].bg, super::super::theme::CREAM);

    press(&mut app, KeyCode::Char('c'));
    let buf = render(&mut app, 160, 50);
    let screen = text(&buf);
    for expected in ["vco1", "slow wobb…", "lfo · lfo1", "vcf1", "out1", "┌◦"] {
        assert!(
            screen.contains(expected),
            "missing '{expected}' in\n{screen}"
        );
    }
    assert!(
        !screen.contains("0.5 Hz"),
        "no knobs in the overview: {screen}"
    );
    assert!(braille_in(&buf, cable_colour(2)), "cables hang: {screen}");
    assert!(braille_in(&buf, cable_colour(1)), "cables hang: {screen}");
    // A plugged socket is filled in its cable's colour.
    let filter = app.rack.layout.plate(2).expect("the filter");
    let input = filter
        .jacks
        .iter()
        .find(|jack| jack.name == "in")
        .expect("an input");
    let area = app.rack.area;
    let cell = (
        area.x + u16::try_from(input.at.x - app.rack.pan.x).expect("in view"),
        area.y + u16::try_from(input.at.y - app.rack.pan.y).expect("in view"),
    );
    assert_eq!(buf[cell].symbol(), "●");
    assert_eq!(buf[cell].fg, cable_colour(1));

    // Back to full: today's faceplates.
    press(&mut app, KeyCode::Char('c'));
    let screen = text(&render(&mut app, 160, 50));
    assert!(screen.contains("lfo · lfo1"), "{screen}");
    assert!(screen.contains("resonan…"), "{screen}");
    // Every density draws at every size.
    for _ in 0..3 {
        press(&mut app, KeyCode::Char('c'));
        for (width, height) in [(MIN_WIDTH, MIN_HEIGHT), (133, 50), (260, 120)] {
            let screen = text(&render(&mut app, width, height));
            assert!(screen.contains("THE WALL"), "{width}×{height}");
        }
    }
    // The list view is not drawn by density: c says where it works.
    press(&mut app, KeyCode::Char('v'));
    press(&mut app, KeyCode::Char('c'));
    assert!(
        app.status(Instant::now())
            .is_some_and(|status| status.text.contains("rack view"))
    );
}
