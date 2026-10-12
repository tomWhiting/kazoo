//! The console's tests: keys and the exact requests they produce, the
//! screen on ratatui's test backend, and the whole console against a real
//! headless daemon.

mod command;
mod keys;
mod live;
mod mouse;
mod probes;
mod rack;
mod render;
mod saves;

use std::collections::BTreeMap;
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use kazoo_wall::daemon::wall::catalogue;
use kazoo_wall::fingerprints::Fingerprints;
use kazoo_wall::protocol::{
    CableRecord, Change, ChangeResult, ClockSource, Faults, KnobView, Levels, Listen, LogPage,
    ModuleView, Request, Snapshot, What,
};

use super::app::App;
use super::knob::show;
use super::link::LinkState;
use super::worker::{Job, Reply};

/// A module of `kind` as the wall would show it, every knob at its
/// default.
pub fn module(id: &str, kind: &str, name: Option<&str>) -> ModuleView {
    let kinds = catalogue().kinds;
    let info = kinds
        .iter()
        .find(|info| info.kind == kind)
        .unwrap_or_else(|| panic!("the catalogue has no {kind}"));
    ModuleView {
        id: id.to_string(),
        kind: kind.to_string(),
        name: name.map(str::to_string),
        knobs: info
            .knobs
            .iter()
            .map(|knob| {
                let display = show(knob.default, &knob.unit, &knob.labels, knob.min);
                KnobView {
                    name: knob.name.clone(),
                    value: knob.default,
                    target: knob.default,
                    min: knob.min,
                    max: knob.max,
                    unit: knob.unit.clone(),
                    stepped: knob.stepped,
                    display: display.clone(),
                    target_display: display,
                }
            })
            .collect(),
        inputs: info.inputs.iter().map(|port| port.name.clone()).collect(),
        outputs: info.outputs.iter().map(|port| port.name.clone()).collect(),
    }
}

pub fn cable(id: u32, from: &str, to: &str, amount: f64) -> CableRecord {
    CableRecord {
        id,
        from: from.to_string(),
        to: to.to_string(),
        amount,
    }
}

/// A small wall: an oscillator and a named LFO into a filter (its cutoff
/// gliding from 420 Hz to 800 Hz), out to the master.
pub fn snapshot() -> Snapshot {
    let mut filter = module("vcf1", "vcf", None);
    let cutoff = filter
        .knobs
        .iter_mut()
        .find(|knob| knob.name == "cutoff")
        .expect("a filter has a cutoff");
    cutoff.value = 420.0;
    cutoff.display = "420 Hz".to_string();
    cutoff.target = 800.0;
    cutoff.target_display = "800 Hz".to_string();
    Snapshot {
        revision: 58,
        wall: 0,
        tempo: 96.0,
        beat: 45.5,
        clock: ClockSource::Own,
        on_desk: false,
        heard: true,
        seats: vec!["Tom".to_string(), "Waffles".to_string()],
        modules: vec![
            module("vco1", "vco", None),
            module("lfo1", "lfo", Some("slow wobble")),
            filter,
            module("out1", "out", None),
        ],
        cables: vec![
            cable(1, "vco1.out", "vcf1.in", 1.0),
            cable(2, "lfo1.out", "vcf1.cutoff", 0.4),
            cable(3, "vcf1.out", "out1.left", 1.0),
        ],
        levels: Levels {
            peak_l: -12.0,
            peak_r: -14.5,
        },
        listen: Some(Listen {
            at: "2026-09-26T12:00:01Z".to_string(),
            rms_db: -24.0,
            peak_db: -12.0,
            centroid_hz: 600.0,
            low: 0.5,
            mid: 0.4,
            high: 0.1,
            onsets_per_second: 1.5,
            pitch_hz: Some(110.0),
            pitch: Some("A2".to_string()),
            words: "dark, sparse, slow pulse around A2, quiet".to_string(),
        }),
        faults: Faults {
            count: 0,
            recent: Vec::new(),
        },
        fingerprints: Fingerprints {
            modules: BTreeMap::from([(
                "vcf1".to_string(),
                BTreeMap::from([("Tom".to_string(), 0.3), ("Waffles".to_string(), 0.7)]),
            )]),
            cables: BTreeMap::new(),
        },
        timing: kazoo_wall::protocol::Timings::default(),
        recording: None,
        rack: None,
        groups: std::collections::BTreeMap::new(),
        save: None,
        log: kazoo_wall::protocol::LogHealth::default(),
        probes: Vec::new(),
        engine: None,
    }
}

