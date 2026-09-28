//! The console's tests: keys and the exact requests they produce, the
//! screen on ratatui's test backend, and the whole console against a real
//! headless daemon.

mod keys;
mod live;
mod mouse;
mod rack;
mod render;

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
    assert!(app.take_jobs().is_empty(), "the log is up to date");
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

/// Type `text`, a key per character.
pub fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        press(app, KeyCode::Char(c));
    }
}
