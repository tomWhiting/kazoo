//! Scopes and readouts on the rack: their screens laid out on the
//! faceplate, drawn live from what `look` carries, at every size and
//! density.

use std::time::Instant;

use crossterm::event::KeyCode;
use ratatui::buffer::Buffer;
use ratatui::style::Color;

use kazoo_wall::protocol::{
    ErrorCode, GateMeasured, MAX_PROBES, Measured, Probe, ProbeResult, ReadoutReading, Request,
    Rhythm, ScopeReading, Snapshot,
};

use super::super::draw::{MIN_HEIGHT, MIN_WIDTH};
use super::super::rack::geometry::{self, Density, KNOB_ROWS, KNOB_W, READOUT_W, SCOPE_W};
use super::super::theme::{AMBER, METER_BG, RED, SAGE};
use super::super::worker::{Failure, Job, Reply};
use super::render::{render, text};
use super::{answer, cable, connected, module, press, snapshot};

fn quiet(connected: bool) -> Measured {
    Measured {
        connected,
        waiting: false,
        last: 0.0,
        low: 0.0,
        high: 0.0,
        mean: 0.0,
        rms_db: -120.0,
        peak_db: -120.0,
        hz: None,
        rhythm: Rhythm::None,
        shape: None,
        gate: GateMeasured::default(),
        min: None,
        max: None,
        faults: 0,
        faults_since_reset: 0,
    }
}

/// A scope's sweep: a sine on a, a slower one on b.
fn sweep() -> ScopeReading {
    let wave = |cycles: f64, amplitude: f64| -> Vec<[f64; 2]> {
        (0..96)
            .map(|point| {
                let value =
                    amplitude * (std::f64::consts::TAU * cycles * f64::from(point) / 96.0).sin();
                [value, value]
            })
            .collect()
    };
    ScopeReading {
        mode: "trigger".to_string(),
        span: 0.02,
        zoom: 1.0,
        trigger: 0.0,
        triggered: true,
        points: 96,
        sweeping: None,
        a: wave(4.0, 0.8),
        b: wave(1.0, 0.4),
        xy: Vec::new(),
        input_a: Measured {
            hz: Some(220.0),
            rhythm: Rhythm::Steady,
            ..quiet(true)
        },
        input_b: quiet(true),
        words: "a: 220 Hz sine".to_string(),
    }
}

fn reading(lines: &[&str]) -> ReadoutReading {
    ReadoutReading {
        mode: "pitch".to_string(),
        auto: true,
        input: quiet(true),
        note: Some("A4".to_string()),
        cents: Some(3.0),
        pitch_hz: Some(440.8),
        lines: lines.iter().map(|line| (*line).to_string()).collect(),
    }
}

fn probe(
    id: &str,
    kind: &str,
    scope: Option<ScopeReading>,
    readout: Option<ReadoutReading>,
) -> Probe {
    Probe {
        module: id.to_string(),
        kind: kind.to_string(),
        age: 0.05,
        scope,
        readout,
    }
}

/// A wall with a scope on the oscillator and a readout on the scope.
fn measuring() -> Snapshot {
    let mut wall = snapshot();
    wall.modules = vec![
        module("vco1", "vco", None),
        module("scope1", "scope", None),
        module("readout1", "readout", None),
    ];
    wall.cables = vec![
        cable(1, "vco1.out", "scope1.a", 1.0),
        cable(2, "scope1.a", "readout1.in", 1.0),
    ];
    wall.probes = vec![
        probe("scope1", "scope", Some(sweep()), None),
        probe(
            "readout1",
            "readout",
            None,
            Some(reading(&[
                "! 3 not numbers",
                "A4 +3¢",
                "441 Hz",
                "cv +0.753",
            ])),
        ),
    ];
    wall
}

/// The console showing `wall` in the rack view.
fn rack_of(wall: Snapshot) -> super::super::app::App {
    let mut app = connected();
    app.on_reply(Reply::Snapshot(Box::new(wall)), Instant::now());
    press(&mut app, KeyCode::Char('v'));
    app
}

/// Whether any cell on a meter's dark background shows braille dots in
/// `colour`.
fn traced_in(buf: &Buffer, colour: Color) -> bool {
    buf.content().iter().any(|cell| {
        cell.fg == colour
            && cell.bg == METER_BG
            && cell
                .symbol()
                .chars()
                .next()
                .is_some_and(|ch| ('\u{2801}'..='\u{28ff}').contains(&ch))
    })
}