/// A logged change.
pub fn change(seq: u64, seat: &str, summary: &str, undoes: Option<u64>) -> Change {
    Change {
        seq,
        at: "2026-09-26T12:00:01Z".to_string(),
        seat: seat.to_string(),
        what: What::Turn {
            module: "vcf1".to_string(),
            knob: "cutoff".to_string(),
            from: 420.0,
            to: 800.0,
            glide_beats: 4.0,
        },
        summary: summary.to_string(),
        undoes,
    }
}

/// A console connected to the small wall, its log holding a change from
/// Waffles (57) and one from Tom (58), with the start-up jobs taken.
pub fn connected() -> App {
    let now = Instant::now();
    let mut app = App::new("Tom", "/tmp/kw/kazoo-wall.sock".to_string());
    app.on_reply(
        Reply::Link(LinkState::Connected {
            daemon: "kazoo-wall 0.1.0".to_string(),
        }),
        now,
    );
    app.on_reply(
        Reply::Feed(LinkState::Connected {
            daemon: "feed".to_string(),
        }),
        now,
    );
    let jobs = app.take_jobs();
    assert_eq!(
        jobs,
        vec![
            Job::Call(Request::Catalogue),
            Job::Call(Request::Log {
                before: None,
                limit: Some(100)
            })
        ]
    );
    answer(&mut app, Request::Catalogue, &catalogue());
    answer(
        &mut app,
        Request::Log {
            before: None,
            limit: Some(100),
        },
        &LogPage {
            changes: vec![
                change(
                    57,
                    "Waffles",
                    "Waffles turned vcf1 cutoff 420 Hz → 800 Hz over 4 beats",
                    None,
                ),
                change(
                    58,
                    "Tom",
                    "Tom patched lfo1.out → vcf1.cutoff (cable 2, amount 0.4)",
                    None,
                ),
            ],
            more: false,
        },
    );
    app.on_reply(Reply::Snapshot(Box::new(snapshot())), now);
    assert_eq!(
        app.take_jobs(),
        vec![Job::Wall(0)],
        "the log is up to date; the worker is told which wall is shown"
    );
    app
}

/// Answer `request` with `result`.
pub fn answer(app: &mut App, request: Request, result: &impl serde::Serialize) {
    app.on_reply(
        Reply::Answer {
            request,
            outcome: Ok(serde_json::to_value(result).expect("results encode")),
        },
        Instant::now(),
    );
}

/// Answer a change request with the change it made.
pub fn changed(app: &mut App, request: Request, change: Change, module: Option<&str>) {
    answer(
        app,
        request,
        &ChangeResult {
            change,
            module: module.map(str::to_string),
            cable: None,
        },
    );
}

/// Press `code`.
pub fn press(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE), Instant::now());
}

/// Press Ctrl and `c`.
pub fn ctrl(app: &mut App, c: char) {
    app.handle_key(
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL),
        Instant::now(),
    );
}

/// Type `text`, a key per character.
pub fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        press(app, KeyCode::Char(c));
    }
}

fn fold_fixture() -> App {
    let mut app = connected();
    let mut wall = snapshot();
    wall.groups.insert(
        "beat".to_string(),
        vec!["vco1".to_string(), "vcf1".to_string()],
    );
    wall.rack = Some(vec![vec![
        "lfo1".to_string(),
        "vcf1".to_string(),
        "vco1".to_string(),
        "out1".to_string(),
    ]]);
    app.on_reply(Reply::Snapshot(Box::new(wall)), Instant::now());
    press(&mut app, KeyCode::Char('v'));
    press(&mut app, KeyCode::Char('f'));
    app
}

fn fold_command(app: &mut App, command: &str) {
    press(app, KeyCode::Char(':'));
    type_text(app, command);
    press(app, KeyCode::Enter);
    press(app, KeyCode::Esc);
}

