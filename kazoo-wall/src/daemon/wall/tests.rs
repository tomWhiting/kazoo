//! Wall tests: requests, the flood guard, undo, the log, the snapshot and
//! faults.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use super::*;
use crate::SUB_BLOCK;
use crate::dsp::{Io, Module as DspModule, Tick};
use crate::engine::{Engine, EngineConfig, engine};
use crate::protocol::{ChangeResult, Snapshot};
use crate::seed::seed;

fn wall() -> (Wall, Engine) {
    let (engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    (
        Wall::new(seed().unwrap(), control, None, Vec::new(), None),
        engine,
    )
}

fn ask(wall: &mut Wall, request: &Request) -> Result<serde_json::Value, WallError> {
    wall.request("Tom", request, Instant::now())
        .map(|outcome| outcome.result)
}

fn code(result: Result<serde_json::Value, WallError>) -> ErrorCode {
    result.unwrap_err().code
}

fn turn(module: &str, knob: &str, value: f64) -> Request {
    Request::Turn {
        module: module.to_string(),
        knob: knob.to_string(),
        value,
        glide_beats: Some(0.0),
    }
}

fn changed(value: serde_json::Value) -> ChangeResult {
    serde_json::from_value(value).unwrap()
}

#[test]
fn the_flood_guard_allows_thirty_changes_per_ten_seconds() {
    let start = Instant::now();
    let mut bucket = Bucket::new(start);
    for _ in 0..30 {
        assert!(bucket.take(start));
    }
    assert!(!bucket.take(start));
    // A third of a second earns one change back.
    assert!(bucket.take(start + Duration::from_millis(340)));
    assert!(!bucket.take(start + Duration::from_millis(340)));
    // Ten seconds fill it again, and no more.
    let later = start + Duration::from_secs(60);
    for _ in 0..30 {
        assert!(bucket.take(later));
    }
    assert!(!bucket.take(later));
}

#[test]
fn a_flooding_seat_is_told_to_slow_down_and_others_are_not() {
    let (mut wall, _engine) = wall();
    let now = Instant::now();
    for step in 0..30 {
        let request = turn("vcf1", "cutoff", 400.0 + f64::from(step));
        assert!(wall.request("Loop", &request, now).is_ok());
    }
    let refused = wall
        .request("Loop", &turn("vcf1", "cutoff", 1.0), now)
        .unwrap_err();
    assert_eq!(refused.code, ErrorCode::SlowDown);
    // Looking is not a change.
    assert!(wall.request("Loop", &Request::Look, now).is_ok());
    assert!(
        wall.request("Tom", &turn("vcf1", "cutoff", 1.0), now)
            .is_ok()
    );
}

#[test]
fn every_wall_error_code_is_reachable() {
    let (mut wall, _engine) = wall();
    assert_eq!(
        code(ask(
            &mut wall,
            &Request::Unpatch {
                cable: Some(1),
                to: Some("vcf1.in".to_string())
            }
        )),
        ErrorCode::BadRequest
    );
    assert_eq!(
        code(ask(
            &mut wall,
            &Request::Add {
                kind: "lfo".to_string(),
                name: Some("bad/name".to_string()),
                place: None,
            }
        )),
        ErrorCode::BadName
    );
    assert_eq!(
        code(ask(&mut wall, &turn("vcf9", "cutoff", 1.0))),
        ErrorCode::UnknownModule
    );
    assert_eq!(
        code(ask(&mut wall, &turn("vcf1", "cutof", 1.0))),
        ErrorCode::UnknownKnob
    );
    assert_eq!(
        code(ask(
            &mut wall,
            &Request::Patch {
                from: "vcf1.nope".to_string(),
                to: "vco1.pitch".to_string(),
                amount: None
            }
        )),
        ErrorCode::UnknownPort
    );
    assert_eq!(
        code(ask(
            &mut wall,
            &Request::Add {
                kind: "theremin".to_string(),
                name: None,
                place: None,
            }
        )),
        ErrorCode::UnknownKind
    );
    assert_eq!(
        code(ask(
            &mut wall,
            &Request::Unpatch {
                cable: Some(999),
                to: None
            }
        )),
        ErrorCode::UnknownCable
    );
    assert_eq!(
        code(ask(&mut wall, &Request::Undo { change: 999 })),
        ErrorCode::UnknownChange
    );
}

#[test]
fn undo_that_no_longer_applies_and_caps_are_errors() {
    let (mut wall, _engine) = wall();
    let added = changed(
        ask(
            &mut wall,
            &Request::Add {
                kind: "sh".to_string(),
                name: None,
                place: None,
            },
        )
        .unwrap(),
    );
    ask(
        &mut wall,
        &Request::Undo {
            change: added.change.seq,
        },
    )
    .unwrap();
    // Undoing the add twice: the module is gone already.
    assert_eq!(
        code(ask(
            &mut wall,
            &Request::Undo {
                change: added.change.seq
            }
        )),
        ErrorCode::UnknownModule
    );
    let removed = changed(
        ask(
            &mut wall,
            &Request::Remove {
                module: "lfo2".to_string(),
            },
        )
        .unwrap(),
    );
    ask(
        &mut wall,
        &Request::Undo {
            change: removed.change.seq,
        },
    )
    .unwrap();
    assert_eq!(
        code(ask(
            &mut wall,
            &Request::Undo {
                change: removed.change.seq
            }
        )),
        ErrorCode::NotAllowed
    );
    assert_eq!(
        code(ask(&mut wall, &Request::Shutdown)),
        ErrorCode::Internal
    );
    // Filled by many seats, each within its own flood allowance.
    for count in wall.patch().modules().len()..crate::MAX_MODULES {
        let add = Request::Add {
            kind: "mix".to_string(),
            name: None,
            place: None,
        };
        wall.request(&format!("Filler{count}"), &add, Instant::now())
            .unwrap();
    }
    assert_eq!(
        code(ask(
            &mut wall,
            &Request::Add {
                kind: "mix".to_string(),
                name: None,
                place: None,
            }
        )),
        ErrorCode::Full
    );
}

#[test]
fn changes_are_numbered_summarised_and_undone() {
    let (mut wall, _engine) = wall();
    let first = changed(ask(&mut wall, &turn("vcf1", "cutoff", 420.0)).unwrap());
    assert_eq!(first.change.seq, 1);
    assert_eq!(
        first.change.summary,
        "Tom turned vcf1 cutoff 900 Hz → 420 Hz at once"
    );
    let added = changed(
        ask(
            &mut wall,
            &Request::Add {
                kind: "lfo".to_string(),
                name: Some("slow wobble".to_string()),
                place: None,
            },
        )
        .unwrap(),
    );
    assert_eq!(added.module.as_deref(), Some("lfo3"));
    let patched = changed(
        ask(
            &mut wall,
            &Request::Patch {
                from: "lfo3.out".to_string(),
                to: "vcf1.drive".to_string(),
                amount: Some(0.4),
            },
        )
        .unwrap(),
    );
    assert!(patched.cable.is_some());
    let undone = changed(ask(&mut wall, &Request::Undo { change: 1 }).unwrap());
    assert_eq!(undone.change.undoes, Some(1));
    assert_eq!(
        undone.change.summary,
        "Tom undid change 1: turned vcf1 cutoff 420 Hz → 900 Hz at once"
    );
    assert_eq!(wall.revision(), 4);
    let page: LogPage = serde_json::from_value(
        ask(
            &mut wall,
            &Request::Log {
                before: Some(4),
                limit: Some(2),
            },
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        page.changes.iter().map(|c| c.seq).collect::<Vec<_>>(),
        vec![2, 3]
    );
    assert!(page.more);
    let page: LogPage = serde_json::from_value(
        ask(
            &mut wall,
            &Request::Log {
                before: None,
                limit: None,
            },
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(page.changes.len(), 4);
    assert!(!page.more);
}

#[test]
fn tempo_changes_the_own_clock_and_undoes() {
    let (mut wall, mut engine) = wall();
    let set: TempoResult =
        serde_json::from_value(ask(&mut wall, &Request::Tempo { bpm: 1_000.0 }).unwrap()).unwrap();
    assert!((set.bpm - 300.0).abs() < f64::EPSILON);
    assert!(!set.desk);
    let mut buffer = vec![0.0; 64];
    engine.render(&mut buffer, 2);
    assert!((wall.snapshot().tempo - 300.0).abs() < f64::EPSILON);
    let undone = changed(
        ask(
            &mut wall,
            &Request::Undo {
                change: set.change.seq,
            },
        )
        .unwrap(),
    );
    assert!(matches!(undone.change.what, What::Tempo { to, .. } if (to - 120.0).abs() < 1e-9));
    engine.render(&mut buffer, 2);
    assert!((wall.snapshot().tempo - 120.0).abs() < f64::EPSILON);
    assert_eq!(
        code(ask(&mut wall, &Request::Tempo { bpm: f64::NAN })),
        ErrorCode::BadRequest
    );
}

#[test]
fn the_snapshot_shows_values_on_their_way() {
    let (mut wall, mut engine) = wall();
    ask(
        &mut wall,
        &Request::Turn {
            module: "vcf1".to_string(),
            knob: "cutoff".to_string(),
            value: 1_900.0,
            glide_beats: Some(2.0),
        },
    )
    .unwrap();
    // Half the glide: one beat at 120 BPM.
    let mut buffer = vec![0.0; 24_000 * 2];
    engine.render(&mut buffer, 2);
    let look: Snapshot = serde_json::from_value(ask(&mut wall, &Request::Look).unwrap()).unwrap();
    let vcf = look.modules.iter().find(|m| m.id == "vcf1").unwrap();
    let cutoff = &vcf.knobs[0];
    assert!((cutoff.target - 1_900.0).abs() < 1e-3);
    // Half way along a logarithmic knob is the geometric middle.
    assert!(
        (cutoff.value - (900.0_f64 * 1_900.0).sqrt()).abs() < 20.0,
        "{cutoff:?}"
    );
    assert_eq!(cutoff.target_display, "1.9 kHz");
    assert_eq!(vcf.inputs, vec!["in"]);
    assert_eq!(look.clock, ClockSource::Own);
    let seeded = seed().unwrap();
    assert_eq!(look.modules.len(), seeded.modules().len());
    assert_eq!(look.cables.len(), seeded.cables().len());
    assert!(look.levels.peak_l <= 0.0);
    let catalogue: CatalogueResult =
        serde_json::from_value(ask(&mut wall, &Request::Catalogue).unwrap()).unwrap();
    assert_eq!(catalogue.kinds.len(), Kind::all().count());
    assert!(
        catalogue
            .kinds
            .iter()
            .any(|k| k.kind == "testgain" && k.family == "fx")
    );
    let clock = catalogue.kinds.iter().find(|k| k.kind == "clock").unwrap();
    assert_eq!(clock.knobs[0].labels.len(), 7);
}

#[test]
fn seats_come_and_go_by_their_last_connection() {
    let (mut wall, _engine) = wall();
    assert!(wall.seat_joined("Tom"));
    assert!(!wall.seat_joined("Tom"));
    assert!(wall.seat_joined("Waffles"));
    assert_eq!(wall.seats(), vec!["Tom", "Waffles"]);
    assert!(!wall.seat_left("Tom"));
    assert!(wall.seat_left("Tom"));
    assert!(!wall.seat_left("Tom"));
    assert_eq!(wall.seats(), vec!["Waffles"]);
}

/// A module that turns out NaN, counting its resets.
#[derive(Debug)]
struct Broken(Arc<AtomicU32>);

impl DspModule for Broken {
    fn process(&mut self, _tick: &Tick, io: Io<'_>) {
        io.outputs[0] = [f32::NAN; SUB_BLOCK];
    }

    fn reset(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn faults_are_counted_and_reported_at_most_every_five_seconds() {
    let (mut wall, mut engine) = wall();
    let slot = wall.rack.slot_of("vco1").unwrap();
    let (_, tag) = wall.rack.slots[slot].clone().unwrap();
    let resets = Arc::new(AtomicU32::new(0));
    wall.engine_mut().send(Command::Insert {
        slot,
        tag,
        kind: Kind::VCO,
        module: Box::new(Broken(Arc::clone(&resets))),
        knobs: [0.0; MAX_KNOBS],
    });
    let mut buffer = vec![0.0; SUB_BLOCK * 2 * 4];
    engine.render(&mut buffer, 2);
    assert!(buffer.iter().all(|s| s.is_finite()));
    let now = Instant::now();
    let events = wall.tick(now);
    assert_eq!(
        events,
        vec![Event::Fault {
            summary: "vco1 produced NaN 4 times; reset each time".to_string(),
            seq: Some(0),
        }]
    );
    engine.render(&mut buffer, 2);
    assert!(wall.tick(now + Duration::from_secs(1)).is_empty());
    let events = wall.tick(now + FAULT_REPORT_INTERVAL);
    assert_eq!(events.len(), 1);
    let look = wall.snapshot();
    assert_eq!(look.faults.count, 8);
    assert_eq!(look.faults.recent.len(), 2);
    assert_eq!(resets.load(Ordering::Relaxed), 8);
}

#[test]
fn the_patch_is_saved_when_due_and_the_log_keeps_every_change() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let (_engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let mut wall = Wall::new(seed().unwrap(), control, None, Vec::new(), Some(store));
    let now = Instant::now();
    wall.request("Tom", &turn("vcf1", "cutoff", 333.0), now)
        .unwrap();
    wall.tick(now);
    let saved = Store::open(dir.path()).unwrap();
    let crate::store::Loaded::Patch { patch, .. } = saved.load_patch().unwrap() else {
        panic!("not saved");
    };
    assert!((patch.module("vcf1").unwrap().knobs[0] - 333.0).abs() < 1e-3);
    assert_eq!(patch.last_seq, 1);
    // A second change within the second waits for the next save.
    wall.request("Tom", &turn("vcf1", "cutoff", 444.0), now)
        .unwrap();
    wall.tick(now + Duration::from_millis(100));
    let crate::store::Loaded::Patch { patch, .. } = saved.load_patch().unwrap() else {
        panic!("not saved");
    };
    assert!((patch.module("vcf1").unwrap().knobs[0] - 333.0).abs() < 1e-3);
    wall.tick(now + SAVE_INTERVAL);
    let crate::store::Loaded::Patch { patch, .. } = saved.load_patch().unwrap() else {
        panic!("not saved");
    };
    assert!((patch.module("vcf1").unwrap().knobs[0] - 444.0).abs() < 1e-3);
    assert_eq!(saved.load_log().unwrap().changes.len(), 2);
    // A wall remembering that history carries on the numbering.
    let (_engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let history = saved.load_log().unwrap().changes;
    let wall = Wall::new(*patch, control, None, history, None);
    assert_eq!(wall.revision(), 2);
}

#[test]
fn a_tempo_is_saved_before_the_engine_hears_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let (_engine, control) = engine(EngineConfig::new(48_000, 92.0, 0.0), None);
    let mut wall = Wall::new(seed().unwrap(), control, None, Vec::new(), Some(store));
    // The engine never renders here: the tempo must be saved regardless.
    ask(&mut wall, &Request::Tempo { bpm: 100.0 }).unwrap();
    wall.save(Instant::now());
    let saved = Store::open(dir.path()).unwrap();
    let crate::store::Loaded::Patch { patch, .. } = saved.load_patch().unwrap() else {
        panic!("not saved");
    };
    assert!((patch.tempo - 100.0).abs() < 1e-9);
}

#[test]
fn amounts_and_labels_read_as_people_wrote_them() {
    let (mut wall, _engine) = wall();
    ask(
        &mut wall,
        &Request::Patch {
            from: "lfo1.out".to_string(),
            to: "vcf1.drive".to_string(),
            amount: Some(0.35),
        },
    )
    .unwrap();
    let text = serde_json::to_string(&ask(&mut wall, &Request::Look).unwrap()).unwrap();
    assert!(text.contains("\"amount\":0.35"), "amount not clean");
    assert!(!text.contains("0.3499999"), "f32 noise in the snapshot");
    let look = wall.snapshot();
    let knob = |module: &str, name: &str| {
        look.modules
            .iter()
            .find(|m| m.id == module)
            .and_then(|m| m.knobs.iter().find(|k| k.name == name))
            .cloned()
            .unwrap()
    };
    for (module, name, label) in [
        ("vco1", "shape", "saw"),
        ("clock1", "division", "1/8"),
        ("quant1", "scale", "pentatonic minor"),
        ("quant1", "root", "A"),
    ] {
        let view = knob(module, name);
        assert_eq!(view.unit, "", "{module}.{name}");
        assert_eq!(view.target_display, label, "{module}.{name}");
    }
    assert_eq!(knob("vcf1", "cutoff").unit, "Hz");
}

#[test]
fn a_change_moves_the_fingerprints_and_says_so_with_its_seq() {
    let (mut wall, mut engine) = wall();
    assert!(wall.snapshot().fingerprints.is_empty());
    let outcome = wall
        .request("Tom", &turn("lfo1", "depth", 0.0), Instant::now())
        .unwrap();
    let seq = outcome.change.as_ref().unwrap().seq;
    let Some(Event::Fingerprints {
        seq: moved_seq,
        modules,
        cables,
    }) = outcome.event
    else {
        panic!("no fingerprints event");
    };
    assert_eq!(moved_seq, seq);
    // lfo1 sweeps the filter's cutoff: the dye reaches it and beyond.
    assert!(modules["lfo1"]["Tom"] > 0.999);
    assert!(modules.contains_key("vcf1") && modules.contains_key("vca1"));
    assert!(cables.values().all(|shares| shares.contains_key("Tom")));
    // The event carries exactly what the snapshot now shows for them.
    let look = wall.snapshot();
    for (id, shares) in &modules {
        assert_eq!(look.fingerprints.modules.get(id), Some(shares), "{id}");
    }

    // Time passes, the engine plays, housekeeping runs: nothing moves.
    let mut buffer = vec![0.0; 48_000 * 2];
    for step in 0..5 {
        engine.render(&mut buffer, 2);
        wall.tick(Instant::now() + Duration::from_secs(step * 10));
    }
    assert_eq!(wall.snapshot().fingerprints, look.fingerprints);

    // A change that moves no dye sends no fingerprints event.
    let again = wall
        .request("Tom", &turn("lfo1", "depth", 0.0), Instant::now())
        .unwrap();
    assert!(again.change.is_some() && again.event.is_none());
    let tempo = wall
        .request("Tom", &Request::Tempo { bpm: 99.0 }, Instant::now())
        .unwrap();
    assert!(tempo.event.is_none());

    // Another seat's touch mixes in.
    let outcome = wall
        .request("Waffles", &turn("vcf1", "cutoff", 18_000.0), Instant::now())
        .unwrap();
    let Some(Event::Fingerprints { modules, .. }) = outcome.event else {
        panic!("no fingerprints event");
    };
    assert!(!modules.contains_key("lfo1"), "upstream is untouched");
    let vcf = &modules["vcf1"];
    assert!(vcf["Tom"] > 0.0 && vcf["Waffles"] > 0.0);
    assert!((vcf.values().sum::<f64>() - 1.0).abs() < 1e-5);
}

#[test]
fn fingerprints_survive_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (_engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let store = Store::open(dir.path()).unwrap();
    let mut wall = Wall::new(seed().unwrap(), control, None, Vec::new(), Some(store));
    wall.request("Tom", &turn("vco1", "tune", 3.0), Instant::now())
        .unwrap();
    wall.request(
        "Cassio",
        &Request::Add {
            kind: "noise".to_string(),
            name: None,
            place: None,
        },
        Instant::now(),
    )
    .unwrap();
    let before = wall.snapshot().fingerprints;
    wall.save(Instant::now());
    let saved = Store::open(dir.path()).unwrap();
    let crate::store::Loaded::Patch { patch, .. } = saved.load_patch().unwrap() else {
        panic!("not saved");
    };
    let (_engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let wall = Wall::new(*patch, control, None, Vec::new(), None);
    assert_eq!(wall.snapshot().fingerprints, before);
    assert_eq!(
        before.modules["noise1"],
        std::iter::once(("Cassio".to_string(), 1.0)).collect()
    );
}

#[test]
fn a_dry_path_is_held_back_to_meet_a_drive() {
    let (_engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let mut wall = Wall::new(Patch::empty(120.0), control, None, Vec::new(), None);
    let now = Instant::now();
    for kind in ["vco", "klon", "mix"] {
        let add = Request::Add {
            kind: kind.to_string(),
            name: None,
            place: None,
        };
        wall.request("Tom", &add, now).unwrap();
    }
    let plug = |wall: &mut Wall, from: &str, to: &str| {
        let patch = Request::Patch {
            from: from.to_string(),
            to: to.to_string(),
            amount: None,
        };
        let outcome = wall.request("Tom", &patch, now).unwrap();
        outcome.change.map(|change| match change.what {
            What::Patch { cable, .. } => cable.id,
            other => panic!("{other:?}"),
        })
    };
    plug(&mut wall, "vco1.out", "klon1.left");
    let wet = plug(&mut wall, "klon1.left", "mix1.a").unwrap();
    let dry = plug(&mut wall, "vco1.out", "mix1.b").unwrap();
    let timing = wall.snapshot().timing;
    assert_eq!(timing.sample_rate, 48_000);
    let klon = timing.modules["klon1"];
    assert!(klon.latency_frames > 0, "the drive reports no latency");
    assert!((klon.latency_ms - f64::from(klon.latency_frames) / 48.0).abs() < 1e-9);
    assert_eq!(timing.modules["mix1"].arrival_frames, klon.latency_frames);
    // The dry cable waits exactly as long as the drive; the wet one not at all.
    assert_eq!(timing.cables[&dry.to_string()], klon.latency_frames);
    assert!(!timing.cables.contains_key(&wet.to_string()));
    assert!(timing.uncompensated.is_empty());

    // Unplug the wet path: nothing needs holding back any more.
    let unpatch = Request::Unpatch {
        cable: Some(wet),
        to: None,
    };
    wall.request("Tom", &unpatch, now).unwrap();
    let timing = wall.snapshot().timing;
    assert!(timing.cables.is_empty());
    assert!(!timing.modules.contains_key("mix1"));
    // The drive still says what it costs.
    assert_eq!(timing.modules["klon1"].latency_frames, klon.latency_frames);
}

/// The flanger's own latency at `rate` with its `mode` knob at `mode`, as
/// `look` shows it (nothing when it has none).
fn flanger_at(mode: f32, rate: f32) -> Option<u32> {
    let kind = kazoo_fx::catalogue()
        .find(|kind| kind.id == "flanger")
        .unwrap();
    let mut flanger = (kind.build)();
    flanger.prepare(rate);
    let index = kind
        .params
        .iter()
        .position(|param| param.name == "mode")
        .unwrap();
    flanger.set_param(index, mode);
    let latency = u32::try_from(flanger.latency()).unwrap();
    (latency > 0).then_some(latency)
}

/// The flanger's own latency at 48 kHz: classic, then through-zero.
fn flanger_modes() -> (Option<u32>, Option<u32>) {
    let modes = (flanger_at(0.0, 48_000.0), flanger_at(1.0, 48_000.0));
    assert!(
        modes.1.unwrap_or(0) > modes.0.unwrap_or(0),
        "through-zero holds the dry path back further: {modes:?}"
    );
    modes
}

/// A wall of `kinds`, cabled `cables`; returns it and each cable's id.
fn built(kinds: &[&str], cables: &[(&str, &str)], now: Instant) -> (Wall, Vec<u32>) {
    let (_engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let mut wall = Wall::new(Patch::empty(120.0), control, None, Vec::new(), None);
    for kind in kinds {
        let add = Request::Add {
            kind: (*kind).to_string(),
            name: None,
            place: None,
        };
        wall.request("Tom", &add, now).unwrap();
    }
    let mut ids = Vec::new();
    for (from, to) in cables {
        let patch = Request::Patch {
            from: (*from).to_string(),
            to: (*to).to_string(),
            amount: None,
        };
        match wall.request("Tom", &patch, now).unwrap().change {
            Some(Change {
                what: What::Patch { cable, .. },
                ..
            }) => ids.push(cable.id),
            other => panic!("{from} → {to}: {other:?}"),
        }
    }
    (wall, ids)
}

fn glided(module: &str, knob: &str, value: f64, beats: f64) -> Request {
    Request::Turn {
        module: module.to_string(),
        knob: knob.to_string(),
        value,
        glide_beats: Some(beats),
    }
}

fn latency_of_module(wall: &Wall, module: &str) -> Option<u32> {
    wall.snapshot()
        .timing
        .modules
        .get(module)
        .map(|timing| timing.latency_frames)
        .filter(|frames| *frames > 0)
}

/// vco1 → flanger1 → mix1.a, and vco1 → mix1.b dry (the third cable).
const FLANGED: [(&str, &str); 3] = [
    ("vco1.out", "flanger1.left"),
    ("flanger1.left", "mix1.a"),
    ("vco1.out", "mix1.b"),
];

#[test]
fn a_flangers_latency_follows_its_mode_once_it_settles() {
    let now = Instant::now();
    let (mut wall, cables) = built(&["vco", "flanger", "mix"], &FLANGED, now);
    let dry = cables[2].to_string();
    let (classic, hold) = flanger_modes();
    let delay = |wall: &Wall| wall.snapshot().timing.cables.get(&dry).copied();
    assert_eq!(latency_of_module(&wall, "flanger1"), classic);
    assert_eq!(delay(&wall), classic);

    wall.request("Tom", &turn("flanger1", "mode", 1.0), now)
        .unwrap();
    wall.tick(now + Duration::from_millis(60));
    assert_eq!(latency_of_module(&wall, "flanger1"), classic, "early");
    // Turned again: the wait starts again.
    let later = now + Duration::from_millis(80);
    wall.request("Tom", &turn("flanger1", "mode", 1.0), later)
        .unwrap();
    wall.tick(now + Duration::from_millis(150));
    assert_eq!(latency_of_module(&wall, "flanger1"), classic, "mid-sweep");
    wall.tick(later + RETIME_AFTER);
    assert_eq!(latency_of_module(&wall, "flanger1"), hold);
    assert_eq!(delay(&wall), hold);

    let back = now + Duration::from_secs(3);
    wall.request("Tom", &turn("flanger1", "mode", 0.0), back)
        .unwrap();
    wall.tick(back + RETIME_AFTER);
    assert_eq!(latency_of_module(&wall, "flanger1"), classic);
    assert_eq!(delay(&wall), classic);
}

#[test]
fn a_knob_turned_again_is_timed_from_its_latest_turn() {
    let now = Instant::now();
    let (mut wall, _) = built(&["vco", "flanger", "mix"], &FLANGED, now);
    let (_, hold) = flanger_modes();
    // A 64-beat glide (32 s at 120 BPM), then straight there instead.
    wall.request("Tom", &glided("flanger1", "mode", 1.0, 64.0), now)
        .unwrap();
    wall.request("Tom", &turn("flanger1", "mode", 1.0), now)
        .unwrap();
    wall.tick(now + RETIME_AFTER);
    assert_eq!(latency_of_module(&wall, "flanger1"), hold);
}

#[test]
fn one_effect_gliding_never_holds_up_another() {
    let now = Instant::now();
    let (mut wall, _) = built(
        &["vco", "flanger", "flanger", "mix"],
        &[
            ("vco1.out", "flanger1.left"),
            ("vco1.out", "flanger2.left"),
            ("flanger1.left", "mix1.a"),
            ("flanger2.left", "mix1.b"),
            ("vco1.out", "mix1.c"),
        ],
        now,
    );
    let (classic, hold) = flanger_modes();
    wall.request("Tom", &glided("flanger1", "mode", 1.0, 64.0), now)
        .unwrap();
    wall.request("Tom", &turn("flanger2", "mode", 1.0), now)
        .unwrap();
    wall.tick(now + RETIME_AFTER);
    assert_eq!(latency_of_module(&wall, "flanger2"), hold);
    // flanger1 is still on its way (it lands half-way, 16 s in).
    assert_eq!(latency_of_module(&wall, "flanger1"), classic);
    // Nor does a new cable re-time flanger1 early.
    let plug = Request::Patch {
        from: "vco1.out".to_string(),
        to: "mix1.d".to_string(),
        amount: None,
    };
    wall.request("Tom", &plug, now + Duration::from_secs(1))
        .unwrap();
    assert_eq!(latency_of_module(&wall, "flanger1"), classic);
    wall.tick(now + Duration::from_secs(16) + RETIME_AFTER);
    assert_eq!(latency_of_module(&wall, "flanger1"), hold);
}

#[test]
fn a_stepped_knob_is_timed_from_where_it_lands() {
    let now = Instant::now();
    let (mut wall, _) = built(&["vco", "flanger", "mix"], &FLANGED, now);
    let (classic, hold) = flanger_modes();
    // Two beats (1 s) from classic to through-zero: the switch rounds over
    // half-way, 500 ms in.
    wall.request("Tom", &glided("flanger1", "mode", 1.0, 2.0), now)
        .unwrap();
    wall.tick(now + Duration::from_millis(550));
    assert_eq!(latency_of_module(&wall, "flanger1"), classic);
    wall.tick(now + Duration::from_millis(500) + RETIME_AFTER);
    assert_eq!(latency_of_module(&wall, "flanger1"), hold);
}

#[test]
fn a_gate_is_a_gate_whichever_input_it_goes_into() {
    let now = Instant::now();
    let (wall, cables) = built(
        &["clock", "mix", "vco", "env"],
        &[
            ("clock1.out", "mix1.a"),
            ("vco1.out", "mix1.b"),
            ("clock1.out", "env1.gate"),
            ("clock1.out", "vco1.fm"),
        ],
        now,
    );
    let gate = |id: u32| {
        let cable = wall
            .patch()
            .cables()
            .iter()
            .find(|cable| cable.id == id)
            .unwrap()
            .clone();
        wall.carries_gate(&cable)
    };
    assert!(gate(cables[0]), "a clock into a mixer");
    assert!(!gate(cables[1]), "a vco into a mixer");
    assert!(gate(cables[2]), "a clock into a gate input");
    assert!(gate(cables[3]), "a clock into an fm input");
}

#[test]
fn a_cable_keeps_its_line_and_a_new_one_takes_the_lowest_free() {
    let now = Instant::now();
    let (mut wall, cables) = built(
        &["vco", "mix"],
        &[
            ("vco1.out", "mix1.a"),
            ("vco1.out", "mix1.b"),
            ("vco1.out", "mix1.c"),
        ],
        now,
    );
    let before = wall.lines.clone();
    let lines: Vec<u16> = cables.iter().map(|id| before[id].0).collect();
    assert_eq!(lines, [0, 1, 2]);
    let unplug = Request::Unpatch {
        cable: Some(cables[1]),
        to: None,
    };
    wall.request("Tom", &unplug, now).unwrap();
    let plug = Request::Patch {
        from: "vco1.out".to_string(),
        to: "mix1.d".to_string(),
        amount: None,
    };
    let new = match wall.request("Tom", &plug, now).unwrap().change {
        Some(Change {
            what: What::Patch { cable, .. },
            ..
        }) => cable.id,
        other => panic!("{other:?}"),
    };
    // The others keep line and tag; the newcomer takes line 1 under a tag
    // never used before.
    assert_eq!(wall.lines[&cables[0]], before[&cables[0]]);
    assert_eq!(wall.lines[&cables[2]], before[&cables[2]]);
    let (line, tag) = wall.lines[&new];
    assert_eq!(line, 1);
    assert!(before.values().all(|(_, old)| *old != tag));
}

#[test]
fn an_effect_leaving_the_wall_lets_its_probe_go() {
    let now = Instant::now();
    let (mut wall, _) = built(&["vco", "flanger", "mix"], &FLANGED, now);
    assert_eq!(wall.probes.len(), 1);
    let remove = Request::Remove {
        module: "flanger1".to_string(),
    };
    wall.request("Tom", &remove, now).unwrap();
    assert!(wall.probes.is_empty());
}

#[test]
fn a_new_engine_rate_measures_latency_again() {
    let now = Instant::now();
    let (mut wall, _) = built(&["vco", "flanger", "mix"], &FLANGED, now);
    wall.request("Tom", &turn("flanger1", "mode", 1.0), now)
        .unwrap();
    wall.tick(now + RETIME_AFTER);
    assert_eq!(
        latency_of_module(&wall, "flanger1"),
        flanger_at(1.0, 48_000.0)
    );
    let (_engine, control) = engine(EngineConfig::new(96_000, 120.0, 0.0), None);
    wall.attach(control, None);
    assert_eq!(
        latency_of_module(&wall, "flanger1"),
        flanger_at(1.0, 96_000.0)
    );
    assert_eq!(wall.snapshot().timing.sample_rate, 96_000);
}

#[test]
fn a_speaker_brought_back_by_undo_says_its_words_again() {
    let (_engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let mut wall = Wall::new(Patch::empty(120.0), control, None, Vec::new(), None);
    let (speech, script) = super::super::speech::tests::scripted(16);
    wall.speech = speech;
    let now = Instant::now();
    let add = Request::Add {
        kind: "speak".to_string(),
        name: None,
        place: None,
    };
    wall.request("Tom", &add, now).unwrap();
    let speak = Request::Speak {
        module: "speak1".to_string(),
        text: "hello there".to_string(),
        voice: None,
    };
    wall.request("Tom", &speak, now).unwrap();
    script.lock().unwrap().finish_all();
    let answers = wall.spoken();
    assert!(answers[0].1.is_ok());
    let remove = Request::Remove {
        module: "speak1".to_string(),
    };
    let removed = wall.request("Tom", &remove, now).unwrap();
    let seq = removed.change.unwrap().seq;
    assert!(wall.patch().speech.is_empty());
    wall.request("Tom", &Request::Undo { change: seq }, now)
        .unwrap();
    assert_eq!(
        wall.patch()
            .speech
            .get("speak1")
            .map(|words| words.text.as_str()),
        Some("hello there")
    );
    // Its words are on their way back to its player.
    assert_eq!(script.lock().unwrap().texts(), ["hello there"]);
}

#[test]
fn a_latency_that_a_cable_moves_is_said_to_be_unsteady() {
    let now = Instant::now();
    let (mut wall, _) = built(
        &["vco", "flanger", "mix", "lfo", "lfo"],
        &[
            ("vco1.out", "flanger1.left"),
            ("flanger1.left", "mix1.a"),
            ("vco1.out", "mix1.b"),
            // The rate knob does not move the latency...
            ("lfo1.out", "flanger1.rate"),
        ],
        now,
    );
    assert!(wall.snapshot().timing.unsteady.is_empty());
    // ...the mode knob does.
    let plug = Request::Patch {
        from: "lfo2.out".to_string(),
        to: "flanger1.mode".to_string(),
        amount: None,
    };
    wall.request("Tom", &plug, now).unwrap();
    assert_eq!(wall.snapshot().timing.unsteady, ["flanger1"]);
}

#[test]
fn a_snapshot_from_a_wall_without_timing_still_reads() {
    let (wall, _engine) = wall();
    let mut value = serde_json::to_value(wall.snapshot()).unwrap();
    value.as_object_mut().unwrap().remove("timing");
    let read: Snapshot = serde_json::from_value(value).unwrap();
    assert_eq!(read.timing, Timings::default());
}

/// The loudest sample the wall plays over `blocks` blocks of 10 ms, after
/// half a second to settle.
fn loudest(wall: &mut Wall, engine: &mut Engine, blocks: usize) -> f32 {
    let mut buffer = vec![0.0_f32; 480 * 2];
    let mut peak = 0.0_f32;
    for block in 0..blocks + 50 {
        engine.render(&mut buffer, 2);
        wall.tick(Instant::now());
        if block >= 50 {
            peak = buffer.iter().fold(peak, |m, s| m.max(s.abs()));
        }
    }
    peak
}

#[test]
fn a_dry_path_and_its_held_back_twin_cancel() {
    // A through-zero flanger with no wet signal is its dry signal held
    // back; summed against the dry path turned over, the two cancel only
    // if the dry path is held back exactly as long.
    let now = Instant::now();
    let (mut engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let mut wall = Wall::new(Patch::empty(120.0), control, None, Vec::new(), None);
    for kind in ["vco", "flanger", "mix", "out"] {
        let add = Request::Add {
            kind: kind.to_string(),
            name: None,
            place: None,
        };
        wall.request("Tom", &add, now).unwrap();
    }
    wall.request("Tom", &turn("flanger1", "mode", 1.0), now)
        .unwrap();
    wall.request("Tom", &turn("flanger1", "mix", 0.0), now)
        .unwrap();
    let mut dry = 0;
    for (from, to, amount) in [
        ("vco1.out", "flanger1.left", None),
        ("flanger1.left", "mix1.a", None),
        ("vco1.out", "mix1.b", Some(-1.0)),
        ("mix1.out", "out1.left", None),
    ] {
        let plug = Request::Patch {
            from: from.to_string(),
            to: to.to_string(),
            amount,
        };
        if let Some(Change {
            what: What::Patch { cable, .. },
            ..
        }) = wall.request("Tom", &plug, now).unwrap().change
        {
            if amount.is_some() {
                dry = cable.id;
            }
        } else {
            panic!("{from} → {to} made no cable");
        }
    }
    wall.tick(now + RETIME_AFTER);
    assert!(latency_of_module(&wall, "flanger1").is_some());
    let cancelled = loudest(&mut wall, &mut engine, 100);
    let unplug = Request::Unpatch {
        cable: Some(dry),
        to: None,
    };
    wall.request("Tom", &unplug, now).unwrap();
    let alone = loudest(&mut wall, &mut engine, 100);
    assert!(alone > 0.05, "the flanger path alone is {alone}");
    assert!(
        cancelled < alone * 0.01,
        "in step, the two leave {cancelled} of {alone}"
    );
}

/// A full wall: `MAX_MODULES` modules, every kind in turn, and
/// `MAX_CABLES` cables, each module's first output into the next one's
/// first input and the rest into knob jacks. Returns it and its cable count.
fn full_wall() -> (Patch, usize) {
    let mut patch = Patch::empty(120.0);
    let kinds: Vec<Kind> = Kind::all().collect();
    let mut ids = Vec::new();
    for index in 0..crate::MAX_MODULES {
        let kind = kinds[index % kinds.len()];
        match patch.add(kind.name(), None, None).unwrap() {
            What::Add { module } => ids.push((module.id, kind)),
            other => panic!("{other:?}"),
        }
    }
    let output = |index: usize| {
        let (id, kind) = &ids[index % ids.len()];
        kind.spec()
            .outputs
            .first()
            .map(|port| format!("{id}.{}", port.name))
    };
    let mut destinations = Vec::new();
    for (id, kind) in &ids {
        let spec = kind.spec();
        destinations.extend(spec.inputs.iter().map(|port| format!("{id}.{}", port.name)));
        destinations.extend(spec.knobs.iter().map(|knob| format!("{id}.{}", knob.name)));
    }
    let mut cables = 0;
    let mut source = 0;
    for to in destinations {
        if cables == crate::MAX_CABLES {
            break;
        }
        // The next module that has an output feeds this jack.
        let from = loop {
            source += 1;
            if let Some(from) = output(source) {
                break from;
            }
        };
        if patch.plug(&from, &to, Some(0.3), None).is_ok() {
            cables += 1;
        }
    }
    (patch, cables)
}

#[test]
fn a_full_wall_holds_every_module_and_cable_and_no_more() {
    let (mut patch, cables) = full_wall();
    assert_eq!(patch.modules().len(), crate::MAX_MODULES);
    assert_eq!(cables, crate::MAX_CABLES);
    assert_eq!(
        patch.add("vco", None, None).unwrap_err().code,
        ErrorCode::Full
    );
    // And it plays, without allocating, with every cable on a line.
    let (mut engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let mut wall = Wall::new(patch, control, None, Vec::new(), None);
    assert_eq!(wall.lines.len(), crate::MAX_CABLES);
    let mut buffer = vec![0.0_f32; 512 * 2];
    for _ in 0..20 {
        engine.render(&mut buffer, 2);
        wall.tick(Instant::now());
    }
    let before = assert_no_alloc::violation_count();
    for _ in 0..20 {
        assert_no_alloc::assert_no_alloc(|| engine.render(&mut buffer, 2));
        assert!(buffer.iter().all(|s| s.is_finite()));
    }
    assert_eq!(assert_no_alloc::violation_count(), before);
}

/// The share of a callback's time a full wall takes at `rate` with
/// `frames`-frame buffers: the 99th percentile over `callbacks` calls.
fn load(rate: u32, frames: usize, callbacks: usize) -> f64 {
    let (patch, _) = full_wall();
    let (mut engine, control) = engine(EngineConfig::new(rate, 120.0, 0.0), None);
    let mut wall = Wall::new(patch, control, None, Vec::new(), None);
    let mut buffer = vec![0.0_f32; frames * 2];
    for _ in 0..20 {
        engine.render(&mut buffer, 2);
        wall.tick(Instant::now());
    }
    let budget = frames as f64 / f64::from(rate);
    let mut shares: Vec<f64> = (0..callbacks)
        .map(|_| {
            let start = Instant::now();
            engine.render(&mut buffer, 2);
            start.elapsed().as_secs_f64() / budget
        })
        .collect();
    shares.sort_by(f64::total_cmp);
    shares[callbacks * 99 / 100]
}

#[test]
fn a_full_wall_keeps_well_inside_the_callback_budget() {
    for (rate, frames) in [(192_000, 1_024), (48_000, 512)] {
        let share = load(rate, frames, 200);
        println!(
            "full wall at {rate} Hz, {frames} frames: {:.1}% of the callback",
            share * 100.0
        );
        // Only an optimised build says anything about the audio thread.
        if !cfg!(debug_assertions) {
            assert!(share < 0.6, "{:.1}% at {rate} Hz", share * 100.0);
        }
    }
}

/// A wall recording into a directory of its own.
fn recording_wall() -> (Wall, Engine, tempfile::TempDir) {
    let (mut wall, engine) = wall();
    let dir = tempfile::tempdir().unwrap();
    wall.set_recordings_dir(dir.path().to_path_buf());
    (wall, engine, dir)
}

fn record(wall: &mut Wall, seat: &str, on: bool) -> Outcome {
    wall.request(seat, &Request::Record { on }, Instant::now())
        .unwrap()
}

fn record_result(outcome: &Outcome) -> RecordResult {
    serde_json::from_value(outcome.result.clone()).unwrap()
}

#[test]
fn a_seat_starts_and_stops_a_recording_and_both_are_logged() {
    let (mut wall, mut engine, dir) = recording_wall();
    assert_eq!(wall.snapshot().recording, None);
    let started = record(&mut wall, "Waffles", true);
    let result = record_result(&started);
    assert!(result.on);
    let path = result.path.unwrap();
    assert!(
        path.starts_with(&dir.path().to_string_lossy().into_owned()),
        "{path}"
    );
    let change = started.change.unwrap();
    assert_eq!(change.seat, "Waffles");
    assert!(matches!(&change.what, What::Record { on: true, path: logged, .. } if *logged == path));
    assert!(
        change
            .summary
            .starts_with("Waffles started recording wall-")
    );
    assert!(started.event.is_none(), "a recording touches no module");
    let mut buffer = vec![0.0; 4_800 * 2];
    engine.render(&mut buffer, 2);
    // Asked again: answered as it is, with no change.
    let again = record(&mut wall, "Tom", true);
    assert!(again.change.is_none());
    assert_eq!(record_result(&again).path.as_deref(), Some(path.as_str()));
    let shown = wall.snapshot().recording.unwrap();
    assert_eq!(shown.path, path);
    assert_eq!(shown.seat, "Waffles");
    assert_eq!(shown.sample_rate, 48_000);
    // Anyone may stop it.
    let stopped = record(&mut wall, "Tom", false);
    let result = record_result(&stopped);
    assert!(!result.on);
    assert_eq!(result.path.as_deref(), Some(path.as_str()));
    assert!((result.seconds - 0.1).abs() < 1e-9, "{}", result.seconds);
    let change = stopped.change.unwrap();
    assert_eq!(change.seat, "Tom");
    assert!(change.summary.starts_with("Tom stopped recording wall-"));
    assert!(
        change.summary.ends_with(" after 0:00"),
        "{}",
        change.summary
    );
    assert_eq!(wall.snapshot().recording, None);
    assert!(std::path::Path::new(&path).is_file());
    // Nothing to stop: no change.
    let idle = record(&mut wall, "Tom", false);
    assert!(idle.change.is_none());
    assert_eq!(record_result(&idle).path, None);
    // A recording cannot be undone.
    let err = ask(&mut wall, &Request::Undo { change: change.seq }).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotAllowed);
}

#[test]
fn a_recording_that_cannot_start_is_an_error_and_changes_nothing() {
    let (mut wall, _engine) = wall();
    let dir = tempfile::tempdir().unwrap();
    let blocked = dir.path().join("blocked");
    std::fs::write(&blocked, b"not a directory").unwrap();
    wall.set_recordings_dir(blocked.join("recordings"));
    let revision = wall.revision();
    let err = ask(&mut wall, &Request::Record { on: true }).unwrap_err();
    assert_eq!(err.code, ErrorCode::Internal);
    assert!(err.message.contains("could not start"), "{}", err.message);
    assert_eq!(wall.revision(), revision);
    assert_eq!(wall.snapshot().recording, None);
}

#[test]
fn a_rebuilt_engine_carries_the_recording_on_into_a_new_file() {
    let (mut wall, mut engine, _dir) = recording_wall();
    let first = record_result(&record(&mut wall, "Tom", true)).path.unwrap();
    let mut buffer = vec![0.0; 960 * 2];
    engine.render(&mut buffer, 2);
    drop(engine);
    let (mut rebuilt, control) = crate::engine::engine(EngineConfig::new(44_100, 120.0, 0.0), None);
    wall.attach(control, None);
    let shown = wall.snapshot().recording.unwrap();
    assert_ne!(shown.path, first);
    assert_eq!(shown.seat, "Tom", "still Tom's recording");
    assert_eq!(shown.sample_rate, 44_100);
    let told = wall.tick(Instant::now());
    let changes: Vec<&Change> = told
        .iter()
        .filter_map(|event| match event {
            Event::Change { change } => Some(change.as_ref()),
            _ => None,
        })
        .collect();
    assert_eq!(changes.len(), 2, "{told:?}");
    assert_eq!(changes[0].seat, LOADER_SEAT);
    assert!(
        changes[0]
            .summary
            .ends_with(": the audio restarted at 44100 Hz (it was 48000 Hz)"),
        "{}",
        changes[0].summary
    );
    assert!(
        matches!(&changes[1].what, What::Record { on: true, continues: Some(before), .. } if *before == first)
    );
    assert!(wall.tick(Instant::now()).is_empty(), "told once");
    rebuilt.render(&mut buffer, 2);
    let hound = hound::WavReader::open(&first).unwrap();
    assert_eq!(hound.spec().sample_rate, 48_000);
    assert_eq!(hound.duration(), 960);
    // The daemon stopping ends it, as the wall's own change.
    let Some(Event::Change { change }) = wall.end_recording() else {
        panic!("a recording was under way");
    };
    assert_eq!(change.seat, LOADER_SEAT);
    assert!(
        change.summary.ends_with(": the wall stopped"),
        "{}",
        change.summary
    );
    assert_eq!(wall.end_recording(), None);
}

#[test]
fn a_file_that_fills_carries_on_and_says_so() {
    let (mut wall, mut engine, _dir) = recording_wall();
    wall.recorder_mut().set_limit_frames(1_000);
    let first = record_result(&record(&mut wall, "Tom", true)).path.unwrap();
    let mut buffer = vec![0.0; 1_600 * 2];
    engine.render(&mut buffer, 2);
    let deadline = Instant::now() + Duration::from_secs(5);
    let told = loop {
        let told = wall.tick(Instant::now());
        if !told.is_empty() {
            break told;
        }
        assert!(Instant::now() < deadline, "the file never filled");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(told.len(), 2, "{told:?}");
    let shown = wall.snapshot().recording.unwrap();
    assert_ne!(shown.path, first);
    assert_eq!(hound::WavReader::open(&first).unwrap().duration(), 1_000);
}

fn arrange(module: &str, row: usize, before: Option<&str>) -> Request {
    Request::Arrange {
        module: module.to_string(),
        row,
        before: before.map(str::to_string),
        own: false,
    }
}

#[test]
fn the_seed_hangs_a_row_for_each_group() {
    let (wall, _engine) = wall();
    let rows = wall.snapshot().rack.expect("the rows");
    assert_eq!(rows[0], vec!["vco1", "vco2"], "sound sources first");
    let groups: Vec<usize> = rows
        .iter()
        .map(|row| {
            let kind = wall.patch().module(&row[0]).expect("on the wall").kind;
            kind.spec().group()
        })
        .collect();
    let mut sorted = groups.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(groups, sorted, "one row per group, in order");
    for row in &rows {
        let group = wall
            .patch()
            .module(&row[0])
            .expect("on the wall")
            .kind
            .spec()
            .group();
        assert!(row.iter().all(|id| {
            wall.patch()
                .module(id)
                .expect("on the wall")
                .kind
                .spec()
                .group()
                == group
        }));
    }
}

#[test]
fn a_move_is_shared_saved_and_told_but_never_logged() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let (_engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let mut wall = Wall::new(seed().unwrap(), control, None, Vec::new(), Some(store));
    let now = Instant::now();
    let revision = wall.revision();
    let outcome = wall
        .request("Tom", &arrange("lfo1", 0, Some("vco2")), now)
        .unwrap();
    // No change: nothing in the log, the revision stays.
    assert!(outcome.change.is_none());
    assert_eq!(wall.revision(), revision);
    assert!(wall.log_page(None, None).changes.is_empty());
    // Every subscriber hears the rows as they now stand, and the asker
    // has them in its answer.
    let result: crate::protocol::ArrangeResult = serde_json::from_value(outcome.result).unwrap();
    assert_eq!(result.rows[0], vec!["vco1", "lfo1", "vco2"]);
    let Some(Event::Rack { rows }) = outcome.event else {
        panic!("no rows event: {:?}", outcome.event);
    };
    assert_eq!(rows, result.rows);
    assert_eq!(wall.snapshot().rack, Some(rows.clone()));
    // Saved with the patch.
    wall.tick(now);
    let saved = Store::open(dir.path()).unwrap();
    let crate::store::Loaded::Patch { patch, .. } = saved.load_patch().unwrap() else {
        panic!("not saved");
    };
    assert_eq!(patch.rows(), rows.as_slice());
    // A move to where it is changes nothing, and still answers.
    let again = wall
        .request("Tom", &arrange("lfo1", 0, Some("vco2")), now)
        .unwrap();
    assert!(matches!(again.event, Some(Event::Rack { .. })));
    // A module not on the wall cannot move.
    assert_eq!(
        wall.request("Tom", &arrange("vco9", 0, None), now)
            .unwrap_err()
            .code,
        ErrorCode::UnknownModule
    );
}

#[test]
fn moves_count_against_the_flood_guard() {
    let (mut wall, _engine) = wall();
    let now = Instant::now();
    for step in 0..FLOOD_BURST as usize {
        let row = step % 2;
        assert!(
            wall.request("Loop", &arrange("lfo1", row, None), now)
                .is_ok()
        );
    }
    assert_eq!(
        wall.request("Loop", &arrange("lfo1", 0, None), now)
            .unwrap_err()
            .code,
        ErrorCode::SlowDown
    );
}

#[test]
fn adding_in_a_place_and_undoing_a_removal_keep_the_rows() {
    let (mut wall, _engine) = wall();
    let rows = |wall: &Wall| wall.snapshot().rack.expect("the rows");
    let added = changed(
        ask(
            &mut wall,
            &Request::Add {
                kind: "noise".to_string(),
                name: None,
                place: Some(Place {
                    row: 0,
                    before: Some("vco2".to_string()),
                    own: false,
                }),
            },
        )
        .unwrap(),
    );
    let id = added.module.expect("the new module");
    assert_eq!(
        rows(&wall)[0],
        vec!["vco1".to_string(), id.clone(), "vco2".to_string()]
    );
    // Without a place: beside the newest of its sort.
    let second = changed(
        ask(
            &mut wall,
            &Request::Add {
                kind: "noise".to_string(),
                name: None,
                place: None,
            },
        )
        .unwrap(),
    )
    .module
    .expect("the new module");
    assert_eq!(rows(&wall)[0].last(), Some(&second));
    // Taken away and brought back: where it was.
    let before = rows(&wall);
    let removed = changed(ask(&mut wall, &Request::Remove { module: id.clone() }).unwrap());
    assert!(!rows(&wall)[0].contains(&id));
    ask(
        &mut wall,
        &Request::Undo {
            change: removed.change.seq,
        },
    )
    .unwrap();
    assert_eq!(rows(&wall), before);
}