/// Where `needle` first appears on screen, and its first cell.
fn cell_of<'a>(buf: &'a Buffer, needle: &str) -> Option<&'a ratatui::buffer::Cell> {
    let screen = text(buf);
    for (y, row) in screen.lines().enumerate() {
        if let Some(byte) = row.find(needle) {
            let x = row[..byte].chars().count();
            let x = u16::try_from(x).expect("fits");
            let y = u16::try_from(y).expect("fits");
            return Some(&buf[(x, y)]);
        }
    }
    None
}

#[test]
fn a_scope_and_a_readout_have_screens_beside_their_knobs() {
    let scope = module("scope1", "scope", None);
    let (width, _) = geometry::plate_size(&scope, Density::Full);
    let columns = u16::try_from(scope.knobs.len())
        .expect("few knobs")
        .div_ceil(KNOB_ROWS);
    assert_eq!(width, columns * KNOB_W + SCOPE_W + 2);
    let readout = module("readout1", "readout", None);
    let (width, _) = geometry::plate_size(&readout, Density::Full);
    assert_eq!(width, KNOB_W + READOUT_W + 2);
    let wall = measuring();
    let rack = geometry::rack(&wall.modules, Density::Full, (400, 60), None);
    let plate = rack.plate(1).expect("the scope");
    let screen = plate.screen.expect("a scope has a screen");
    assert_eq!(screen.x, plate.at.x + 1 + u32::from(columns * KNOB_W));
    assert!(rack.plate(0).expect("the vco").screen.is_none());
    // The overview's blocks have no room for one.
    let overview = geometry::rack(&wall.modules, Density::Overview, (400, 60), None);
    assert!(overview.plates.iter().all(|plate| plate.screen.is_none()));
}

#[test]
fn the_rack_draws_a_live_trace_and_a_readout_s_numbers() {
    let mut app = rack_of(measuring());
    let buf = render(&mut app, 160, 60);
    let screen = text(&buf);
    for expected in [
        "trig'd 20 ms ×1 · 220 Hz",
        "A4 +3¢",
        "441 Hz",
        "pitch · auto",
    ] {
        assert!(
            screen.contains(expected),
            "missing '{expected}' in\n{screen}"
        );
    }
    // Input a in green, b in amber, on the dark screen.
    assert!(traced_in(&buf, SAGE), "{screen}");
    assert!(traced_in(&buf, AMBER), "{screen}");
    // A fault reads in red; the first line of numbers stands out.
    let fault = cell_of(&buf, "! 3 not numbers").expect("the fault");
    assert_eq!((fault.fg, fault.bg), (RED, METER_BG));
    // No cable is drawn across a screen: its blank cells are blank braille.
    let label = cell_of(&buf, "trig'd").expect("the label");
    assert_eq!(label.bg, METER_BG);
}

#[test]
fn a_screen_says_why_it_is_empty() {
    // Nothing measured yet.
    let mut waiting = measuring();
    waiting.probes = vec![
        probe("scope1", "scope", None, None),
        probe("readout1", "readout", None, None),
    ];
    let mut app = rack_of(waiting);
    let screen = text(&render(&mut app, 160, 60));
    assert_eq!(screen.matches("waiting…").count(), 2, "{screen}");
    // A wall older than probes sends none.
    let mut older = measuring();
    older.probes.clear();
    let mut app = rack_of(older);
    let screen = text(&render(&mut app, 160, 60));
    assert!(screen.contains("no readings"), "{screen}");
    // Past the first sixteen, the wall sends no readings.
    let mut crowded = measuring();
    crowded.modules = (1..=MAX_PROBES + 1)
        .map(|n| module(&format!("readout{n}"), "readout", None))
        .collect();
    crowded.cables.clear();
    crowded.probes = (1..=MAX_PROBES)
        .map(|n| {
            probe(
                &format!("readout{n}"),
                "readout",
                None,
                Some(reading(&["0 Hz"])),
            )
        })
        .collect();
    let mut app = rack_of(crowded);
    let screen = text(&render(&mut app, 260, 120));
    assert!(screen.contains("reading…"), "{screen}");
}