#[test]
fn folded_group_renders_one_tile_with_count() {
    let mut app = fold_fixture();
    let before = serde_json::to_value(app.snapshot()).expect("snapshot encodes");
    fold_command(&mut app, "fold beat");
    let screen = render::text(&render::render(&mut app, 240, 80));
    assert!(
        screen.contains("beat") && screen.contains("2 modules"),
        "missing group tile: {screen}"
    );
    assert!(
        !screen.contains("vco1") && !screen.contains("vcf1"),
        "members still drawn: {screen}"
    );
    assert_eq!(app.rack.layout.plates.len(), 3);
    for density in 0..3 {
        let screen = render::text(&render::render(&mut app, 240, 80));
        assert!(
            screen.contains("beat") && screen.contains("2 modules"),
            "density {density}"
        );
        assert!(!screen.contains("vco1") && !screen.contains("vcf1"));
        press(&mut app, KeyCode::Char('c'));
    }
    assert_eq!(
        serde_json::to_value(app.snapshot()).expect("snapshot encodes"),
        before
    );
    fold_command(&mut app, "unfold beat");
    let screen = render::text(&render::render(&mut app, 240, 80));
    assert!(screen.contains("vco1") && screen.contains("vcf1"));
    assert_eq!(
        serde_json::to_value(app.snapshot()).expect("snapshot encodes"),
        before
    );
}

#[test]
fn outside_cables_land_on_the_tile() {
    let mut app = fold_fixture();
    fold_command(&mut app, "fold beat");
    render::render(&mut app, 240, 80);
    assert!(
        !app.rack.paths.keys().any(|(id, ..)| *id == 1),
        "internal cable still drawn"
    );
    let tile = app
        .rack
        .layout
        .plate(2)
        .expect("tile at first member in rack order");
    for (id, from, to) in app.rack.paths.keys() {
        if *id == 2 {
            assert_eq!(to.x, tile.at.x);
            assert!(tile.contains(*to));
        } else if *id == 3 {
            assert_eq!(from.x, tile.at.x + u32::from(tile.width) - 1);
            assert!(tile.contains(*from));
        }
    }
    assert_eq!(app.rack.paths.len(), 2, "both outside cables drawn");
}

#[test]
fn cursor_skips_folded_members() {
    let mut app = fold_fixture();
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    fold_command(&mut app, "fold beat");
    render::render(&mut app, 240, 80);
    assert_eq!(
        app.selected_index(),
        Some(2),
        "cursor maps to the first member in rack order"
    );
    assert_eq!(app.knob_index(), 0, "a tile has no hidden knob cursor");
    press(&mut app, KeyCode::Right);
    assert_eq!(app.selected_index(), Some(3), "right skips hidden vco1");
    press(&mut app, KeyCode::Left);
    assert_eq!(app.selected_index(), Some(2));
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.focus(), super::app::Focus::Wall);
    render::render(&mut app, 240, 80);
    assert_eq!(app.rack.layout.plates.len(), 4, "Tab unfolds the tile");
    fold_command(&mut app, "fold");
    assert_eq!(
        app.rack.layout.plates.len(),
        3,
        "unnamed fold uses the cursor group"
    );
    assert!(
        app.take_jobs()
            .iter()
            .all(|job| matches!(job, Job::Call(Request::Saves)))
    );
    let before = serde_json::to_value(app.snapshot()).expect("snapshot encodes");
    for key in ['H', 'J', 'K', 'L', '=', 'x', 'p', 'u'] {
        press(&mut app, KeyCode::Char(key));
    }
    assert!(
        app.take_jobs().is_empty(),
        "a tile never edits a hidden member"
    );
    assert!(
        app.rack.moving.is_none(),
        "group moves are explicitly blocked"
    );
    assert_eq!(
        serde_json::to_value(app.snapshot()).expect("snapshot encodes"),
        before
    );
}

#[test]
fn fold_state_survives_a_restart() {
    let dir = tempfile::tempdir_in(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/kz-logs"))
        .expect("test folder");
    let file = dir.path().join("console.json");
    let mut app = fold_fixture();
    fold_command(&mut app, "fold beat");
    super::app::save_prefs(&file, &app.prefs()).expect("prefs saved");
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).expect("prefs read"))
            .expect("valid prefs");
    assert_eq!(saved["folded"], serde_json::json!(["beat"]));
    let mut fresh = App::new("Tom", String::new());
    fresh.apply_prefs(super::app::load_prefs(&file).expect("prefs loaded"));
    fresh.on_reply(
        Reply::Snapshot(Box::new(app.snapshot().expect("wall seen").clone())),
        Instant::now(),
    );
    render::render(&mut fresh, 240, 80);
    assert_eq!(fresh.rack.layout.plates.len(), 3);
    std::fs::write(
        &file,
        r#"{"rack":true,"density":"full","log_hidden":true,"picker_percent":100}"#,
    )
    .expect("old prefs");
    fresh.apply_prefs(super::app::load_prefs(&file).expect("old install loaded"));
    render::render(&mut fresh, 240, 80);
    assert_eq!(
        fresh.rack.layout.plates.len(),
        4,
        "old preferences migrate to unfolded"
    );
}
