//! A fire frame: when the next is due, and what it draws again.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::blaze::{self, Beat};
use crate::app::{App, Grown, PANEL};
use crate::test_support::grown;

#[test]
fn a_slow_fire_frame_is_never_followed_at_once() {
    let start = Instant::now();
    let period = blaze::period();
    // A frame that takes 20 ms, more than two periods: the next waits a
    // period from when it finished, never drawn at once.
    let mut beat = Beat::new(start);
    assert!(beat.due(start));
    let finished = start + Duration::from_millis(20);
    beat.drawn(finished);
    assert!(!beat.due(finished), "drawn again at once");
    assert_eq!(beat.wait(finished), period);
    // A quick frame keeps the beat.
    let mut beat = Beat::new(start);
    beat.drawn(start + Duration::from_millis(2));
    assert_eq!(beat.wait(start), period);
}

fn app() -> App {
    let history = grown(200, &[(0, 50, 60), (0, 120, 40)]);
    let mut app = App::showing(PathBuf::from("/nonexistent"), Grown::of(history));
    app.said.clear();
    app
}

fn frame(app: &mut App, area: Rect) -> Buffer {
    let mut buf = Buffer::empty(area);
    crate::frame_of(app, area, &mut buf);
    buf
}

/// The cells of the panel and the footer: everything but the tree.
fn chrome(buf: &Buffer) -> Vec<ratatui::buffer::Cell> {
    let area = buf.area;
    let panel_x = area.width - PANEL;
    let mut cells = Vec::new();
    for y in 0..area.height {
        for x in 0..area.width {
            if y + 1 == area.height || x >= panel_x {
                cells.push(buf[(x, y)].clone());
            }
        }
    }
    cells
}

#[test]
fn a_fire_only_frame_keeps_the_panel_and_footer_without_working_them_out() {
    let area = Rect::new(0, 0, 140, 40);
    let mut app = app();
    frame(&mut app, area);
    for _ in 0..60 {
        app.fire.burn(1.0 / 60.0);
    }
    // Two tries: a frame either side of a second's turn draws afresh.
    let mut kept = false;
    for _ in 0..2 {
        app.said.clear();
        let full = frame(&mut app, area);
        // Something the footer shows changes without a frame of its own:
        // a fire-only frame must not have worked the footer out again.
        "a word said since".clone_into(&mut app.said);
        app.fire.fire_only();
        let only = frame(&mut app, area);
        if app.fire.reused() {
            assert_eq!(
                chrome(&only),
                chrome(&full),
                "the panel or footer was redrawn"
            );
            kept = true;
            break;
        }
    }
    assert!(kept, "the tree's picture was never kept");
    // A frame that is not fire-only shows it.
    let full = frame(&mut app, area);
    let footer: String = (0..area.width)
        .map(|x| full[(x, area.height - 1)].symbol())
        .collect();
    assert!(footer.contains("a word said since"), "{footer}");
}