#[test]
fn screens_in_view_past_the_sixteen_look_carries_are_read_with_probe() {
    let mut crowded = measuring();
    crowded.modules = (1..=MAX_PROBES + 2)
        .map(|n| module(&format!("readout{n}"), "readout", None))
        .collect();
    crowded.cables.clear();
    crowded.probes = (1..=MAX_PROBES)
        .map(|n| {
            probe(
                &format!("readout{n}"),
                "readout",
                None,
                Some(reading(&["0 Hz"])),
            )
        })
        .collect();
    let mut app = rack_of(crowded);
    render(&mut app, 260, 120);
    app.take_jobs();
    let now = Instant::now();
    app.tick(now);
    let extra: Vec<String> = (MAX_PROBES + 1..=MAX_PROBES + 2)
        .map(|n| format!("readout{n}"))
        .collect();
    let asked = Request::Probe {
        module: None,
        modules: extra.clone(),
    };
    assert_eq!(app.take_jobs(), vec![Job::Call(asked.clone())]);
    // One at a time, at most ten a second.
    app.tick(now);
    assert!(app.take_jobs().is_empty());
    answer(
        &mut app,
        asked.clone(),
        &ProbeResult {
            probes: vec![probe(
                &extra[0],
                "readout",
                None,
                Some(reading(&["7 Hz extra"])),
            )],
            left_out: Vec::new(),
            missing: Vec::new(),
            dropped: 0,
        },
    );
    let screen = text(&render(&mut app, 260, 120));
    assert!(screen.contains("7 Hz extra"), "{screen}");
    app.tick(now);
    assert!(app.take_jobs().is_empty(), "not before the next look");
    app.tick(now + super::super::app::PROBE_EVERY);
    assert_eq!(app.take_jobs(), vec![Job::Call(asked.clone())]);
    // Refused (one was just taken away): asked again next time, quietly.
    app.on_reply(
        Reply::Answer {
            request: asked,
            outcome: Err(Failure {
                code: Some(ErrorCode::UnknownModule),
                message: "gone".to_string(),
            }),
        },
        now,
    );
    assert!(app.rack.readings.is_empty());
    assert!(
        app.status(now)
            .is_none_or(|status| !status.text.contains("gone")),
        "no status line for it"
    );
    // Nothing past the sixteen in view: nothing is asked.
    let mut app = rack_of(measuring());
    render(&mut app, 260, 120);
    app.take_jobs();
    app.tick(now);
    assert!(app.take_jobs().is_empty());
}

#[test]
fn xy_and_rolling_screens_draw_too() {
    let mut wall = measuring();
    let mut figure = sweep();
    figure.mode = "xy".to_string();
    figure.xy = (0..96)
        .map(|step| {
            let angle = std::f64::consts::TAU * f64::from(step) / 96.0;
            [0.7 * angle.cos(), 0.7 * angle.sin()]
        })
        .collect();
    figure.a.clear();
    figure.b.clear();
    wall.probes[0].scope = Some(figure);
    let mut app = rack_of(wall.clone());
    let buf = render(&mut app, 160, 60);
    assert!(text(&buf).contains("xy 20 ms ×1"));
    assert!(traced_in(&buf, SAGE));
    let mut rolling = sweep();
    rolling.mode = "free".to_string();
    rolling.a.truncate(10);
    rolling.b.clear();
    wall.probes[0].scope = Some(rolling);
    let mut app = rack_of(wall);
    let buf = render(&mut app, 160, 60);
    assert!(text(&buf).contains("roll 20 ms ×1"));
    assert!(traced_in(&buf, SAGE));
    assert!(!traced_in(&buf, AMBER), "b is unplugged");
}

#[test]
fn screens_draw_at_every_size_and_density() {
    let mut app = rack_of(measuring());
    for _ in 0..3 {
        for width in [MIN_WIDTH, 100, 133, 180, 260] {
            for height in [MIN_HEIGHT, 30, 50, 120] {
                let screen = text(&render(&mut app, width, height));
                assert!(screen.contains("THE WALL"), "{width}×{height}");
            }
        }
        press(&mut app, KeyCode::Char('c'));
    }
    // Compact still shows the trace and the numbers.
    press(&mut app, KeyCode::Char('c'));
    let buf = render(&mut app, 160, 50);
    let screen = text(&buf);
    assert!(screen.contains("A4 +3¢"), "{screen}");
    assert!(traced_in(&buf, SAGE), "{screen}");
    // And the list view is not troubled by them.
    press(&mut app, KeyCode::Char('v'));
    let screen = text(&render(&mut app, 120, 40));
    assert!(screen.contains("scope1"), "{screen}");
}
