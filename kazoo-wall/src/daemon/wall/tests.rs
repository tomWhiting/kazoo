//! Wall tests: requests, the flood guard, undo, the log, the snapshot and
//! faults.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use super::*;
use crate::SUB_BLOCK;
use crate::dsp::{Io, Module as DspModule, Tick};
use crate::engine::{Engine, EngineConfig, engine};
use crate::patch::PatchFile;
use crate::protocol::{ChangeResult, ProbeResult, Snapshot};
use crate::seed::seed;
use crate::store::{FailAt, Loaded};

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
fn undo_that_no_longer_applies_is_an_error() {
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
}

fn add(kind: &str) -> Request {
    Request::Add {
        kind: kind.to_string(),
        name: None,
        place: None,
    }
}

fn patch(from: &str, to: &str) -> Request {
    Request::Patch {
        from: from.to_string(),
        to: to.to_string(),
        amount: None,
    }
}

#[test]
fn the_wall_has_no_fixed_size_and_grows_its_engine_as_it_goes() {
    let (mut wall, _engine) = wall();
    // Far past the 96 modules and 320 cables the wall once held at most,
    // and past the engine's first room, by many seats, each within its
    // own flood allowance.
    let target = 130;
    let mut mixers = Vec::new();
    for count in wall.patch().modules().len()..target {
        let added = wall
            .request(&format!("Filler{count}"), &add("mix"), Instant::now())
            .unwrap();
        mixers.push(changed(added.result).module.unwrap());
    }
    assert_eq!(wall.patch().modules().len(), target);
    let mut cables = wall.patch().cables().len();
    for (n, mixer) in mixers.iter().enumerate() {
        for jack in ["a", "b", "c", "d"] {
            let seat = format!("Patcher{n}{jack}");
            wall.request(
                &seat,
                &patch("vco1.out", &format!("{mixer}.{jack}")),
                Instant::now(),
            )
            .unwrap();
            cables += 1;
        }
    }
    assert!(cables > 320, "{cables}");
    assert_eq!(wall.patch().cables().len(), cables);
    assert_eq!(wall.lines.len(), cables);
    // Every cable has a line of its own, and the engine has been sent
    // room for every module and line, grown by doubling.
    let lines: BTreeSet<u32> = wall.lines.values().map(|(line, _)| *line).collect();
    assert_eq!(lines.len(), cables);
    assert!(wall.engine.slots() >= target && wall.engine.slots().is_power_of_two());
    assert!(wall.engine.lines() >= cables);
}

#[test]
fn the_wall_refuses_more_work_while_rendering_takes_most_of_its_time() {
    let (mut wall, _engine) = wall();
    let removed = changed(
        ask(
            &mut wall,
            &Request::Remove {
                module: "lfo2".to_string(),
            },
        )
        .unwrap(),
    );
    let mixer = changed(ask(&mut wall, &add("mix")).unwrap())
        .module
        .unwrap();
    let plugged = changed(ask(&mut wall, &patch("vco1.out", &format!("{mixer}.a"))).unwrap());
    let unplugged = changed(
        ask(
            &mut wall,
            &Request::Unpatch {
                cable: plugged.cable,
                to: None,
            },
        )
        .unwrap(),
    );
    ask(&mut wall, &patch("vco1.out", &format!("{mixer}.b"))).unwrap();

    // The daemon turns the guard on; the wall's load has been measured
    // long enough to judge by.
    wall.set_load_guard(true);
    measured_long_enough(&wall);
    wall.engine.shared().set_render_load(LOAD_GUARD + 0.05);
    let full = ask(&mut wall, &add("mix")).unwrap_err();
    assert_eq!(full.code, ErrorCode::Full);
    assert!(full.message.contains("85%"), "{}", full.message);
    // A new cable is more work; so is bringing back what was taken away.
    let into_free = patch("vco1.out", &format!("{mixer}.c"));
    assert_eq!(code(ask(&mut wall, &into_free)), ErrorCode::Full);
    for change in [removed.change.seq, unplugged.change.seq] {
        assert_eq!(
            code(ask(&mut wall, &Request::Undo { change })),
            ErrorCode::Full
        );
    }
    // Refused changes changed nothing.
    assert!(wall.patch().module("lfo2").is_none());
    assert!(!wall.patch().is_plugged(&format!("{mixer}.c")));
    // Changes that add no work still go: a knob, a cable moved to a jack
    // already in use, taking things away.
    ask(&mut wall, &turn("vcf1", "cutoff", 420.0)).unwrap();
    ask(&mut wall, &patch("lfo1.out", &format!("{mixer}.b"))).unwrap();
    ask(&mut wall, &Request::Remove { module: mixer }).unwrap();

    // Once refusing, a load just under the guard is not enough: it takes
    // more only below RESUME_SHARE of it.
    wall.engine.shared().set_render_load(LOAD_GUARD - 0.05);
    assert_eq!(code(ask(&mut wall, &add("mix"))), ErrorCode::Full);
    wall.engine
        .shared()
        .set_render_load(LOAD_GUARD.mul_add(RESUME_SHARE, -0.2));
    ask(&mut wall, &add("mix")).unwrap();
    ask(
        &mut wall,
        &Request::Undo {
            change: removed.change.seq,
        },
    )
    .unwrap();
    assert!(wall.patch().module("lfo2").is_some());
    assert!(!wall.load_watch.refusing);

    // A wall told not to guard (a test daemon in a debug build) takes more
    // whatever the load.
    wall.engine.shared().set_render_load(LOAD_GUARD + 1.0);
    assert_eq!(code(ask(&mut wall, &add("mix"))), ErrorCode::Full);
    wall.set_load_guard(false);
    ask(&mut wall, &add("mix")).unwrap();
    let health = wall.snapshot().engine.unwrap();
    assert_eq!(health.load_guard, None);
    assert!((health.load - f64::from(LOAD_GUARD + 1.0)).abs() < 1e-6);
}

/// Move the engine's frames on past what the wall waits for after it is
/// put up, and past every change it has not taken in yet.
fn measured_long_enough(wall: &Wall) {
    let window = u64::from(wall.engine.sample_rate());
    let shared = wall.engine.shared();
    shared.set_frames(shared.frames() + window * ABSORBED_WINDOWS * 2);
}

#[test]
fn a_wall_just_put_up_is_measured_before_it_takes_more() {
    let (mut wall, _engine) = wall();
    wall.set_load_guard(true);
    let waits = ask(&mut wall, &add("mix")).unwrap_err();
    assert_eq!(waits.code, ErrorCode::SlowDown);
    assert!(waits.message.contains("measured"), "{}", waits.message);
    assert!(wall.snapshot().engine.unwrap().measuring);
    // What adds nothing goes meanwhile.
    ask(&mut wall, &turn("vcf1", "cutoff", 420.0)).unwrap();
    measured_long_enough(&wall);
    ask(&mut wall, &add("mix")).unwrap();
    let health = wall.snapshot().engine.unwrap();
    assert!(!health.measuring && !health.refusing);
    assert_eq!(health.load_guard, Some(f64::from(LOAD_GUARD)));
    // Another wall put up is measured afresh.
    wall.swap_engine();
    assert!(wall.snapshot().engine.unwrap().measuring);
    assert_eq!(code(ask(&mut wall, &add("mix"))), ErrorCode::SlowDown);
}

#[test]
fn each_addition_is_costed_and_they_come_apart_near_the_guard() {
    let (mut wall, _engine) = wall();
    wall.set_load_guard(true);
    measured_long_enough(&wall);
    let units = wall.patch().modules().len() + wall.patch().cables().len();
    assert!(units >= COSTED_FROM, "{units}");
    // Each module and cable takes a share of the load: one more past the
    // guard is refused, however the average stands.
    let share = LOAD_GUARD / units as f32;
    wall.engine
        .shared()
        .set_render_load(share * (units as f32 - 0.5));
    let costed = ask(&mut wall, &add("mix")).unwrap_err();
    assert_eq!(costed.code, ErrorCode::Full);
    assert!(
        costed.message.contains("would take it"),
        "{}",
        costed.message
    );
    assert!(!wall.load_watch.refusing, "costed, not refusing");

    // Above SETTLE_SHARE of the guard, an addition waits for the last to
    // be taken in.
    let settling = LOAD_GUARD.mul_add(SETTLE_SHARE, 0.01);
    wall.engine.shared().set_render_load(settling * 0.5);
    ask(&mut wall, &add("mix")).unwrap();
    wall.engine.shared().set_render_load(settling);
    let wait = ask(&mut wall, &add("mix")).unwrap_err();
    assert_eq!(wait.code, ErrorCode::SlowDown);
    // Taking something away is no reason to wait, and is no addition.
    let removed = changed(
        ask(
            &mut wall,
            &Request::Remove {
                module: "lfo2".to_string(),
            },
        )
        .unwrap(),
    );
    assert_eq!(code(ask(&mut wall, &add("mix"))), ErrorCode::SlowDown);
    // Below it, additions come as fast as the seats make them.
    wall.engine.shared().set_render_load(settling * 0.5);
    ask(&mut wall, &add("mix")).unwrap();
    // Once the load has taken them in, the next is judged on it alone.
    measured_long_enough(&wall);
    wall.engine.shared().set_render_load(settling);
    ask(
        &mut wall,
        &Request::Undo {
            change: removed.change.seq,
        },
    )
    .unwrap();
    assert_eq!(wall.load_watch.recent.len(), 1);
    assert!(wall.load_watch.recent[0].1 > 1, "a module and its cables");
}

#[test]
fn what_was_taken_away_lately_counts_against_the_cost() {
    let (mut wall, _engine) = wall();
    wall.set_load_guard(true);
    measured_long_enough(&wall);
    let units = wall.patch().modules().len() + wall.patch().cables().len();
    assert!((COSTED_FROM..50).contains(&units), "{units}");
    // Measured with every module and cable there, just inside the guard
    // for one more. (Shared over what is left after a removal instead, one
    // more would be past it, on a wall of fewer than 50.)
    let share = LOAD_GUARD / (units as f32 + 1.02);
    wall.engine.shared().set_render_load(share * units as f32);
    // Taking a module away, the load (not yet taking it in) is shared
    // over what it was measured on, not over what is left: the next
    // addition is costed no higher than before.
    ask(
        &mut wall,
        &Request::Remove {
            module: "lfo2".to_string(),
        },
    )
    .unwrap();
    ask(&mut wall, &add("mix")).unwrap();
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
    // The wall's own tempo, as the daemon starts its clock on it (this
    // engine was started at 120).
    let initial = wall.patch().tempo;
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
    assert!(matches!(undone.change.what, What::Tempo { to, .. } if (to - initial).abs() < 1e-9));
    engine.render(&mut buffer, 2);
    assert!((wall.snapshot().tempo - initial).abs() < f64::EPSILON);
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
    let store = Store::peek(dir.path()).unwrap();
    let (_engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let mut wall = Wall::new(seed().unwrap(), control, None, Vec::new(), Some(store));
    let now = Instant::now();
    wall.request("Tom", &turn("vcf1", "cutoff", 333.0), now)
        .unwrap();
    wall.tick(now);
    let saved = Store::peek(dir.path()).unwrap();
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
    let store = Store::peek(dir.path()).unwrap();
    let (_engine, control) = engine(EngineConfig::new(48_000, 92.0, 0.0), None);
    let mut wall = Wall::new(seed().unwrap(), control, None, Vec::new(), Some(store));
    // The engine never renders here: the tempo must be saved regardless.
    ask(&mut wall, &Request::Tempo { bpm: 100.0 }).unwrap();
    wall.save(Instant::now());
    let saved = Store::peek(dir.path()).unwrap();
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
    let store = Store::peek(dir.path()).unwrap();
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
    let saved = Store::peek(dir.path()).unwrap();
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
    wall.request("Tom", &add("readout"), now).unwrap();
    plug(&mut wall, "mix1.out", "readout1.in");
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
    // The disconnected drive is no longer measured or rendered.
    assert!(!timing.modules.contains_key("klon1"));
    assert_eq!(timing.unheard, ["klon1"]);
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
    wall.request("Tom", &add("readout"), now).unwrap();
    wall.request("Tom", &patch("mix1.out", "readout1.in"), now)
        .unwrap();
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
        let kinds: HashMap<&str, Kind> = wall
            .patch()
            .modules()
            .iter()
            .map(|module| (module.id.as_str(), module.kind))
            .collect();
        carries_gate(&kinds, &cable)
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
    let lines: Vec<u32> = cables.iter().map(|id| before[id].0).collect();
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

/// The most modules the wall once held, when it had a fixed size: the
/// callback budget is still measured at it.
const OLD_MODULES: usize = 96;

/// The most cables the wall once held.
const OLD_CABLES: usize = 320;

/// A wall of `modules` modules, every kind in turn, and up to `most`
/// cables, each module's first output into the next one's first input and
/// the rest into knob jacks. Returns it and its cable count.
fn full_wall(modules: usize, most: usize) -> (Patch, usize) {
    let mut patch = Patch::empty(120.0);
    let kinds: Vec<Kind> = Kind::all().collect();
    let mut ids = Vec::new();
    for index in 0..modules {
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
        if cables == most {
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

/// Render one 512-frame block under `assert_no_alloc`, failing on any
/// allocation or free.
fn render_without_allocating(engine: &mut Engine, buffer: &mut [f32]) {
    let before = assert_no_alloc::violation_count();
    assert_no_alloc::assert_no_alloc(|| engine.render(buffer, 2));
    assert_eq!(
        assert_no_alloc::violation_count(),
        before,
        "the audio thread allocated or freed memory"
    );
    assert!(buffer.iter().all(|s| s.is_finite()));
}

#[test]
fn a_wall_past_the_old_limits_grows_and_plays_without_allocating() {
    let (patch, cables) = full_wall(OLD_MODULES * 2, OLD_CABLES * 2);
    assert_eq!(patch.modules().len(), OLD_MODULES * 2);
    assert_eq!(cables, OLD_CABLES * 2);
    let (mut engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let mut wall = Wall::new(patch, control, None, Vec::new(), None);
    assert_eq!(wall.lines.len(), cables);
    // The engine starts with room for a modest wall. The room and lines
    // this one needs were built on the control side and sent; the engine
    // moves into them as it renders, without allocating.
    assert_eq!(wall.engine.shared().slots(), crate::engine::INITIAL_SLOTS);
    let mut buffer = vec![0.0_f32; 512 * 2];
    render_without_allocating(&mut engine, &mut buffer);
    assert!(wall.engine.shared().slots() >= OLD_MODULES * 2);
    assert!(wall.engine.shared().lines() >= cables);
    for _ in 0..5 {
        render_without_allocating(&mut engine, &mut buffer);
        wall.tick(Instant::now());
    }
    // Seats add to it as it plays, past the room it has: it grows again.
    // (Unguarded, as a test wall is: a debug build renders far slower than
    // the audio thread will, so its load says nothing.)
    let room = wall.engine.shared().slots();
    let mut count = 0;
    while wall.patch().modules().len() <= room {
        wall.request(&format!("Grower{count}"), &add("vco"), Instant::now())
            .unwrap();
        count += 1;
    }
    for _ in 0..5 {
        render_without_allocating(&mut engine, &mut buffer);
        wall.tick(Instant::now());
    }
    assert!(wall.engine.shared().slots() > room);
    assert!(wall.engine.shared().slots() >= wall.patch().modules().len());
    assert_eq!(wall.engine_mut().shared().ignored(), 0);
    assert_eq!(wall.engine_mut().shared().leaked(), 0);
}

/// The share of a callback's time a wall of the old most modules and
/// cables takes at `rate` with `frames`-frame buffers: the 99th percentile
/// over `callbacks` calls.
fn load(rate: u32, frames: usize, callbacks: usize) -> f64 {
    load_of(
        full_wall(OLD_MODULES, OLD_CABLES).0,
        rate,
        frames,
        callbacks,
    )
}

/// The share of a callback's time `patch` takes, as [`load`] measures it.
fn load_of(patch: Patch, rate: u32, frames: usize, callbacks: usize) -> f64 {
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
fn a_wall_of_the_old_most_keeps_well_inside_the_callback_budget() {
    for (rate, frames) in [(192_000, 1_024), (48_000, 512)] {
        let share = load(rate, frames, 200);
        println!(
            "{OLD_MODULES} modules and {OLD_CABLES} cables at {rate} Hz, {frames} frames: {:.1}% of the callback",
            share * 100.0
        );
        // Only an optimised build says anything about the audio thread.
        if !cfg!(debug_assertions) {
            assert!(share < 0.6, "{:.1}% at {rate} Hz", share * 100.0);
        }
    }
}

/// A wall of pitch shifters: `count` voices, each an oscillator through a
/// shifter with its `formant` switch at `formant` and its wet signal
/// patched on, into an `out`.
fn pitch_wall(count: usize, formant: f64) -> Patch {
    let mut patch = Patch::empty(120.0);
    let mut added = |kind: &str| match patch.add(kind, None, None).unwrap() {
        What::Add { module } => module.id,
        other => panic!("{other:?}"),
    };
    let mut voices = Vec::new();
    for _ in 0..count {
        voices.push((added("vco"), added("pitch"), added("out")));
    }
    for (vco, pitch, out) in voices {
        patch.turn(&pitch, "formant", formant, None).unwrap();
        patch.turn(&pitch, "shift", 7.0, None).unwrap();
        patch
            .plug(&format!("{vco}.out"), &format!("{pitch}.left"), None, None)
            .unwrap();
        patch
            .plug(&format!("{pitch}.left"), &format!("{out}.left"), None, None)
            .unwrap();
        patch
            .plug(
                &format!("{pitch}.wet_right"),
                &format!("{out}.right"),
                None,
                None,
            )
            .unwrap();
    }
    patch
}

#[test]
fn pitch_shifters_keep_inside_the_callback_budget() {
    // Sixteen voices, each shifted with its formants moved or kept: well
    // past what a wall holds of any one effect in practice.
    for (formant, way) in [(0.0, "moved"), (1.0, "kept")] {
        for (rate, frames) in [(48_000, 512), (192_000, 1_024)] {
            let share = load_of(pitch_wall(16, formant), rate, frames, 200);
            println!(
                "16 pitch shifters, formants {way}, at {rate} Hz, {frames} frames: \
                 {:.1}% of the callback",
                share * 100.0
            );
            // Only an optimised build says anything about the audio thread.
            if !cfg!(debug_assertions) {
                assert!(share < 0.6, "{way}: {:.1}% at {rate} Hz", share * 100.0);
            }
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
    let store = Store::peek(dir.path()).unwrap();
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
    let saved = Store::peek(dir.path()).unwrap();
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

// ---------------------------------------------------------------------------
// Saves
// ---------------------------------------------------------------------------

/// Tom's "Airports" wall as version 1 saved it.
const AIRPORTS: &str = include_str!("../../../tests/fixtures/airports-v1.json");

/// A wall on the seed patch keeping its state, saves and recordings in a
/// directory of its own.
/// Make step `at` of writing `wall`'s state fail, once.
fn fail_once(wall: &mut Wall, at: FailAt) {
    wall.store_mut().unwrap().fail_once(at);
}

fn saving_wall() -> (Wall, Engine, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let (engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let store = Store::open(dir.path()).unwrap();
    let mut wall = Wall::new(seed().unwrap(), control, None, Vec::new(), Some(store));
    wall.set_recordings_dir(dir.path().join("recordings"));
    (wall, engine, dir)
}

fn save(wall: &mut Wall, name: &str, overwrite: bool) -> Result<Outcome, WallError> {
    let request = Request::Save {
        name: name.to_string(),
        overwrite,
    };
    wall.request("Tom", &request, Instant::now())
}

fn load_save(wall: &mut Wall, name: &str) -> Result<LoadResult, WallError> {
    let request = Request::Load {
        name: name.to_string(),
    };
    ask(wall, &request).map(|value| serde_json::from_value(value).unwrap())
}

fn undo_load(wall: &mut Wall, change: u64) -> LoadResult {
    serde_json::from_value(ask(wall, &Request::Undo { change }).unwrap()).unwrap()
}

/// The wall as it would be saved, but for the latest change's number
/// (which every change moves on) and which wall it is (which every load
/// moves on).
fn as_saved(wall: &Wall) -> PatchFile {
    PatchFile {
        last_seq: 0,
        wall: 0,
        ..wall.patch().to_file()
    }
}

/// A save's file, as written.
fn save_file(dir: &tempfile::TempDir, name: &str) -> PatchFile {
    let path = dir.path().join("saves").join(format!("{name}.json"));
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

/// Render a little, so the engine takes in everything it was sent, then
/// tidy up after it.
fn play(wall: &mut Wall, engine: &mut Engine) {
    let mut buffer = vec![0.0; 4_096 * 2];
    for _ in 0..3 {
        engine.render(&mut buffer, 2);
    }
    wall.tick(Instant::now());
}

/// Every module the engine plays, by id, with its knobs where they are.
fn playing(wall: &Wall) -> Vec<(String, Vec<f64>)> {
    wall.snapshot()
        .modules
        .iter()
        .map(|module| {
            (
                module.id.clone(),
                module.knobs.iter().map(|knob| knob.value).collect(),
            )
        })
        .collect()
}

/// Put a slow wobble on the filter and save the wall as `jam`; returns the
/// wobble's id.
fn save_a_wobble(wall: &mut Wall) -> String {
    let add = Request::Add {
        kind: "lfo".to_string(),
        name: Some("slow wobble".to_string()),
        place: None,
    };
    let lfo = changed(ask(wall, &add).unwrap()).module.unwrap();
    let patch = Request::Patch {
        from: format!("{lfo}.out"),
        to: "vcf1.resonance".to_string(),
        amount: Some(0.3),
    };
    ask(wall, &patch).unwrap();
    save(wall, "jam", false).unwrap();
    lfo
}

/// A load names the save `backup` it kept the wall in by the very bytes of
/// its file (`fingerprint`), and that save is the wall that played (whose
/// fingerprint then was `print`).
fn kept_by_its_bytes(dir: &tempfile::TempDir, backup: &str, fingerprint: &str, print: &str) {
    let file = dir.path().join("saves").join(format!("{backup}.json"));
    assert_eq!(fingerprint, hash_bytes(&std::fs::read(&file).unwrap()));
    let (kept, _) = Patch::load(&save_file(dir, backup)).unwrap();
    assert_eq!(kept.fingerprint(), print, "and that save is the wall");
}

/// The engine plays every module of `saved`, in order, every knob at its
/// target: nothing glides into a loaded wall.
fn plays_at_its_targets(wall: &Wall, saved: &PatchFile) {
    let now_playing = playing(wall);
    let ids: Vec<&str> = now_playing.iter().map(|(id, _)| id.as_str()).collect();
    let saved_ids: Vec<&str> = saved
        .modules
        .iter()
        .map(|module| module.id.as_str())
        .collect();
    assert_eq!(ids, saved_ids);
    for (module, (_, values)) in saved.modules.iter().zip(&now_playing) {
        let spec = crate::catalogue::Kind::from_name(&module.kind)
            .unwrap()
            .spec();
        for (knob, value) in spec.knobs.iter().zip(values) {
            let target = module.knobs[knob.name];
            assert!(
                (value - target).abs() <= 1e-3 * target.abs().max(1.0),
                "{}.{} is at {value}, not {target}",
                module.id,
                knob.name
            );
        }
    }
}

#[test]
fn a_save_is_the_wall_at_its_targets_and_changes_nothing() {
    let (mut wall, _engine, dir) = saving_wall();
    // A knob on its way somewhere: the save holds where it is going.
    let glide = glided("vcf1", "cutoff", 2_000.0, 64.0);
    ask(&mut wall, &glide).unwrap();
    let revision = wall.revision();
    let outcome = save(&mut wall, "jam", false).unwrap();
    assert!(outcome.change.is_none(), "saving is not a change");
    assert_eq!(wall.revision(), revision);
    assert_eq!(
        outcome.event,
        Some(Event::Saved {
            seat: "Tom".to_string(),
            name: "jam".to_string(),
            replaced: false,
        })
    );
    let result: SaveResult = serde_json::from_value(outcome.result).unwrap();
    assert_eq!(result.save.name, "jam");
    assert!(!result.replaced && !result.save.auto);
    assert_eq!(result.save.modules as usize, wall.patch().modules().len());
    assert_eq!(result.save.cables as usize, wall.patch().cables().len());
    assert!((result.save.tempo - wall.patch().tempo).abs() < 1e-9);
    assert!(result.save.saved_at.ends_with('Z'));
    let file = save_file(&dir, "jam");
    assert_eq!(file, wall.patch().to_file(), "patch.json's own format");
    let vcf = file
        .modules
        .iter()
        .find(|module| module.id == "vcf1")
        .unwrap();
    assert!((vcf.knobs["cutoff"] - 2_000.0).abs() < 1e-9);
    assert_eq!(
        wall.snapshot().save,
        Some(SaveMark {
            name: "jam".to_string(),
            revision,
            changed: false,
        })
    );
    // A save of that name (in any case) is replaced only when asked.
    let refused = save(&mut wall, "JAM", false).unwrap_err();
    assert_eq!(refused.code, ErrorCode::SaveExists);
    assert!(refused.message.contains("\"jam\""), "{}", refused.message);
    let again = save(&mut wall, "JAM", true).unwrap();
    let again: SaveResult = serde_json::from_value(again.result).unwrap();
    assert!(again.replaced);
    assert_eq!(again.save.name, "jam", "it keeps its name");
    // Names that cannot be saves, and the wall's own.
    for bad in ["", "../patch", "jam.", "before-load-20261009-143005"] {
        let refused = save(&mut wall, bad, false).unwrap_err();
        assert_eq!(refused.code, ErrorCode::BadName, "{bad}");
    }
    let saves: SavesResult =
        serde_json::from_value(ask(&mut wall, &Request::Saves).unwrap()).unwrap();
    assert_eq!(saves.saves.len(), 1);
    assert_eq!(saves.saves[0].name, "jam");
    assert_eq!(saves.saves[0].version, crate::patch::FILE_VERSION);
    assert!(saves.broken.is_empty());
}

#[test]
fn a_load_replaces_the_wall_as_it_plays_and_undo_brings_it_back_exactly() {
    let (mut wall, mut engine, dir) = saving_wall();
    let lfo = save_a_wobble(&mut wall);
    let saved = as_saved(&wall);
    // Then the wall moves on: a module goes, another comes, a knob glides.
    ask(&mut wall, &Request::Remove { module: lfo }).unwrap();
    ask(
        &mut wall,
        &Request::Add {
            kind: "noise".to_string(),
            name: None,
            place: None,
        },
    )
    .unwrap();
    ask(&mut wall, &glided("vcf1", "cutoff", 3_000.0, 64.0)).unwrap();
    play(&mut wall, &mut engine);
    let before = as_saved(&wall);
    let before_print = wall.patch().fingerprint();
    let before_playing: Vec<String> = playing(&wall).into_iter().map(|(id, _)| id).collect();

    let loaded = load_save(&mut wall, "jam").unwrap();
    let What::Load {
        save,
        backup,
        modules,
        cables,
        fingerprint,
        ..
    } = &loaded.change.what
    else {
        panic!("not a load: {:?}", loaded.change.what);
    };
    assert_eq!(save.as_deref(), Some("jam"));
    assert!(backup.starts_with("before-load-"), "{backup}");
    kept_by_its_bytes(&dir, backup, fingerprint, &before_print);
    // The wall up is named by the load.
    assert_eq!(wall.snapshot().wall, loaded.change.seq);
    assert_eq!(&loaded.backup, backup);
    assert_eq!(*modules as usize, saved.modules.len());
    assert_eq!(*cables as usize, saved.cables.len());
    assert!(
        loaded
            .change
            .summary
            .starts_with("Tom loaded the save \"jam\" ("),
        "{}",
        loaded.change.summary
    );
    // The wall is the save, ids and cable numbers and all...
    assert_eq!(as_saved(&wall), saved);
    assert_eq!(wall.patch().last_seq, loaded.change.seq);
    assert_eq!(wall.revision(), loaded.change.seq, "one change");
    // ...and the wall as it was was saved first, exactly.
    assert_eq!(
        PatchFile {
            last_seq: 0,
            wall: 0,
            ..save_file(&dir, backup)
        },
        before
    );
    // Before the engine has swapped (it fades out first), the snapshot
    // shows the save's knobs, never the old wall's left in the slots...
    plays_at_its_targets(&wall, &saved);
    // ...and after, the engine plays the save, every knob at its target.
    play(&mut wall, &mut engine);
    plays_at_its_targets(&wall, &saved);
    assert_eq!(wall.snapshot().save.unwrap().name, "jam");
    // The log tells the load without the wall it replaced.
    let page: LogPage = serde_json::from_value(
        ask(
            &mut wall,
            &Request::Log {
                before: None,
                limit: Some(1),
            },
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(page.changes, std::slice::from_ref(&loaded.change));

    // Undone, the wall before is back exactly. The wall up is still the
    // save, untouched, so it is kept in that save, not written again.
    let undone = undo_load(&mut wall, loaded.change.seq);
    assert_eq!(undone.change.undoes, Some(loaded.change.seq));
    assert_eq!(undone.backup, "jam");
    assert!(
        undone
            .change
            .summary
            .contains("brought back the wall it replaced"),
        "{}",
        undone.change.summary
    );
    assert_eq!(as_saved(&wall), before);
    assert_eq!(
        PatchFile {
            last_seq: 0,
            wall: 0,
            ..save_file(&dir, &undone.backup)
        },
        saved
    );
    assert_eq!(wall.snapshot().save, None);
    play(&mut wall, &mut engine);
    let back: Vec<String> = playing(&wall).into_iter().map(|(id, _)| id).collect();
    assert_eq!(back, before_playing);
    // And undoing the undo puts the save back up.
    undo_load(&mut wall, undone.change.seq);
    assert_eq!(as_saved(&wall), saved);
    assert_eq!(wall.engine_mut().shared().leaked(), 0);
}

#[test]
fn changes_to_a_wall_that_was_replaced_wait_until_it_is_back() {
    let (mut wall, _engine, _dir) = saving_wall();
    let first = changed(ask(&mut wall, &turn("vcf1", "cutoff", 1_200.0)).unwrap());
    save(&mut wall, "jam", false).unwrap();
    let second = changed(ask(&mut wall, &turn("vcf1", "cutoff", 1_500.0)).unwrap());
    let loaded = load_save(&mut wall, "jam").unwrap();
    // A change to the wall before the load would land on the wrong wall.
    let refused = ask(
        &mut wall,
        &Request::Undo {
            change: second.change.seq,
        },
    )
    .unwrap_err();
    assert_eq!(refused.code, ErrorCode::NotAllowed);
    assert!(
        refused.message.contains(&format!(
            "undo change {} to bring back the wall before it",
            loaded.change.seq
        )),
        "{}",
        refused.message
    );
    // A change to the wall that is up can be undone.
    let third = changed(ask(&mut wall, &turn("vcf1", "cutoff", 400.0)).unwrap());
    ask(
        &mut wall,
        &Request::Undo {
            change: third.change.seq,
        },
    )
    .unwrap();
    // With the load undone, the wall before is up again, and so are its
    // changes.
    undo_load(&mut wall, loaded.change.seq);
    let back = changed(
        ask(
            &mut wall,
            &Request::Undo {
                change: second.change.seq,
            },
        )
        .unwrap(),
    );
    assert!(matches!(back.change.what, What::Turn { to, .. } if (to - 1_200.0).abs() < 1e-9));
    ask(
        &mut wall,
        &Request::Undo {
            change: first.change.seq,
        },
    )
    .unwrap();
}

#[test]
fn a_wall_that_cannot_be_saved_first_is_not_replaced() {
    let (mut wall, _engine, dir) = saving_wall();
    save(&mut wall, "jam", false).unwrap();
    ask(&mut wall, &turn("vcf1", "cutoff", 1_500.0)).unwrap();
    let before = as_saved(&wall);
    let revision = wall.revision();
    // The disk refuses the wall as it is (it is full, say).
    let saves = dir.path().join("saves");
    fail_once(&mut wall, FailAt::WriteTemp);
    let refused = load_save(&mut wall, "jam");
    fail_once(&mut wall, FailAt::WriteTemp);
    let cleared = ask(&mut wall, &Request::New);
    for refused in [refused.map(|_| ()), cleared.map(|_| ())] {
        let refused = refused.unwrap_err();
        assert_eq!(refused.code, ErrorCode::Internal);
        assert!(
            refused.message.contains("nothing was replaced"),
            "{}",
            refused.message
        );
    }
    assert_eq!(as_saved(&wall), before);
    assert_eq!(wall.revision(), revision);
    assert_eq!(std::fs::read_dir(&saves).unwrap().count(), 1);
}

#[test]
fn unknown_broken_and_old_saves_are_told_apart() {
    let (mut wall, _engine, dir) = saving_wall();
    let saves = dir.path().join("saves");
    std::fs::create_dir_all(&saves).unwrap();
    assert_eq!(
        load_save(&mut wall, "nope").unwrap_err().code,
        ErrorCode::UnknownSave
    );
    assert_eq!(
        load_save(&mut wall, "a/b").unwrap_err().code,
        ErrorCode::BadName
    );
    std::fs::write(saves.join("torn.json"), b"{\"version\":").unwrap();
    let broken = load_save(&mut wall, "torn").unwrap_err();
    assert_eq!(broken.code, ErrorCode::BrokenSave);
    assert!(
        broken.message.contains("left as it is"),
        "{}",
        broken.message
    );
    let listed: SavesResult =
        serde_json::from_value(ask(&mut wall, &Request::Saves).unwrap()).unwrap();
    assert!(listed.saves.is_empty());
    assert_eq!(listed.broken.len(), 1);
    assert_eq!(listed.broken[0].name, "torn");
    // A save from an older wall loads brought up to date, and says so; its
    // file is left as it was.
    std::fs::write(saves.join("airports.json"), AIRPORTS).unwrap();
    let loaded = load_save(&mut wall, "Airports").unwrap();
    assert_eq!(wall.patch().modules().len(), 47);
    assert!(
        loaded.notes[0].starts_with("the save was made by an older wall (version 1)"),
        "{:?}",
        loaded.notes
    );
    assert_eq!(
        std::fs::read(saves.join("airports.json")).unwrap(),
        AIRPORTS.as_bytes()
    );
    let listed: SavesResult =
        serde_json::from_value(ask(&mut wall, &Request::Saves).unwrap()).unwrap();
    let airports = listed
        .saves
        .iter()
        .find(|save| save.name == "airports")
        .unwrap();
    assert_eq!((airports.version, airports.modules), (1, 47));
    let backup = listed.saves.iter().find(|save| save.auto).unwrap();
    assert!(backup.name.starts_with("before-load-"));
}

#[test]
fn a_new_wall_is_empty_and_undo_brings_the_old_one_back() {
    let (mut wall, mut engine, _dir) = saving_wall();
    let before = as_saved(&wall);
    let outcome = wall.request("Tom", &Request::New, Instant::now()).unwrap();
    assert!(matches!(
        outcome.event,
        Some(Event::Loaded { save: None, .. })
    ));
    let cleared: LoadResult = serde_json::from_value(outcome.result).unwrap();
    assert!(
        cleared.backup.starts_with("before-new-"),
        "{}",
        cleared.backup
    );
    assert!(
        cleared.change.summary.starts_with("Tom cleared the wall;"),
        "{}",
        cleared.change.summary
    );
    assert!(wall.patch().modules().is_empty() && wall.patch().cables().is_empty());
    assert!(
        (wall.patch().tempo - before.tempo).abs() < 1e-9,
        "the tempo stays"
    );
    play(&mut wall, &mut engine);
    assert!(wall.snapshot().modules.is_empty());
    undo_load(&mut wall, cleared.change.seq);
    assert_eq!(as_saved(&wall), before);
}

#[test]
fn a_wall_of_decisions_and_routers_saves_and_loads_back_exactly() {
    let (mut wall, mut engine, dir) = saving_wall();
    let kinds = [
        "compare", "logic", "branch", "switch", "select", "divide", "math",
    ];
    for kind in kinds {
        let add = Request::Add {
            kind: kind.to_string(),
            name: None,
            place: None,
        };
        let id = changed(ask(&mut wall, &add).unwrap()).module.unwrap();
        assert_eq!(id, format!("{kind}1"));
    }
    for (module, knob, value) in [
        ("compare1", "window", 0.4),
        ("compare1", "hysteresis", 0.1),
        ("logic1", "threshold", 0.3),
        ("branch1", "mode", 1.0),
        ("branch1", "chance", 0.3),
        ("switch1", "positions", 3.0),
        ("switch1", "fade", 0.02),
        ("select1", "select", 2.0),
        ("divide1", "division", 4.0),
        ("math1", "scale_b", -1.0),
        ("math1", "offset_a", 0.25),
    ] {
        ask(&mut wall, &turn(module, knob, value)).unwrap();
    }
    for (from, to, amount) in [
        ("clock1.out", "divide1.in", 1.0),
        ("divide1.out", "branch1.in", 1.0),
        ("branch1.a", "switch1.step", 1.0),
        ("seq1.pitch", "math1.a", 1.0),
        ("math1.sum", "compare1.in", 1.0),
        ("compare1.gate", "logic1.a", 1.0),
        ("logic1.xor", "select1.in1", 1.0),
        ("switch1.out2", "select1.in2", 1.0),
        ("divide1.count", "select1.select", 1.0),
        ("select1.out", "vcf1.cutoff", 0.2),
    ] {
        let patch = Request::Patch {
            from: from.to_string(),
            to: to.to_string(),
            amount: Some(amount),
        };
        ask(&mut wall, &patch).unwrap();
    }
    play(&mut wall, &mut engine);
    save(&mut wall, "routers", false).unwrap();
    let saved = as_saved(&wall);
    let file = save_file(&dir, "routers");
    for kind in kinds {
        let module = file
            .modules
            .iter()
            .find(|module| module.kind == kind)
            .unwrap_or_else(|| panic!("no {kind} in the save"));
        assert_eq!(module.id, format!("{kind}1"));
    }
    let branch = file.modules.iter().find(|m| m.id == "branch1").unwrap();
    assert!((branch.knobs["mode"] - 1.0).abs() < 1e-9);

    // Cleared, then loaded: the wall is the save, and plays it.
    ask(&mut wall, &Request::New).unwrap();
    play(&mut wall, &mut engine);
    assert!(wall.snapshot().modules.is_empty());
    load_save(&mut wall, "routers").unwrap();
    assert_eq!(as_saved(&wall), saved);
    play(&mut wall, &mut engine);
    plays_at_its_targets(&wall, &saved);
    // Nothing in the file needed mending to read back.
    let (_, notes) = Patch::load(&file).unwrap();
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(wall.engine_mut().shared().leaked(), 0);
}

#[test]
fn deleting_a_save_takes_only_the_save() {
    let (mut wall, _engine, dir) = saving_wall();
    save(&mut wall, "Jam", false).unwrap();
    let before = as_saved(&wall);
    let outcome = wall
        .request(
            "Tom",
            &Request::DeleteSave {
                name: "jam".to_string(),
            },
            Instant::now(),
        )
        .unwrap();
    assert!(outcome.change.is_none());
    assert_eq!(
        outcome.event,
        Some(Event::SaveDeleted {
            seat: "Tom".to_string(),
            name: "Jam".to_string(),
        })
    );
    assert!(!dir.path().join("saves").join("Jam.json").exists());
    assert_eq!(wall.snapshot().save, None);
    assert_eq!(as_saved(&wall), before);
    let again = wall
        .request(
            "Tom",
            &Request::DeleteSave {
                name: "jam".to_string(),
            },
            Instant::now(),
        )
        .unwrap_err();
    assert_eq!(again.code, ErrorCode::UnknownSave);
}

#[test]
fn a_load_while_recording_records_on_through_it() {
    let (mut wall, mut engine, _dir) = saving_wall();
    save(&mut wall, "jam", false).unwrap();
    let started = record_result(&record(&mut wall, "Tom", true));
    play(&mut wall, &mut engine);
    let loaded = load_save(&mut wall, "jam").unwrap();
    assert!(
        loaded.notes.iter().any(|note| note.contains("recording")),
        "{:?}",
        loaded.notes
    );
    play(&mut wall, &mut engine);
    let recording = wall.snapshot().recording.unwrap();
    assert_eq!(Some(recording.path), started.path, "the same file");
    // One file, all of it: from before the load to after it.
    let stopped = record_result(&record(&mut wall, "Tom", false));
    assert_eq!(stopped.path, started.path);
    assert!(stopped.seconds > 0.45, "{}", stopped.seconds);
    assert_eq!(stopped.dropped, 0);
}

fn undo(wall: &mut Wall, change: u64) -> Result<serde_json::Value, WallError> {
    ask(wall, &Request::Undo { change })
}

#[test]
fn undoing_the_same_load_twice_holds_only_the_changes_its_wall_had() {
    let (mut wall, _engine, _dir) = saving_wall();
    // On the first wall: a change before the load.
    let early = changed(ask(&mut wall, &turn("vcf1", "cutoff", 1_100.0)).unwrap());
    save(&mut wall, "jam", false).unwrap();
    let late = changed(ask(&mut wall, &turn("vcf1", "resonance", 0.2)).unwrap());
    let before = as_saved(&wall);
    let loaded = load_save(&mut wall, "jam").unwrap();
    // Undone: the first wall is back, and its changes hold again.
    undo_load(&mut wall, loaded.change.seq);
    assert_eq!(as_saved(&wall), before);
    // A change made to the wall brought back...
    let meanwhile = changed(ask(&mut wall, &turn("vcf1", "drive", 0.4)).unwrap());
    // ...and the same load undone again: the first wall as it was before
    // the load, which never had that change.
    let again = undo_load(&mut wall, loaded.change.seq);
    assert_eq!(again.change.undoes, Some(loaded.change.seq));
    assert_eq!(as_saved(&wall), before);
    let refused = undo(&mut wall, meanwhile.change.seq).unwrap_err();
    assert_eq!(refused.code, ErrorCode::NotAllowed, "{}", refused.message);
    // The changes made before the load hold on it, newest first.
    undo(&mut wall, late.change.seq).unwrap();
    undo(&mut wall, early.change.seq).unwrap();
    let vcf = wall.patch().module("vcf1").unwrap();
    let cutoff = vcf.knobs[vcf.kind.spec().knob_index("cutoff").unwrap()];
    assert!((cutoff - 900.0).abs() < 1e-3, "{cutoff}");
    // Undoing the second undo puts up the wall the first undo had put up,
    // with the change made to it.
    undo_load(&mut wall, again.change.seq);
    let drive = changed(undo(&mut wall, meanwhile.change.seq).unwrap());
    assert!(matches!(drive.change.what, What::Turn { ref knob, .. } if knob == "drive"));
}

#[test]
fn a_load_lands_whole_on_disk_and_survives_a_crash_after_it() {
    let (mut wall, _engine, dir) = saving_wall();
    save(&mut wall, "jam", false).unwrap();
    ask(&mut wall, &turn("vcf1", "cutoff", 1_500.0)).unwrap();
    let before = as_saved(&wall);
    let loaded = load_save(&mut wall, "jam").unwrap();
    // With no housekeeping since: the backup, the log line and the new
    // patch.json are all on disk already.
    let store = Store::peek(dir.path()).unwrap();
    let crate::store::Loaded::Patch { patch: on_disk, .. } = store.load_patch().unwrap() else {
        panic!("no patch on disk");
    };
    assert_eq!(*on_disk, *wall.patch(), "patch.json is the loaded wall");
    assert_eq!(on_disk.wall, loaded.change.seq);
    let logged = store.load_log().unwrap().changes;
    assert_eq!(logged.last(), Some(&loaded.change));
    let What::Load { backup, .. } = &loaded.change.what else {
        panic!("not a load");
    };
    assert_eq!(
        PatchFile {
            last_seq: 0,
            wall: 0,
            ..save_file(&dir, backup)
        },
        before
    );
    // The daemon dies here and starts again from the disk alone: the load
    // undoes exactly.
    drop(wall);
    let (mut wall, _engine) = restart(&dir);
    assert_eq!(wall.snapshot().wall, loaded.change.seq);
    let logged = store.load_log().unwrap().changes;
    assert!(
        !logged
            .iter()
            .any(|change| matches!(change.what, What::Abandoned { .. } | What::Resumed { .. })),
        "nothing was left out"
    );
    undo_load(&mut wall, loaded.change.seq);
    assert_eq!(as_saved(&wall), before);
}

/// A wall starting again as the daemon does from what is on disk in
/// `dir`: patch.json and the log.
fn restart(dir: &tempfile::TempDir) -> (Wall, Engine) {
    let store = Store::peek(dir.path()).unwrap();
    let Loaded::Patch { patch, .. } = store.load_patch().unwrap() else {
        panic!("no patch.json");
    };
    restart_on(dir, *patch, false)
}

/// A wall starting again as the daemon does on `patch` (`seeded` afresh,
/// or patch.json) with the log in `dir`.
fn restart_on(dir: &tempfile::TempDir, mut patch: Patch, seeded: bool) -> (Wall, Engine) {
    let (engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let mut store = Store::open(dir.path()).unwrap();
    let mut history = store.load_log().unwrap().changes;
    resume(&mut store, &mut patch, &mut history, seeded, None, None).unwrap();
    (
        Wall::new(patch, control, None, history, Some(store)),
        engine,
    )
}

/// The notes the wall made as it started again ([`What::Resumed`]), in
/// the log in `dir`.
fn resumed_notes(dir: &tempfile::TempDir) -> Vec<Change> {
    Store::peek(dir.path())
        .unwrap()
        .load_log()
        .unwrap()
        .changes
        .into_iter()
        .filter(|change| matches!(change.what, What::Resumed { .. }))
        .collect()
}

/// patch.json in `dir` made `patch`, as a crash or a restored copy leaves
/// it.
fn put_patch_json(dir: &tempfile::TempDir, patch: &Patch) {
    std::fs::write(
        dir.path().join("patch.json"),
        crate::store::encode(patch).unwrap(),
    )
    .unwrap();
}

/// The wall `before`, as patch.json held it just before change `load`.
fn as_it_was(before: &PatchFile, load: u64) -> Patch {
    Patch::load(&PatchFile {
        last_seq: load - 1,
        ..before.clone()
    })
    .unwrap()
    .0
}

#[test]
fn a_crash_between_the_log_line_and_patch_json_leaves_the_load_out() {
    let (mut wall, _engine, dir) = saving_wall();
    save(&mut wall, "jam", false).unwrap();
    let made_before = changed(ask(&mut wall, &turn("vcf1", "cutoff", 1_500.0)).unwrap());
    let before = as_saved(&wall);
    let loaded = load_save(&mut wall, "jam").unwrap();
    let load = loaded.change.seq;
    drop(wall);
    // The disk as a crash right after the log line leaves it: the load is
    // in the log, and patch.json still holds the wall before it.
    put_patch_json(&dir, &as_it_was(&before, load));
    let (mut wall, _engine) = restart(&dir);
    assert_eq!(as_saved(&wall), before, "the wall before plays");
    // Said in the log, by the wall itself.
    let page: LogPage = serde_json::from_value(
        ask(
            &mut wall,
            &Request::Log {
                before: None,
                limit: Some(1),
            },
        )
        .unwrap(),
    )
    .unwrap();
    let note = &page.changes[0];
    assert_eq!(note.seat, LOADER_SEAT);
    assert_eq!(
        note.what,
        What::Resumed {
            from: Some(0),
            through: load - 1,
            left_out: vec![load],
        }
    );
    assert!(
        note.summary.contains(&format!(
            "the load in change {load} never reached it and is left out"
        )),
        "{}",
        note.summary
    );
    // The wall up is a wall of its own, named by the note.
    assert_eq!(wall.snapshot().wall, note.seq);
    // The note itself is refused for what it is, not for the wall.
    let refused = undo(&mut wall, note.seq).unwrap_err();
    assert_eq!(refused.code, ErrorCode::NotAllowed);
    assert_eq!(
        refused.message,
        "the wall starting again from patch.json cannot be undone"
    );
    // The load is left out: it cannot be undone, and the change made
    // before it is a change to the wall up again.
    let refused = undo(&mut wall, load).unwrap_err();
    assert_eq!(refused.code, ErrorCode::NotAllowed);
    assert!(
        refused.message.contains("did not come back up"),
        "{}",
        refused.message
    );
    undo(&mut wall, made_before.change.seq).unwrap();
    let after = changed(ask(&mut wall, &turn("vcf1", "drive", 0.3)).unwrap());
    undo(&mut wall, after.change.seq).unwrap();
    // Starting again on the wall as it was saved says nothing more.
    wall.save(Instant::now());
    drop(wall);
    let (wall, _engine) = restart(&dir);
    drop(wall);
    assert_eq!(resumed_notes(&dir).len(), 1);
}

#[test]
fn a_start_that_cannot_say_what_it_starts_on_does_not_start() {
    let (mut wall, _engine, dir) = saving_wall();
    ask(&mut wall, &turn("vcf1", "cutoff", 1_500.0)).unwrap();
    drop(wall);
    // Each start is a daemon of its own, reading the disk afresh.
    for at in [FailAt::AppendLog, FailAt::SyncLog, FailAt::WritePatch] {
        let mut store = Store::peek(dir.path()).unwrap();
        let mut history = store.load_log().unwrap().changes;
        store.fail_once(at);
        let failed = resume(
            &mut store,
            &mut seed().unwrap(),
            &mut history,
            true,
            None,
            None,
        );
        assert!(failed.is_err(), "{at:?}");
        assert!(
            !dir.path().join("patch.json").exists(),
            "{at:?}: patch.json is never ahead of the log"
        );
    }
    // With the disk back, it starts: what the failed starts got into the
    // log is read as it is, each a start on a seeded wall holding nothing.
    let (mut wall, _engine) = restart_on(&dir, seed().unwrap(), true);
    assert!(dir.path().join("patch.json").exists());
    let notes = resumed_notes(&dir);
    assert!(
        notes
            .iter()
            .all(|note| matches!(note.what, What::Resumed { from: None, .. }))
    );
    assert_eq!(wall.snapshot().wall, notes.last().unwrap().seq);
    let seqs: Vec<u64> = Store::peek(dir.path())
        .unwrap()
        .load_log()
        .unwrap()
        .changes
        .iter()
        .map(|change| change.seq)
        .collect();
    assert!(seqs.windows(2).all(|pair| pair[0] < pair[1]), "{seqs:?}");
    assert_eq!(undo(&mut wall, 1).unwrap_err().code, ErrorCode::NotAllowed);
}

#[test]
fn a_seeded_wall_holds_no_earlier_change() {
    let (mut wall, _engine, dir) = saving_wall();
    save(&mut wall, "jam", false).unwrap();
    let turned = changed(ask(&mut wall, &turn("vcf1", "cutoff", 1_500.0)).unwrap());
    let loaded = load_save(&mut wall, "jam").unwrap();
    let after = changed(ask(&mut wall, &turn("vcf1", "drive", 0.3)).unwrap());
    wall.save(Instant::now());
    drop(wall);
    // patch.json was lost and the wall seeded afresh: its wall number (0)
    // is one the log knows, but it is not that wall.
    let (mut wall, _engine) = restart_on(&dir, seed().unwrap(), true);
    let notes = resumed_notes(&dir);
    assert_eq!(notes.len(), 1);
    let What::Resumed {
        from,
        through,
        left_out,
    } = &notes[0].what
    else {
        panic!("{:?}", notes[0]);
    };
    assert_eq!((*from, *through), (None, 0));
    assert_eq!(left_out, &[loaded.change.seq]);
    assert_eq!(wall.snapshot().wall, notes[0].seq);
    for earlier in [turned.change.seq, after.change.seq] {
        let refused = undo(&mut wall, earlier).unwrap_err();
        assert_eq!(refused.code, ErrorCode::NotAllowed, "{}", refused.message);
        assert!(
            refused.message.contains("replaced since"),
            "{}",
            refused.message
        );
    }
    // Its loads are left out, not said never to have happened.
    let refused = undo(&mut wall, loaded.change.seq).unwrap_err();
    assert!(
        refused.message.contains("did not come back up"),
        "{}",
        refused.message
    );
    // Changes made to the seeded wall undo as ever.
    let fresh = changed(ask(&mut wall, &turn("vcf1", "cutoff", 900.0)).unwrap());
    undo(&mut wall, fresh.change.seq).unwrap();
}

#[test]
fn a_patch_json_rolled_back_holds_only_what_it_had() {
    let (mut wall, _engine, dir) = saving_wall();
    save(&mut wall, "jam", false).unwrap();
    save(&mut wall, "mix", false).unwrap();
    let kept = changed(ask(&mut wall, &turn("vcf1", "cutoff", 1_500.0)).unwrap());
    wall.save(Instant::now());
    // Tom keeps a copy of patch.json here...
    let copy = std::fs::read(dir.path().join("patch.json")).unwrap();
    let lost = changed(ask(&mut wall, &turn("vcf1", "drive", 0.3)).unwrap());
    // ...and the wall goes on, through several loads.
    let first = load_save(&mut wall, "jam").unwrap();
    ask(&mut wall, &turn("vcf1", "cutoff", 700.0)).unwrap();
    let second = load_save(&mut wall, "mix").unwrap();
    wall.save(Instant::now());
    drop(wall);
    // Then puts the copy back.
    std::fs::write(dir.path().join("patch.json"), &copy).unwrap();
    let (mut wall, _engine) = restart(&dir);
    let notes = resumed_notes(&dir);
    assert_eq!(notes.len(), 1);
    assert_eq!(
        notes[0].what,
        What::Resumed {
            from: Some(0),
            through: kept.change.seq,
            left_out: vec![first.change.seq, second.change.seq],
        }
    );
    // What the copy had undoes; what it never had does not.
    let refused = undo(&mut wall, lost.change.seq).unwrap_err();
    assert_eq!(refused.code, ErrorCode::NotAllowed);
    for load in [first.change.seq, second.change.seq] {
        let refused = undo(&mut wall, load).unwrap_err();
        assert!(
            refused.message.contains("did not come back up"),
            "{}",
            refused.message
        );
    }
    undo(&mut wall, kept.change.seq).unwrap();
    // Neither load is said never to have happened.
    assert!(
        !Store::peek(dir.path())
            .unwrap()
            .load_log()
            .unwrap()
            .changes
            .iter()
            .any(|change| matches!(change.what, What::Abandoned { .. }))
    );
    // A second start, with nothing new to say, says nothing.
    wall.save(Instant::now());
    drop(wall);
    let (wall, _engine) = restart(&dir);
    drop(wall);
    assert_eq!(resumed_notes(&dir).len(), 1);
}

#[test]
fn a_patch_json_of_a_wall_the_log_does_not_know_holds_nothing() {
    let (mut wall, _engine, dir) = saving_wall();
    let turned = changed(ask(&mut wall, &turn("vcf1", "cutoff", 1_500.0)).unwrap());
    wall.save(Instant::now());
    drop(wall);
    // A patch.json from another wall's state (its wall is no load here).
    let mut stranger = seed().unwrap();
    stranger.wall = 4_000;
    stranger.last_seq = 4_000;
    put_patch_json(&dir, &stranger);
    let (mut wall, _engine) = restart(&dir);
    let notes = resumed_notes(&dir);
    assert_eq!(notes.len(), 1);
    assert!(matches!(notes[0].what, What::Resumed { from: None, .. }));
    assert_eq!(notes[0].seq, 4_001, "numbers carry on past the file's");
    let refused = undo(&mut wall, turned.change.seq).unwrap_err();
    assert_eq!(refused.code, ErrorCode::NotAllowed);
}

#[test]
fn a_load_that_may_be_in_the_log_is_said_not_to_have_happened() {
    let (mut wall, _engine, dir) = saving_wall();
    save(&mut wall, "jam", false).unwrap();
    ask(&mut wall, &turn("vcf1", "cutoff", 1_500.0)).unwrap();
    let before = as_saved(&wall);
    // The load's line is written, but its sync fails: it may be on disk.
    fail_once(&mut wall, FailAt::SyncLog);
    let refused = load_save(&mut wall, "jam").unwrap_err();
    assert_eq!(refused.code, ErrorCode::Internal);
    assert!(
        refused.message.contains("nothing was replaced"),
        "{}",
        refused.message
    );
    assert_eq!(as_saved(&wall), before, "the wall plays on");
    // Said at once, durably, after the load's line.
    let logged = Store::peek(dir.path()).unwrap().load_log().unwrap().changes;
    let [.., load, note] = logged.as_slice() else {
        panic!("{logged:?}");
    };
    assert!(matches!(load.what, What::Load { .. }));
    assert_eq!(note.what, What::Abandoned { change: load.seq });
    assert_eq!(note.seat, LOADER_SEAT);
    // Told to everyone at the next tick.
    let told = wall.tick(Instant::now());
    assert!(told.iter().any(|event| matches!(
        event,
        Event::Change { change } if change.seq == note.seq
    )));
    // Neither it nor the note undoes; changes go on, to the wall up.
    let refused = undo(&mut wall, load.seq).unwrap_err();
    assert!(
        refused.message.contains("did not happen"),
        "{}",
        refused.message
    );
    assert_eq!(
        undo(&mut wall, note.seq).unwrap_err().code,
        ErrorCode::NotAllowed
    );
    let next = changed(ask(&mut wall, &turn("vcf1", "drive", 0.3)).unwrap());
    assert_eq!(next.change.seq, note.seq + 1);
    undo(&mut wall, next.change.seq).unwrap();
    // Starting again: the load stays not having happened, and nothing
    // else is left out.
    wall.save(Instant::now());
    drop(wall);
    let (mut wall, _engine) = restart(&dir);
    assert!(resumed_notes(&dir).is_empty());
    assert_eq!(wall.snapshot().wall, 0, "the wall before the load is up");
    let refused = undo(&mut wall, load.seq).unwrap_err();
    assert!(
        refused.message.contains("did not happen"),
        "{}",
        refused.message
    );
}

#[test]
fn a_note_the_log_cannot_take_is_owed_and_nothing_moves_until_it_is_paid() {
    let (mut wall, _engine, dir) = saving_wall();
    save(&mut wall, "jam", false).unwrap();
    ask(&mut wall, &turn("vcf1", "cutoff", 1_500.0)).unwrap();
    wall.save(Instant::now());
    let before = as_saved(&wall);
    let patch_json = std::fs::read(dir.path().join("patch.json")).unwrap();
    // The load's line fails, then the note saying so, then the next try.
    for _ in 0..3 {
        fail_once(&mut wall, FailAt::AppendLog);
    }
    load_save(&mut wall, "jam").unwrap_err();
    let refused = ask(&mut wall, &turn("vcf1", "drive", 0.3)).unwrap_err();
    assert_eq!(refused.code, ErrorCode::Internal);
    assert!(
        refused.message.contains("nothing was changed"),
        "{}",
        refused.message
    );
    assert_eq!(as_saved(&wall), before, "the turn was put back");
    // patch.json is never ahead of the log: not saved while it owes.
    fail_once(&mut wall, FailAt::AppendLog);
    wall.save(Instant::now());
    assert_eq!(
        std::fs::read(dir.path().join("patch.json")).unwrap(),
        patch_json
    );
    // The log takes lines again: the note first, then the change.
    let next = changed(ask(&mut wall, &turn("vcf1", "drive", 0.3)).unwrap());
    let logged = Store::peek(dir.path()).unwrap().load_log().unwrap().changes;
    let [.., note, last] = logged.as_slice() else {
        panic!("{logged:?}");
    };
    assert!(matches!(note.what, What::Abandoned { .. }), "{note:?}");
    assert_eq!(last, &next.change);
    assert!(note.seq < last.seq);
    wall.save(Instant::now());
    assert_ne!(
        std::fs::read(dir.path().join("patch.json")).unwrap(),
        patch_json
    );
}

#[test]
fn a_change_the_log_cannot_take_is_not_made() {
    let (mut wall, _engine, dir) = saving_wall();
    let requests = every_kind_of_change(&mut wall);
    let before = as_saved(&wall);
    let modules = wall.patch().modules().len();
    for request in &requests {
        fail_once(&mut wall, FailAt::AppendLog);
        let refused = ask(&mut wall, request).unwrap_err();
        assert_eq!(refused.code, ErrorCode::Internal, "{request:?}");
        // Exactly as it was: knobs, cables, ids and their numbering, rows,
        // words and dye.
        assert_eq!(as_saved(&wall), before, "{request:?}");
    }
    assert_eq!(wall.patch().modules().len(), modules);
    assert_eq!(wall.snapshot().modules.len(), modules);
    // Each is said not to have happened; the next change goes through.
    let next = changed(ask(&mut wall, &turn("vco1", "tune", 3.0)).unwrap());
    let logged = Store::peek(dir.path()).unwrap().load_log().unwrap().changes;
    let abandoned: Vec<u64> = logged
        .iter()
        .filter_map(|change| match change.what {
            What::Abandoned { change } => Some(change),
            _ => None,
        })
        .collect();
    assert_eq!(abandoned.len(), requests.len());
    assert!(abandoned.iter().all(|seq| *seq < next.change.seq));
    assert_eq!(logged.last(), Some(&next.change));
    // And what was refused goes through now, numbered after.
    let added = ask(
        &mut wall,
        &Request::Add {
            kind: "lfo".to_string(),
            name: None,
            place: None,
        },
    )
    .unwrap();
    assert!(changed(added).change.seq > next.change.seq);
}

/// A request of every kind of change the patch makes, after making on
/// `wall` the changes they undo: a turn, a cable, a removal, an addition,
/// an unplugging and a restoring.
fn every_kind_of_change(wall: &mut Wall) -> Vec<Request> {
    // Something of every kind to undo, and a cable to unplug.
    let turned = changed(ask(wall, &turn("vcf1", "cutoff", 1_500.0)).unwrap());
    let plugged = changed(
        ask(
            wall,
            &Request::Patch {
                from: "lfo1.out".to_string(),
                to: "vco1.fm".to_string(),
                amount: Some(0.25),
            },
        )
        .unwrap(),
    );
    let cable = plugged.cable.unwrap();
    let removed = changed(
        ask(
            wall,
            &Request::Remove {
                module: "lfo2".to_string(),
            },
        )
        .unwrap(),
    );
    let added = changed(
        ask(
            wall,
            &Request::Add {
                kind: "lfo".to_string(),
                name: None,
                place: None,
            },
        )
        .unwrap(),
    );
    let unplugged = changed(
        ask(
            wall,
            &Request::Unpatch {
                cable: None,
                to: Some("vcf1.resonance".to_string()),
            },
        )
        .unwrap(),
    );
    // A module taken away and brought back: its restoring, to undo.
    let gone = changed(
        ask(
            wall,
            &Request::Remove {
                module: "mix1".to_string(),
            },
        )
        .unwrap(),
    );
    let restored = changed(undo(wall, gone.change.seq).unwrap());
    assert!(matches!(restored.change.what, What::Restore { .. }));
    vec![
        turn("vco1", "tune", 3.0),
        Request::Add {
            kind: "lfo".to_string(),
            name: None,
            place: None,
        },
        Request::Patch {
            from: "lfo1.out".to_string(),
            to: "vco2.fm".to_string(),
            amount: None,
        },
        Request::Unpatch {
            cable: Some(cable),
            to: None,
        },
        Request::Remove {
            module: "vco1".to_string(),
        },
        // Undoing a turn, a cable, a removal, an addition, an unplugging
        // and a restoring.
        Request::Undo {
            change: turned.change.seq,
        },
        Request::Undo {
            change: plugged.change.seq,
        },
        Request::Undo {
            change: removed.change.seq,
        },
        Request::Undo {
            change: added.change.seq,
        },
        Request::Undo {
            change: unplugged.change.seq,
        },
        Request::Undo {
            change: restored.change.seq,
        },
    ]
}

#[test]
fn a_log_that_cannot_be_synced_keeps_patch_json_where_it_was() {
    let (mut wall, _engine, dir) = saving_wall();
    ask(&mut wall, &turn("vcf1", "cutoff", 1_500.0)).unwrap();
    wall.save(Instant::now());
    let patch_json = std::fs::read(dir.path().join("patch.json")).unwrap();
    ask(&mut wall, &turn("vcf1", "drive", 0.3)).unwrap();
    fail_once(&mut wall, FailAt::SyncLog);
    wall.save(Instant::now());
    assert_eq!(
        std::fs::read(dir.path().join("patch.json")).unwrap(),
        patch_json,
        "not ahead of the log"
    );
    // Still unsaved: the next save writes it.
    let due = Instant::now() + SAVE_INTERVAL;
    wall.tick(due);
    assert_ne!(
        std::fs::read(dir.path().join("patch.json")).unwrap(),
        patch_json
    );
}

#[test]
fn a_load_whose_wall_is_not_in_patch_json_yet_says_so_and_saves_it_next() {
    let (mut wall, _engine, dir) = saving_wall();
    save(&mut wall, "jam", false).unwrap();
    ask(&mut wall, &turn("vcf1", "cutoff", 1_500.0)).unwrap();
    wall.save(Instant::now());
    let patch_json = std::fs::read(dir.path().join("patch.json")).unwrap();
    fail_once(&mut wall, FailAt::RenamePatch);
    let loaded = load_save(&mut wall, "jam").unwrap();
    assert!(
        loaded
            .notes
            .iter()
            .any(|note| note.contains("could not be written to patch.json yet")),
        "{:?}",
        loaded.notes
    );
    assert_eq!(wall.snapshot().wall, loaded.change.seq);
    assert_eq!(
        std::fs::read(dir.path().join("patch.json")).unwrap(),
        patch_json
    );
    let due = Instant::now() + SAVE_INTERVAL;
    wall.tick(due);
    let store = Store::peek(dir.path()).unwrap();
    let Loaded::Patch { patch, .. } = store.load_patch().unwrap() else {
        panic!("no patch.json");
    };
    assert_eq!(patch.wall, loaded.change.seq);
    // A start from here has nothing to say.
    drop(wall);
    let (wall, _engine) = restart(&dir);
    drop(wall);
    assert!(resumed_notes(&dir).is_empty());
}

#[test]
fn a_seat_that_is_not_a_console_has_a_load_allowance() {
    let (mut wall, _engine, _dir) = saving_wall();
    save(&mut wall, "jam", false).unwrap();
    let now = Instant::now();
    let load_as = |wall: &mut Wall, seat: &str, console: bool, at: Instant| {
        let request = Request::Load {
            name: "jam".to_string(),
        };
        wall.request_as(seat, console, &request, at)
    };
    let mut loads = Vec::new();
    for _ in 0..2 {
        let outcome = load_as(&mut wall, "Ava", false, now).unwrap();
        loads.push(outcome.change.unwrap().seq);
    }
    // Undoing a load and a new wall count too.
    wall.request_as("Ava", false, &Request::Undo { change: loads[1] }, now)
        .unwrap();
    let refused = wall
        .request_as("Ava", false, &Request::New, now)
        .unwrap_err();
    assert_eq!(refused.code, ErrorCode::SlowDown);
    assert!(
        refused.message.contains("replaced the wall"),
        "{}",
        refused.message
    );
    let refused = load_as(&mut wall, "Ava", false, now).unwrap_err();
    assert_eq!(refused.code, ErrorCode::SlowDown);
    // Her other changes are not held to it.
    wall.request_as("Ava", false, &turn("vcf1", "cutoff", 900.0), now)
        .unwrap();
    // The console is not held to it; a minute later Ava may again.
    for _ in 0..5 {
        load_as(&mut wall, "Tom", true, now).unwrap();
    }
    let later = now + Duration::from_secs(60);
    let outcome = load_as(&mut wall, "Ava", false, later).unwrap();
    let change = outcome.change.unwrap();
    // A seat's load says it was not the console's.
    assert!(matches!(
        change.what,
        What::Load {
            by_console: false,
            ..
        }
    ));
}

#[test]
fn a_crash_before_the_log_line_leaves_the_wall_and_a_spare_save() {
    let (mut wall, _engine, dir) = saving_wall();
    save(&mut wall, "jam", false).unwrap();
    ask(&mut wall, &turn("vcf1", "cutoff", 1_500.0)).unwrap();
    let before = as_saved(&wall);
    let loaded = load_save(&mut wall, "jam").unwrap();
    drop(wall);
    // The disk as a crash before the log line leaves it: the backup is in
    // place, the log and patch.json are as they were.
    let log = dir.path().join("log.jsonl");
    let text = std::fs::read_to_string(&log).unwrap();
    let kept: Vec<&str> = text.lines().filter(|line| !line.is_empty()).collect();
    let (last, rest) = kept.split_last().unwrap();
    assert!(last.contains("\"op\":\"load\""), "{last}");
    std::fs::write(&log, format!("{}\n", rest.join("\n"))).unwrap();
    let load = loaded.change.seq;
    put_patch_json(&dir, &as_it_was(&before, load));
    let (mut wall, _engine) = restart(&dir);
    assert_eq!(as_saved(&wall), before);
    assert!(resumed_notes(&dir).is_empty(), "nothing to say");
    let logged = Store::peek(dir.path()).unwrap().load_log().unwrap().changes;
    assert!(
        logged.iter().all(|change| change.seq < load),
        "no trace of it"
    );
    assert_eq!(
        undo(&mut wall, load).unwrap_err().code,
        ErrorCode::UnknownChange
    );
    // The backup is a save nothing needs, let go with the others in time.
    assert!(
        dir.path()
            .join("saves")
            .join(format!("{}.json", loaded.backup))
            .exists()
    );
    // And the next change takes the number the lost load had.
    let next = changed(ask(&mut wall, &turn("vcf1", "drive", 0.3)).unwrap());
    assert_eq!(next.change.seq, load);
}

#[test]
fn a_load_that_cannot_reach_the_disk_replaces_nothing_and_says_why() {
    let (mut wall, _engine, dir) = saving_wall();
    save(&mut wall, "jam", false).unwrap();
    ask(&mut wall, &turn("vcf1", "cutoff", 1_500.0)).unwrap();
    let before = as_saved(&wall);
    let revision = wall.revision();
    let log_before = std::fs::read(dir.path().join("log.jsonl")).unwrap();
    // The disk refuses the new wall's temporary file.
    fail_once(&mut wall, FailAt::WriteTemp);
    let refused = load_save(&mut wall, "jam").unwrap_err();
    assert_eq!(refused.code, ErrorCode::Internal);
    assert!(
        refused.message.contains("nothing was replaced"),
        "{}",
        refused.message
    );
    assert_eq!(as_saved(&wall), before);
    assert_eq!(wall.revision(), revision);
    assert_eq!(
        std::fs::read(dir.path().join("log.jsonl")).unwrap(),
        log_before
    );
    // Nothing half-written is left among the saves.
    let leftovers: Vec<std::path::PathBuf> = std::fs::read_dir(dir.path().join("saves"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    // With the disk back, the same load goes, and its number is the next.
    let loaded = load_save(&mut wall, "jam").unwrap();
    assert_eq!(loaded.change.seq, revision + 1);
}

#[test]
fn a_load_kept_by_an_older_build_still_undoes() {
    let dir = tempfile::tempdir().unwrap();
    let saves = dir.path().join("saves");
    std::fs::create_dir_all(&saves).unwrap();
    // An older wall kept the wall it replaced in a version-1 save, and
    // logged the load by that file's bytes. This build reads the file
    // through its migration into another shape entirely.
    std::fs::write(saves.join("before-load-20260927-120000.json"), AIRPORTS).unwrap();
    let load = Change {
        seq: 5,
        at: "2026-09-27T02:00:00Z".to_string(),
        seat: "Tom".to_string(),
        what: What::Load {
            save: Some("jam".to_string()),
            backup: "before-load-20260927-120000".to_string(),
            fingerprint: hash_bytes(AIRPORTS.as_bytes()),
            modules: 9,
            cables: 8,
            tempo: 120.0,
            by_console: true,
        },
        summary: "Tom loaded the save \"jam\"".to_string(),
        undoes: None,
    };
    let mut up = seed().unwrap();
    up.last_seq = 5;
    up.wall = 5;
    let (engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let store = Store::peek(dir.path()).unwrap();
    let mut wall = Wall::new(up, control, None, vec![load], Some(store));
    drop(engine);
    undo_load(&mut wall, 5);
    assert_eq!(wall.patch().modules().len(), 47, "Airports is back");
    // What this build makes of that wall is not the file it came from.
    let rewritten = crate::store::encode(wall.patch()).unwrap();
    assert_ne!(hash_bytes(&rewritten), hash_bytes(AIRPORTS.as_bytes()));
}

#[test]
fn a_save_changed_from_outside_is_never_trusted_to_hold_the_wall() {
    let (mut wall, _engine, dir) = saving_wall();
    save(&mut wall, "b", false).unwrap();
    save(&mut wall, "a", false).unwrap();
    let before = as_saved(&wall);
    // The wall is "a", untouched; then "a" is replaced from outside.
    let outside = crate::store::encode(&Patch::empty(90.0)).unwrap();
    std::fs::write(dir.path().join("saves").join("a.json"), &outside).unwrap();
    let loaded = load_save(&mut wall, "b").unwrap();
    assert!(
        loaded.backup.starts_with("before-load-"),
        "{}",
        loaded.backup
    );
    undo_load(&mut wall, loaded.change.seq);
    assert_eq!(as_saved(&wall), before, "the wall that played is back");
}

#[test]
fn a_seat_that_is_not_a_console_has_a_save_allowance_and_a_cap() {
    let (mut wall, _engine, dir) = saving_wall();
    let now = Instant::now();
    let save_as = |wall: &mut Wall, seat: &str, console: bool, name: &str, at: Instant| {
        let request = Request::Save {
            name: name.to_string(),
            overwrite: false,
        };
        wall.request_as(seat, console, &request, at)
    };
    for n in 0..3 {
        save_as(&mut wall, "Ava", false, &format!("ava {n}"), now).unwrap();
    }
    let refused = save_as(&mut wall, "Ava", false, "ava 3", now).unwrap_err();
    assert_eq!(refused.code, ErrorCode::SlowDown);
    // A bad name is refused as that, not counted.
    let bad = save_as(&mut wall, "Ava", false, "../x", now).unwrap_err();
    assert_eq!(bad.code, ErrorCode::BadName);
    // A console is not held to it; a minute later Ava may save again.
    for n in 0..5 {
        save_as(&mut wall, "Tom", true, &format!("tom {n}"), now).unwrap();
    }
    let later = now + Duration::from_secs(60);
    save_as(&mut wall, "Ava", false, "ava 3", later).unwrap();
    // The saves at their cap: a seat may make no more; Tom may.
    let much_later = later + Duration::from_secs(600);
    save_as(&mut wall, "Ava", false, "ava 4", much_later).unwrap();
    save_as(&mut wall, "Ava", false, "ava 5", much_later).unwrap();
    let saves = dir.path().join("saves");
    let bytes = std::fs::read(saves.join("ava 0.json")).unwrap();
    for n in 0..MAX_SAVES {
        std::fs::write(saves.join(format!("filler {n}.json")), &bytes).unwrap();
    }
    let full = save_as(&mut wall, "Ava", false, "one more", much_later).unwrap_err();
    assert_eq!(full.code, ErrorCode::Full);
    assert!(
        full.message.contains("Tom's console can delete some"),
        "{}",
        full.message
    );
    // A name taken is refused as that.
    let taken = save_as(&mut wall, "Ava", false, "ava 4", much_later).unwrap_err();
    assert_eq!(taken.code, ErrorCode::SaveExists);
    save_as(&mut wall, "Tom", true, "still mine", much_later).unwrap();
    // Neither refusal cost Ava her last save: with room again, it goes.
    for n in 0..MAX_SAVES {
        std::fs::remove_file(saves.join(format!("filler {n}.json"))).unwrap();
    }
    save_as(&mut wall, "Ava", false, "one more", much_later).unwrap();
    let refused = save_as(&mut wall, "Ava", false, "and another", much_later).unwrap_err();
    assert_eq!(refused.code, ErrorCode::SlowDown);
}

/// A link to a desk on a socket in `dir` that takes one instrument at
/// `bpm`, and what to keep for as long as the link should stay up: its
/// audio half and the desk's end.
fn a_desk(
    dir: &std::path::Path,
    bpm: f32,
) -> (
    HubLink,
    (
        kazoo_core::ipc::link::HubLinkAudio,
        std::os::unix::net::UnixStream,
    ),
) {
    use kazoo_core::host::link::connect_config_or_direct;
    use kazoo_core::ipc::link::{HubAddress, LinkConfig};
    use kazoo_core::ipc::protocol::FrameBuffer;
    use kazoo_core::ipc::types::{MSG_REGISTER, MSG_REGISTERED, RegisteredMsg};
    let path = dir.join("desk.sock");
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    let linking = std::thread::spawn(move || {
        connect_config_or_direct(LinkConfig {
            address: HubAddress::Socket(path),
            ..LinkConfig::new("kazoo-wall", 2, 48_000, 256)
        })
    });
    let (mut stream, _) = listener.accept().unwrap();
    let mut buf = FrameBuffer::new();
    let header = buf.read_frame(&mut stream).unwrap();
    assert_eq!(header.msg_type, MSG_REGISTER);
    let reply = RegisteredMsg {
        strips: Vec::new(),
        strip_index: 1,
        hub_sample_rate: 48_000,
        hub_buffer_size: 256,
        transport_state: 0,
        bpm,
        position: 0,
    };
    reply.encode(buf.payload_mut());
    buf.write_frame(MSG_REGISTERED, 0, RegisteredMsg::WIRE_SIZE, &mut stream)
        .unwrap();
    let (link, audio) = linking.join().unwrap().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !link.is_connected() {
        assert!(Instant::now() < deadline, "the desk never linked");
        std::thread::sleep(Duration::from_millis(2));
    }
    (link, (audio, stream))
}

#[test]
fn a_load_on_the_desk_says_the_desk_keeps_its_tempo() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::peek(dir.path()).unwrap();
    let mut slow = seed().unwrap();
    slow.tempo = 97.0;
    store.write_save("slow", &slow, false).unwrap();
    let (link, _desk) = a_desk(dir.path(), 120.0);
    let (mut engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let mut wall = Wall::new(
        seed().unwrap(),
        control,
        Some(link),
        Vec::new(),
        Some(store),
    );
    let loaded = load_save(&mut wall, "slow").unwrap();
    // What plays is the desk's tempo, and the change says so.
    let What::Load { tempo, .. } = loaded.change.what else {
        panic!("not a load");
    };
    assert!((tempo - 120.0).abs() < 1e-9, "{tempo}");
    assert!(
        loaded.change.summary.contains("120 BPM"),
        "{}",
        loaded.change.summary
    );
    assert!(
        loaded
            .notes
            .iter()
            .any(|note| note.contains("the desk keeps the tempo at 120 BPM")),
        "{:?}",
        loaded.notes
    );
    // The wall's own clock was not moved to the save's tempo.
    play(&mut wall, &mut engine);
    assert!((wall.engine_mut().shared().bpm() - 120.0).abs() < 1e-9);
}

#[test]
fn a_change_meant_for_a_replaced_wall_is_refused() {
    let (mut wall, _engine, _dir) = saving_wall();
    let first = wall.snapshot().wall;
    assert_eq!(first, 0);
    save(&mut wall, "jam", false).unwrap();
    let loaded = load_save(&mut wall, "jam").unwrap();
    let up = wall.snapshot().wall;
    assert_eq!(up, loaded.change.seq);
    let aimed = turn("vcf1", "cutoff", 500.0);
    let refused = wall.check_wall(Some(first), &aimed).unwrap_err();
    assert_eq!(refused.code, ErrorCode::WallReplaced);
    assert!(
        refused.message.contains("look again"),
        "{}",
        refused.message
    );
    assert!(wall.check_wall(Some(up), &aimed).is_ok());
    assert!(wall.check_wall(None, &aimed).is_ok(), "an older client");
    // What names no module is not checked.
    for free in [Request::Look, Request::Tempo { bpm: 100.0 }, Request::Saves] {
        assert!(wall.check_wall(Some(first), &free).is_ok(), "{free:?}");
    }
}

#[test]
fn loads_keep_the_old_wall_once_and_let_the_walls_own_saves_go() {
    let (mut wall, _engine, dir) = saving_wall();
    let saves = dir.path().join("saves");
    let autos = || {
        std::fs::read_dir(&saves)
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("before-")
            })
            .count()
    };
    save(&mut wall, "a", false).unwrap();
    ask(&mut wall, &turn("vcf1", "cutoff", 1_500.0)).unwrap();
    save(&mut wall, "b", false).unwrap();
    // The wall is "b", untouched: loading "a" keeps it in "b", and back.
    let to_a = load_save(&mut wall, "a").unwrap();
    assert_eq!(to_a.backup, "b");
    let to_b = load_save(&mut wall, "b").unwrap();
    assert_eq!(to_b.backup, "a");
    assert_eq!(autos(), 0, "no copy of a wall a save already holds");
    // Undo still brings each back exactly, from those saves.
    let b_wall = as_saved(&wall);
    undo_load(&mut wall, to_b.change.seq);
    let a_wall = as_saved(&wall);
    assert_ne!(a_wall, b_wall);
    // A wall changed since its save is written afresh.
    ask(&mut wall, &turn("vcf1", "cutoff", 700.0)).unwrap();
    let fresh = load_save(&mut wall, "b").unwrap();
    assert!(fresh.backup.starts_with("before-load-"));
    assert_eq!(autos(), 1);
    // Old automatic saves nobody needs go, beyond the newest kept.
    let wall_file = std::fs::read(saves.join(format!("{}.json", fresh.backup))).unwrap();
    for n in 0..crate::store::AUTO_SAVES_KEPT + 5 {
        let stray = saves.join(format!("before-load-20200101-{n:06}.json"));
        std::fs::write(&stray, &wall_file).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&stray)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_500_000_000 + n as u64))
            .unwrap();
    }
    ask(&mut wall, &turn("vcf1", "cutoff", 650.0)).unwrap();
    let pruned = load_save(&mut wall, "a").unwrap();
    assert_eq!(autos(), crate::store::AUTO_SAVES_KEPT);
    // Those the log needs stay, however old.
    assert!(saves.join(format!("{}.json", fresh.backup)).exists());
    assert!(saves.join(format!("{}.json", pruned.backup)).exists());
    undo_load(&mut wall, fresh.change.seq);
}

#[test]
fn listing_the_saves_has_a_budget_of_its_own() {
    let (mut wall, _engine, _dir) = saving_wall();
    let now = Instant::now();
    for _ in 0..10 {
        assert!(wall.request("Loop", &Request::Saves, now).is_ok());
    }
    let refused = wall.request("Loop", &Request::Saves, now).unwrap_err();
    assert_eq!(refused.code, ErrorCode::SlowDown);
    // Others, and changes, are not held up by it; a second later it has
    // room for one more.
    assert!(wall.request("Tom", &Request::Saves, now).is_ok());
    assert!(
        wall.request("Loop", &turn("vcf1", "cutoff", 800.0), now)
            .is_ok()
    );
    let later = now + Duration::from_secs(1);
    assert!(wall.request("Loop", &Request::Saves, later).is_ok());
    assert!(wall.request("Loop", &Request::Saves, later).is_err());
}

/// A save holds the wall to the last bit, dye amounts included (they are
/// arbitrary f64s, which only read back exactly with `serde_json`'s
/// `float_roundtrip`): a load kept in it comes back with the very
/// fingerprint it left with.
#[test]
fn a_save_holds_the_wall_to_the_last_bit() {
    let (mut wall, _engine, dir) = saving_wall();
    save(&mut wall, "a", false).unwrap();
    ask(&mut wall, &turn("vcf1", "cutoff", 1_500.0)).unwrap();
    save(&mut wall, "b", false).unwrap();
    load_save(&mut wall, "a").unwrap();
    let to_b = load_save(&mut wall, "b").unwrap();
    undo_load(&mut wall, to_b.change.seq);
    ask(&mut wall, &turn("vcf1", "cutoff", 700.0)).unwrap();
    let live = as_saved(&wall);
    let print = wall.patch().fingerprint();
    let fresh = load_save(&mut wall, "b").unwrap();
    let file = PatchFile {
        last_seq: 0,
        wall: 0,
        ..save_file(&dir, &fresh.backup)
    };

    // The dye of turns after a load is the kind of number that read back
    // one bit off without it.
    assert!(!live.dye.is_empty());
    assert_eq!(live, file, "the save is the wall as it was");
    let (decoded, notes) = Patch::load(&file).unwrap();
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(decoded.fingerprint(), print, "and reads back as it");
    undo_load(&mut wall, fresh.change.seq);
    assert_eq!(as_saved(&wall), live);
}

/// Change `seq`, `what`, by Tom, as a log holds it.
fn logged(seq: u64, what: What) -> Change {
    Change {
        seq,
        at: "2026-10-09T00:00:00Z".to_string(),
        seat: "Tom".to_string(),
        summary: format!("change {seq}"),
        what,
        undoes: None,
    }
}

/// A turn of the seed's filter, as a log holds it.
fn a_turn() -> What {
    What::Turn {
        module: "vcf1".to_string(),
        knob: "cutoff".to_string(),
        from: 1_000.0,
        to: 1_200.0,
        glide_beats: 0.0,
    }
}

/// A load, as a log holds it.
fn a_load() -> What {
    What::Load {
        save: Some("jam".to_string()),
        backup: "before-load-20261009-000000".to_string(),
        fingerprint: "0".to_string(),
        modules: 1,
        cables: 0,
        tempo: 120.0,
        by_console: true,
    }
}

#[test]
fn a_tempo_the_log_cannot_take_never_reaches_the_desk() {
    use kazoo_core::ipc::protocol::FrameBuffer;
    use kazoo_core::ipc::types::{MSG_TRANSPORT_REQUEST, TransportRequestMsg};
    let dir = tempfile::tempdir().unwrap();
    let store = Store::peek(dir.path()).unwrap();
    let (link, (_audio, mut stream)) = a_desk(dir.path(), 120.0);
    let (_engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let mut wall = Wall::new(
        seed().unwrap(),
        control,
        Some(link),
        Vec::new(),
        Some(store),
    );
    let tempo = wall.patch().tempo;
    fail_once(&mut wall, FailAt::AppendLog);
    let refused = ask(&mut wall, &Request::Tempo { bpm: 140.0 }).unwrap_err();
    assert_eq!(refused.code, ErrorCode::Internal);
    assert!((wall.patch().tempo - tempo).abs() < 1e-9);
    let asked: TempoResult =
        serde_json::from_value(ask(&mut wall, &Request::Tempo { bpm: 150.0 }).unwrap()).unwrap();
    assert!(asked.desk);
    // The first tempo the desk hears is the one the log took: the other
    // never went.
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut buf = FrameBuffer::new();
    let request = loop {
        let header = buf.read_frame(&mut stream).unwrap();
        if header.msg_type == MSG_TRANSPORT_REQUEST {
            break TransportRequestMsg::decode(buf.payload());
        }
    };
    assert!((request.requested_bpm - 150.0).abs() < 1e-3, "{request:?}");
    // The log says the one did not happen, and has the other.
    let logged = Store::peek(dir.path()).unwrap().load_log().unwrap().changes;
    let [note, made] = logged.as_slice() else {
        panic!("{logged:?}");
    };
    assert!(matches!(note.what, What::Abandoned { .. }), "{note:?}");
    assert_eq!(made, &asked.change);
}

#[test]
fn words_the_log_cannot_take_never_reach_the_player() {
    let (mut wall, _engine, _dir) = saving_wall();
    let (speech, script) = super::super::speech::tests::scripted(16);
    wall.speech = speech;
    let now = Instant::now();
    let add = Request::Add {
        kind: "speak".to_string(),
        name: None,
        place: None,
    };
    wall.request("Tom", &add, now).unwrap();
    let say = |text: &str| Request::Speak {
        module: "speak1".to_string(),
        text: text.to_string(),
        voice: None,
    };
    let said = |wall: &Wall| {
        wall.speech
            .phrase("speak1")
            .map(|phrase| phrase.samples().len() / 100)
    };
    let words = |wall: &Wall| {
        wall.patch()
            .speech
            .get("speak1")
            .map(|words| words.text.clone())
    };
    // A speaker with nothing to say, given words the log refuses: it
    // still says nothing.
    wall.request("Tom", &say("first words"), now).unwrap();
    script.lock().unwrap().finish_all();
    fail_once(&mut wall, FailAt::AppendLog);
    let answers = wall.spoken();
    assert_eq!(answers[0].1.as_ref().unwrap_err().code, ErrorCode::Internal);
    assert_eq!(said(&wall), None);
    assert_eq!(words(&wall), None);
    // Words the log takes are said, and kept.
    wall.request("Tom", &say("hello"), now).unwrap();
    script.lock().unwrap().finish_all();
    assert!(wall.spoken()[0].1.is_ok());
    assert_eq!(said(&wall), Some("hello".len()));
    assert_eq!(words(&wall).as_deref(), Some("hello"));
    // New words the log refuses: the player goes on with the old ones,
    // which the wall still has.
    wall.request("Tom", &say("goodbye now"), now).unwrap();
    script.lock().unwrap().finish_all();
    fail_once(&mut wall, FailAt::AppendLog);
    let answers = wall.spoken();
    assert_eq!(answers[0].1.as_ref().unwrap_err().code, ErrorCode::Internal);
    assert_eq!(said(&wall), Some("hello".len()));
    assert_eq!(words(&wall).as_deref(), Some("hello"));
}

#[test]
fn a_wall_whose_load_rotated_out_of_the_log_starts_as_it_was() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::peek(dir.path()).unwrap();
    // A long-lived wall: its load, then more changes than both logs hold.
    store.append(&logged(1, a_load())).unwrap();
    let last = 2 * crate::store::LOG_ROTATE_LINES as u64 + 10;
    for seq in 2..=last {
        store.append(&logged(seq, a_turn())).unwrap();
    }
    store.sync_log().unwrap();
    let history = store.load_log().unwrap().changes;
    assert!(history[0].seq > 1, "the load has gone with older logs");
    drop(store);
    let mut patch = seed().unwrap();
    patch.wall = 1;
    patch.last_seq = last;
    put_patch_json(&dir, &patch);
    let (mut wall, _engine) = restart(&dir);
    assert!(resumed_notes(&dir).is_empty(), "nothing to say");
    assert_eq!(wall.snapshot().wall, 1);
    // Its changes still undo.
    undo(&mut wall, last).unwrap();
    undo(&mut wall, last - 1).unwrap();
}

#[test]
fn a_wall_started_again_beyond_memory_holds_what_it_had() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::peek(dir.path()).unwrap();
    // Wall 1, then more changes to it than memory keeps; patch.json saved
    // twenty changes short of the end.
    store.append(&logged(1, a_load())).unwrap();
    let last = MEMORY_CHANGES as u64 + 20;
    for seq in 2..=last {
        store.append(&logged(seq, a_turn())).unwrap();
    }
    store.sync_log().unwrap();
    drop(store);
    let through = MEMORY_CHANGES as u64;
    let mut patch = seed().unwrap();
    patch.wall = 1;
    patch.last_seq = through;
    put_patch_json(&dir, &patch);
    let (mut wall, _engine) = restart(&dir);
    let notes = resumed_notes(&dir);
    assert_eq!(notes.len(), 1);
    assert_eq!(
        notes[0].what,
        What::Resumed {
            from: Some(1),
            through,
            left_out: Vec::new(),
        }
    );
    // Memory opens on wall 1, long after its load: what patch.json had of
    // it undoes, what it did not does not.
    assert!(wall.log.front().unwrap().seq > 1);
    undo(&mut wall, through - 5).unwrap();
    let refused = undo(&mut wall, through + 10).unwrap_err();
    assert_eq!(refused.code, ErrorCode::NotAllowed, "{}", refused.message);
}

#[test]
fn a_log_that_owes_says_so_and_says_when_it_is_paid() {
    let (mut wall, _engine, _dir) = saving_wall();
    let now = Instant::now();
    assert!(wall.snapshot().log.is_ok());
    // The turn's line fails, then the note saying so.
    fail_once(&mut wall, FailAt::AppendLog);
    fail_once(&mut wall, FailAt::AppendLog);
    ask(&mut wall, &turn("vcf1", "cutoff", 1_500.0)).unwrap_err();
    let health = wall.snapshot().log;
    assert_eq!(health.owed, 1);
    assert!(health.error.is_some());
    let said = |events: &[Event]| {
        events
            .iter()
            .filter_map(|event| match event {
                Event::Log { log } => Some(log.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    // Told once, and the owed change is not told before it is written.
    let events = wall.tick(now);
    assert_eq!(said(&events), [health]);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::Change { .. }))
    );
    assert!(said(&wall.tick(now)).is_empty());
    // The log takes lines again: what it owed goes first, numbered then.
    let next = changed(ask(&mut wall, &turn("vcf1", "cutoff", 1_500.0)).unwrap());
    assert!(wall.snapshot().log.is_ok());
    let events = wall.tick(now);
    assert_eq!(said(&events), [LogHealth::default()]);
    let notes: Vec<&Change> = events
        .iter()
        .filter_map(|event| match event {
            Event::Change { change } => Some(change.as_ref()),
            _ => None,
        })
        .collect();
    assert_eq!(notes.len(), 1);
    assert!(matches!(notes[0].what, What::Abandoned { .. }));
    assert!(notes[0].seq < next.change.seq);
}

#[test]
fn stopping_while_the_log_owes_numbers_nothing_twice() {
    let (mut wall, _engine, dir) = saving_wall();
    let now = Instant::now();
    let first = changed(ask(&mut wall, &turn("vcf1", "cutoff", 1_500.0)).unwrap());
    wall.save(now);
    let mut told = vec![first.change.seq];
    // The log fails for good: the turn, its note, and every try after.
    for _ in 0..8 {
        fail_once(&mut wall, FailAt::AppendLog);
    }
    ask(&mut wall, &turn("vcf1", "drive", 0.3)).unwrap_err();
    told.extend(wall.tick(now).iter().filter_map(|event| match event {
        Event::Change { change } => Some(change.seq),
        _ => None,
    }));
    let events = wall.close(now);
    told.extend(events.iter().filter_map(|event| match event {
        Event::Change { change } => Some(change.seq),
        _ => None,
    }));
    // What it still owed is lost, said, and never numbered.
    assert_eq!(wall.snapshot().log.owed, 1);
    drop(wall);
    let (mut wall, _engine) = restart(&dir);
    let next = changed(ask(&mut wall, &turn("vcf1", "drive", 0.3)).unwrap());
    let most = told.iter().copied().max().unwrap();
    assert!(next.change.seq > most, "{} after {told:?}", next.change.seq);
}

#[test]
fn a_load_after_owed_notes_are_paid_names_its_own_wall() {
    let (mut wall, _engine, dir) = saving_wall();
    save(&mut wall, "jam", false).unwrap();
    let made = changed(ask(&mut wall, &turn("vcf1", "cutoff", 1_500.0)).unwrap());
    // The next turn's line fails, then the note saying so: it is owed.
    fail_once(&mut wall, FailAt::AppendLog);
    fail_once(&mut wall, FailAt::AppendLog);
    ask(&mut wall, &turn("vcf1", "drive", 0.3)).unwrap_err();
    assert_eq!(wall.snapshot().log.owed, 1);
    // The revision is the latest change made, not the number given to
    // the one that was not.
    assert_eq!(wall.revision(), made.change.seq);
    assert_eq!(wall.snapshot().revision, made.change.seq);
    // The log recovers: the load pays the note first, then is numbered.
    let loaded = load_save(&mut wall, "jam").unwrap();
    let logged = Store::peek(dir.path()).unwrap().load_log().unwrap().changes;
    let [.., note, load] = logged.as_slice() else {
        panic!("{logged:?}");
    };
    assert!(matches!(note.what, What::Abandoned { .. }), "{note:?}");
    assert_eq!(load, &loaded.change);
    assert_eq!(wall.patch().wall, loaded.change.seq);
    assert_eq!(wall.patch().last_seq, loaded.change.seq);
    assert_eq!(wall.snapshot().wall, loaded.change.seq);
    let on_disk = Store::peek(dir.path()).unwrap().load_patch().unwrap();
    let Loaded::Patch { patch, .. } = on_disk else {
        panic!("no patch.json");
    };
    assert_eq!(patch.wall, loaded.change.seq);
    assert_eq!(patch.last_seq, loaded.change.seq);
    // A start from here has nothing to say, and the load undoes.
    drop(wall);
    let (mut wall, _engine) = restart(&dir);
    assert!(resumed_notes(&dir).is_empty());
    undo_load(&mut wall, loaded.change.seq);
}

#[test]
fn a_rotated_log_behind_patch_json_still_undoes_what_patch_json_had() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::peek(dir.path()).unwrap();
    // Wall 0 for longer than both logs hold, and patch.json saved five
    // changes short of the end.
    let last = 2 * crate::store::LOG_ROTATE_LINES as u64 + 10;
    for seq in 1..=last {
        store.append(&logged(seq, a_turn())).unwrap();
    }
    store.sync_log().unwrap();
    drop(store);
    let through = last - 5;
    let mut patch = seed().unwrap();
    patch.last_seq = through;
    put_patch_json(&dir, &patch);
    let (mut wall, _engine) = restart(&dir);
    let notes = resumed_notes(&dir);
    assert_eq!(notes.len(), 1);
    assert_eq!(
        notes[0].what,
        What::Resumed {
            from: Some(0),
            through,
            left_out: Vec::new(),
        }
    );
    undo(&mut wall, through - 1).unwrap();
    undo(&mut wall, through).unwrap();
    let refused = undo(&mut wall, through + 1).unwrap_err();
    assert_eq!(refused.code, ErrorCode::NotAllowed, "{}", refused.message);
}

#[test]
fn back_to_back_tempos_undo_to_the_one_before() {
    let (mut wall, _engine) = wall();
    let initial = wall.patch().tempo;
    let first: TempoResult =
        serde_json::from_value(ask(&mut wall, &Request::Tempo { bpm: 100.0 }).unwrap()).unwrap();
    // At once, before the engine has played a block of the first.
    let second: TempoResult =
        serde_json::from_value(ask(&mut wall, &Request::Tempo { bpm: 110.0 }).unwrap()).unwrap();
    let What::Tempo { from, .. } = second.change.what else {
        panic!("{:?}", second.change);
    };
    assert!((from - 100.0).abs() < 1e-9, "{from}");
    undo(&mut wall, second.change.seq).unwrap();
    assert!((wall.patch().tempo - 100.0).abs() < 1e-9);
    undo(&mut wall, first.change.seq).unwrap();
    assert!(
        (wall.patch().tempo - initial).abs() < 1e-9,
        "{} after {initial}",
        wall.patch().tempo
    );
}

#[test]
fn an_engine_rebuilt_mid_swap_plays_the_new_walls_tempo() {
    let (mut wall, _engine, _dir) = saving_wall();
    let mut slow = seed().unwrap();
    slow.tempo = 97.0;
    wall.store_mut()
        .unwrap()
        .write_save("slow", &slow, false)
        .unwrap();
    // The load sends its swap; before the engine plays a block of it, the
    // audio is rebuilt on a new engine started at the old wall's tempo.
    load_save(&mut wall, "slow").unwrap();
    let (mut rebuilt, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    wall.attach(control, None);
    play(&mut wall, &mut rebuilt);
    assert!(
        (wall.engine_mut().shared().bpm() - 97.0).abs() < 1e-9,
        "{}",
        wall.engine_mut().shared().bpm()
    );
    assert!((wall.snapshot().tempo - 97.0).abs() < 1e-9);
}

#[test]
fn a_save_says_it_changed_only_when_the_wall_did() {
    let (mut wall, _engine, _dir) = saving_wall();
    let changed_since = |wall: &Wall| wall.snapshot().save.unwrap().changed;
    save(&mut wall, "jam", false).unwrap();
    assert!(!changed_since(&wall));
    // A note of the wall's own moves the revision, not the wall.
    fail_once(&mut wall, FailAt::AppendLog);
    ask(&mut wall, &turn("vcf1", "cutoff", 1_500.0)).unwrap_err();
    assert!(wall.revision() > wall.snapshot().save.unwrap().revision);
    assert!(!changed_since(&wall));
    // A move on the rack moves no revision, but changes the wall.
    let revision = wall.revision();
    let rows = wall.patch().rows().to_vec();
    ask(
        &mut wall,
        &Request::Arrange {
            module: "lfo1".to_string(),
            row: rows.len(),
            before: None,
            own: true,
        },
    )
    .unwrap();
    assert_eq!(wall.revision(), revision);
    assert!(changed_since(&wall));
    // Saved again, it holds the wall as it is.
    save(&mut wall, "jam", true).unwrap();
    assert!(!changed_since(&wall));
}

#[test]
fn a_save_or_deletion_whose_directory_cannot_be_flushed_still_happens() {
    let (mut wall, _engine, _dir) = saving_wall();
    save(&mut wall, "jam", false).unwrap();
    ask(&mut wall, &turn("vcf1", "cutoff", 1_500.0)).unwrap();
    // Saved over: in place, so the wall says it is that save, tells
    // everyone, and notes the flush.
    fail_once(&mut wall, FailAt::SyncDir);
    let outcome = save(&mut wall, "jam", true).unwrap();
    let result: SaveResult = serde_json::from_value(outcome.result).unwrap();
    assert!(result.replaced);
    assert_eq!(result.notes.len(), 1, "{:?}", result.notes);
    assert!(
        result.notes[0].contains("take it back"),
        "{:?}",
        result.notes
    );
    assert!(matches!(outcome.event, Some(Event::Saved { .. })));
    assert!(!wall.snapshot().save.unwrap().changed);
    // Deleted: gone, so the wall forgets it and tells everyone.
    fail_once(&mut wall, FailAt::SyncDir);
    let outcome = ask_outcome(
        &mut wall,
        &Request::DeleteSave {
            name: "jam".to_string(),
        },
    );
    let result: DeleteSaveResult = serde_json::from_value(outcome.result).unwrap();
    assert_eq!(result.name, "jam");
    assert_eq!(result.notes.len(), 1, "{:?}", result.notes);
    assert!(matches!(outcome.event, Some(Event::SaveDeleted { .. })));
    assert!(wall.snapshot().save.is_none());
}

#[test]
fn a_save_a_crash_could_take_back_is_no_backup() {
    let (mut wall, _engine, _dir) = saving_wall();
    fail_once(&mut wall, FailAt::SyncDir);
    let outcome = save(&mut wall, "jam", false).unwrap();
    let result: SaveResult = serde_json::from_value(outcome.result).unwrap();
    assert_eq!(result.notes.len(), 1);
    // The wall is that save, but a new wall keeps it afresh.
    let cleared: LoadResult =
        serde_json::from_value(ask(&mut wall, &Request::New).unwrap()).unwrap();
    assert!(
        cleared.backup.starts_with("before-new-"),
        "{}",
        cleared.backup
    );
    // A save that was flushed is trusted.
    save(&mut wall, "kept", false).unwrap();
    let again: LoadResult = serde_json::from_value(ask(&mut wall, &Request::New).unwrap()).unwrap();
    assert_eq!(again.backup, "kept");
}

#[test]
fn on_the_desk_a_load_is_not_changed_by_the_desks_tempo() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    let mut slow = seed().unwrap();
    slow.tempo = 97.0;
    store.write_save("slow", &slow, false).unwrap();
    let (link, _desk) = a_desk(dir.path(), 120.0);
    let (_engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let mut wall = Wall::new(
        seed().unwrap(),
        control,
        Some(link),
        Vec::new(),
        Some(store),
    );
    load_save(&mut wall, "slow").unwrap();
    // The tick takes the desk's tempo into the wall's saved tempo.
    wall.tick(Instant::now());
    assert!(
        (wall.patch().tempo - 120.0).abs() < 1e-9,
        "{}",
        wall.patch().tempo
    );
    assert!(!wall.snapshot().save.unwrap().changed);
    // And the save still holds the wall: a new wall keeps it there.
    let cleared: LoadResult =
        serde_json::from_value(ask(&mut wall, &Request::New).unwrap()).unwrap();
    assert_eq!(cleared.backup, "slow");
}

#[test]
fn a_wall_too_large_to_keep_says_so_and_takes_nothing_that_grows_it() {
    let (mut wall, _engine, dir) = saving_wall();
    let now = Instant::now();
    wall.save(now);
    let kept = std::fs::read(dir.path().join("patch.json")).unwrap();
    // Grown past its file by what no request could give it.
    wall.patch.speech.insert(
        "speak9".to_string(),
        Words {
            text: "a".repeat(usize::try_from(crate::store::MAX_WALL_FILE).unwrap()),
            voice: None,
        },
    );
    wall.dirty = true;
    wall.save(now);
    assert_eq!(std::fs::read(dir.path().join("patch.json")).unwrap(), kept);
    let faults: Vec<String> = wall
        .tick(now)
        .into_iter()
        .filter_map(|event| match event {
            Event::Fault { summary, .. } => Some(summary),
            _ => None,
        })
        .collect();
    assert_eq!(faults.len(), 1, "said once: {faults:?}");
    assert!(
        faults[0].contains("changes are not being kept"),
        "{faults:?}"
    );
    // What grows it is refused; what takes away goes through.
    let refused = ask(
        &mut wall,
        &Request::Add {
            kind: "lfo".to_string(),
            name: None,
            place: None,
        },
    )
    .unwrap_err();
    assert_eq!(refused.code, ErrorCode::Full);
    assert!(refused.message.contains("take some"), "{}", refused.message);
    ask(
        &mut wall,
        &Request::Remove {
            module: "lfo2".to_string(),
        },
    )
    .unwrap();
    // No load either: the wall as it is cannot be kept first.
    let refused = ask(&mut wall, &Request::New).unwrap_err();
    assert_eq!(refused.code, ErrorCode::Full);
    assert!(
        refused.message.contains("cannot be kept in a save first"),
        "{}",
        refused.message
    );
    // Small enough again: kept, said, and nothing refused for size.
    wall.patch.speech.clear();
    wall.save(now);
    assert_ne!(std::fs::read(dir.path().join("patch.json")).unwrap(), kept);
    assert!(wall.tick(now).iter().any(|event| matches!(
        event,
        Event::Fault { summary, .. } if summary.contains("small enough to keep again")
    )));
    ask(
        &mut wall,
        &Request::Add {
            kind: "lfo".to_string(),
            name: None,
            place: None,
        },
    )
    .unwrap();
}

/// `request` from Tom, as the whole outcome.
fn ask_outcome(wall: &mut Wall, request: &Request) -> Outcome {
    wall.request("Tom", request, Instant::now()).unwrap()
}

#[test]
fn a_refused_load_costs_no_allowance() {
    let (mut wall, _engine, _dir) = saving_wall();
    save(&mut wall, "jam", false).unwrap();
    let now = Instant::now();
    let load_as = |wall: &mut Wall, name: &str| {
        let request = Request::Load {
            name: name.to_string(),
        };
        wall.request_as("Ava", false, &request, now)
    };
    for _ in 0..5 {
        assert_eq!(
            load_as(&mut wall, "nope").unwrap_err().code,
            ErrorCode::UnknownSave
        );
        assert_eq!(
            load_as(&mut wall, "../x").unwrap_err().code,
            ErrorCode::BadName
        );
    }
    for _ in 0..3 {
        load_as(&mut wall, "jam").unwrap();
    }
    assert_eq!(
        load_as(&mut wall, "jam").unwrap_err().code,
        ErrorCode::SlowDown
    );
}

/// A wall of `kinds` cabled `cables`, built straight into its patch (no
/// flood guard to meet), with the engine that plays it.
fn probing_wall(kinds: &[&str], cables: &[(&str, &str)]) -> (Wall, Engine) {
    let mut patch = Patch::empty(120.0);
    for kind in kinds {
        patch.add(kind, None, None).unwrap();
    }
    for (from, to) in cables {
        patch.plug(from, to, None, None).unwrap();
    }
    let (engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    (Wall::new(patch, control, None, Vec::new(), None), engine)
}

#[test]
fn a_trail_too_long_to_follow_shows_nothing_and_never_runs_out_of_stack() {
    // A pitch through a chain of slews into a readout: followed back all
    // the way while the chain is within TRAIL_DEPTH, and given up (shown
    // as nothing) past it, on a thread with the control thread's stack.
    for (slews, shown) in [
        (TRAIL_DEPTH - 1, Some(probe::Show::Pitch)),
        (TRAIL_DEPTH, None),
        (TRAIL_STEPS * 2, None),
    ] {
        let mut patch = Patch::empty(120.0);
        patch.add("seq", None, None).unwrap();
        for _ in 0..slews {
            patch.add("slew", None, None).unwrap();
        }
        patch.add("readout", None, None).unwrap();
        patch.plug("seq1.pitch", "slew1.in", None, None).unwrap();
        for n in 1..slews {
            patch
                .plug(
                    &format!("slew{n}.out"),
                    &format!("slew{}.in", n + 1),
                    None,
                    None,
                )
                .unwrap();
        }
        patch
            .plug(&format!("slew{slews}.out"), "readout1.in", None, None)
            .unwrap();
        let found = std::thread::Builder::new()
            .name("trail".to_string())
            .spawn(move || {
                let (engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
                let wall = Wall::new(patch, control, None, Vec::new(), None);
                let shown = wall.source_show("readout1", &mut wall.trail());
                drop(engine);
                shown
            })
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(found, shown, "{slews} slews");
    }
}

#[test]
fn a_change_that_could_take_the_file_past_its_bound_is_refused() {
    let (mut wall, _engine) = wall();
    let measured = crate::store::encode(wall.patch()).unwrap().len();
    // Room for a module at its worst, and not a byte more.
    wall.file_limit = measured + crate::store::WORST_MODULE_BYTES - 1;
    let refused = ask(&mut wall, &add("mix")).unwrap_err();
    assert_eq!(refused.code, ErrorCode::Full);
    assert!(
        refused.message.contains("take something away"),
        "{}",
        refused.message
    );
    assert_eq!(wall.patch().modules().len(), wall.rack.index.len());
    wall.file_limit = measured + crate::store::WORST_MODULE_BYTES;
    let mixer = changed(ask(&mut wall, &add("mix")).unwrap())
        .module
        .unwrap();
    // The next is judged on the file as it now is, measured afresh.
    assert_eq!(code(ask(&mut wall, &add("mix"))), ErrorCode::Full);
    assert!(wall.file_bytes.unwrap() > measured);
    // A cable takes far less at its worst: there is still room for one.
    ask(&mut wall, &patch("vco1.out", &format!("{mixer}.a"))).unwrap();
    // What adds nothing goes: a knob, taking things away.
    ask(&mut wall, &turn("vcf1", "cutoff", 420.0)).unwrap();
    ask(&mut wall, &Request::Remove { module: mixer }).unwrap();
    // Well inside the bound, the file is not measured for every change.
    wall.file_limit = measured * 10;
    wall.file_bytes = Some(measured);
    wall.file_grown = 0;
    ask(&mut wall, &add("mix")).unwrap();
    assert_eq!(wall.file_bytes, Some(measured));
    assert_eq!(wall.file_grown, crate::store::WORST_MODULE_BYTES);
}

/// Play `seconds` of the wall, ticking every 20 ms as the daemon does.
fn run_for(wall: &mut Wall, engine: &mut Engine, seconds: f64) {
    let mut buffer = vec![0.0_f32; 960 * 2];
    for _ in 0..(seconds * 50.0).round() as usize {
        engine.render(&mut buffer, 2);
        wall.tick(Instant::now());
    }
}

fn probed(wall: &mut Wall, module: Option<&str>) -> ProbeResult {
    let request = Request::Probe {
        module: module.map(str::to_string),
        modules: Vec::new(),
    };
    serde_json::from_value(ask(wall, &request).unwrap()).unwrap()
}

#[test]
fn a_scope_and_a_readout_measure_what_is_plugged_into_them() {
    let (mut wall, mut engine) = probing_wall(
        &["vco", "scope", "readout"],
        &[("vco1.out", "scope1.a"), ("scope1.a", "readout1.in")],
    );
    // A4, as a sine.
    ask(&mut wall, &turn("vco1", "tune", 9.0)).unwrap();
    ask(&mut wall, &turn("vco1", "shape", 0.0)).unwrap();
    let revision = wall.revision();
    run_for(&mut wall, &mut engine, 1.0);
    let result = probed(&mut wall, None);
    assert!(result.left_out.is_empty());
    let modules: Vec<&str> = result
        .probes
        .iter()
        .map(|probe| probe.module.as_str())
        .collect();
    assert_eq!(modules, ["scope1", "readout1"]);
    let scope = result.probes[0].scope.as_ref().unwrap();
    assert!(result.probes[0].readout.is_none());
    assert!(result.probes[0].age < 0.5, "{}", result.probes[0].age);
    assert!(scope.triggered, "{scope:?}");
    assert_eq!(scope.a.len(), 96);
    assert!(scope.b.is_empty(), "b is unplugged");
    let hz = scope.input_a.hz.unwrap();
    assert!((hz - 440.0).abs() < 0.5, "{hz}");
    assert_eq!(scope.input_a.shape.as_deref(), Some("sine"));
    assert!(
        scope.words.starts_with("a: 440 Hz sine from "),
        "{}",
        scope.words
    );
    assert!(
        scope.words.contains("; b unplugged; triggered"),
        "{}",
        scope.words
    );
    // The readout hears it through the scope, and shows it as the audio
    // it is.
    let readout = result.probes[1].readout.as_ref().unwrap();
    assert_eq!(readout.mode, "audio");
    assert!(readout.auto);
    assert!((readout.input.hz.unwrap() - 440.0).abs() < 0.5);
    assert_eq!(readout.lines[3], "440 Hz", "{:?}", readout.lines);
    // A look carries the same; probing changed nothing.
    let look = wall.snapshot();
    assert_eq!(look.probes.len(), 2);
    assert_eq!(wall.revision(), revision);
    // One by its id; one that measures nothing; one that is not there.
    assert_eq!(probed(&mut wall, Some("readout1")).probes.len(), 1);
    let refused = ask(
        &mut wall,
        &Request::Probe {
            module: Some("vco1".to_string()),
            modules: Vec::new(),
        },
    )
    .unwrap_err();
    assert_eq!(refused.code, ErrorCode::BadRequest);
    assert!(
        refused.message.contains("probes: scope1, readout1"),
        "{}",
        refused.message
    );
    let missing = Request::Probe {
        module: Some("scope9".to_string()),
        modules: Vec::new(),
    };
    assert_eq!(code(ask(&mut wall, &missing)), ErrorCode::UnknownModule);
    // A watcher may look too: it is a read, not a change.
    let every = Request::Probe {
        module: None,
        modules: Vec::new(),
    };
    assert!(!every.is_change());
}

#[test]
fn a_readout_on_auto_shows_what_its_cable_carries() {
    let (mut wall, mut engine) = probing_wall(
        &[
            "clock", "seq", "vco", "scope", "scope", "readout", "readout", "readout", "readout",
            "readout", "lfo", "readout",
        ],
        &[
            ("lfo1.out", "readout6.in"),
            ("clock1.out", "readout1.in"),
            ("seq1.pitch", "readout2.in"),
            ("vco1.out", "scope1.b"),
            ("scope1.b", "readout3.in"),
            // A loop of pass-throughs: nothing at its start.
            ("readout5.out", "scope2.a"),
            ("scope2.a", "readout5.in"),
        ],
    );
    let source = |wall: &Wall, id: &str| wall.source_show(id, &mut wall.trail());
    assert_eq!(source(&wall, "readout1"), Some(probe::Show::Gate));
    assert_eq!(source(&wall, "readout2"), Some(probe::Show::Pitch));
    assert_eq!(source(&wall, "readout3"), Some(probe::Show::Audio));
    assert_eq!(source(&wall, "readout4"), None);
    assert_eq!(source(&wall, "readout5"), None);
    // Every other CV is a plain CV, not a note.
    assert_eq!(source(&wall, "readout6"), Some(probe::Show::Cv));
    run_for(&mut wall, &mut engine, 0.5);
    let shown = |wall: &mut Wall, id: &str| {
        probed(wall, Some(id)).probes[0]
            .readout
            .clone()
            .unwrap_or_else(|| panic!("{id} has no reading"))
    };
    assert_eq!(shown(&mut wall, "readout1").mode, "gate");
    let pitch = shown(&mut wall, "readout2");
    assert_eq!(pitch.mode, "pitch");
    assert!(pitch.note.is_some(), "{pitch:?}");
    assert_eq!(shown(&mut wall, "readout3").mode, "audio");
    let lfo = shown(&mut wall, "readout6");
    assert_eq!(lfo.mode, "cv");
    assert!(
        lfo.note.is_none() && lfo.lines[0].starts_with("cv "),
        "{lfo:?}"
    );
    let unplugged = shown(&mut wall, "readout4");
    assert_eq!(unplugged.lines, ["nothing plugged in"]);
    // The knob overrides what is plugged in.
    ask(&mut wall, &turn("readout1", "mode", 1.0)).unwrap();
    run_for(&mut wall, &mut engine, 0.3);
    let chosen = shown(&mut wall, "readout1");
    assert_eq!(chosen.mode, "pitch");
    assert!(!chosen.auto);
}

#[test]
fn an_answer_carries_at_most_sixteen_probes_and_stays_small() {
    let kinds: Vec<&str> = std::iter::once("noise")
        .chain(std::iter::repeat_n("scope", MAX_PROBES + 2))
        .collect();
    let ids: Vec<String> = (1..=MAX_PROBES + 2).map(|n| format!("scope{n}")).collect();
    let mut cables = Vec::new();
    for id in &ids {
        cables.push(("noise1.out".to_string(), format!("{id}.a")));
        cables.push(("noise1.out".to_string(), format!("{id}.b")));
    }
    let cables: Vec<(&str, &str)> = cables
        .iter()
        .map(|(from, to)| (from.as_str(), to.as_str()))
        .collect();
    let (mut wall, mut engine) = probing_wall(&kinds, &cables);
    run_for(&mut wall, &mut engine, 0.3);
    let look = wall.snapshot();
    assert_eq!(look.probes.len(), MAX_PROBES);
    assert_eq!(look.probes[MAX_PROBES - 1].module, ids[MAX_PROBES - 1]);
    let result = probed(&mut wall, None);
    assert_eq!(result.probes.len(), MAX_PROBES);
    assert_eq!(result.left_out, ids[MAX_PROBES..]);
    // Every trace is whole, and the answer is bounded: two traces of at
    // most 96 points a scope.
    let bytes = serde_json::to_string(&result).unwrap().len();
    assert!(bytes < MAX_PROBES * 8 * 1024, "{bytes} bytes");
    for found in &result.probes {
        let scope = found.scope.as_ref().unwrap();
        assert_eq!((scope.a.len(), scope.b.len()), (96, 96), "{}", found.module);
    }
    // One past the sixteen is still there by its id.
    let last = probed(&mut wall, Some(&ids[MAX_PROBES + 1]));
    assert!(last.probes[0].scope.is_some());
}

#[test]
fn readings_of_modules_gone_or_replaced_are_let_go() {
    let (mut wall, mut engine) = probing_wall(&["readout"], &[]);
    run_for(&mut wall, &mut engine, 0.3);
    assert!(probed(&mut wall, None).probes[0].readout.is_some());
    let remove = Request::Remove {
        module: "readout1".to_string(),
    };
    ask(&mut wall, &remove).unwrap();
    run_for(&mut wall, &mut engine, 0.1);
    assert!(wall.readings.is_empty());
    assert!(probed(&mut wall, None).probes.is_empty());
    // Undone, it is back, and measures afresh.
    let removal = wall.revision();
    ask(&mut wall, &Request::Undo { change: removal }).unwrap();
    let back = probed(&mut wall, None);
    assert_eq!(back.probes.len(), 1);
    assert!(back.probes[0].readout.is_none(), "nothing measured yet");
    run_for(&mut wall, &mut engine, 0.3);
    assert!(probed(&mut wall, None).probes[0].readout.is_some());
}

#[test]
fn a_wall_of_scopes_and_readouts_saves_and_loads_back_and_measures_again() {
    let (mut wall, mut engine, dir) = saving_wall();
    for kind in ["scope", "readout"] {
        let add = Request::Add {
            kind: kind.to_string(),
            name: Some(format!("my {kind}")),
            place: None,
        };
        ask(&mut wall, &add).unwrap();
    }
    for (module, knob, value) in [
        ("scope1", "time", 0.5),
        ("scope1", "zoom", 4.0),
        ("scope1", "trigger", -0.25),
        ("scope1", "mode", 1.0),
        ("readout1", "mode", 3.0),
    ] {
        ask(&mut wall, &turn(module, knob, value)).unwrap();
    }
    for (from, to) in [
        ("vco1.out", "scope1.a"),
        ("lfo1.out", "scope1.b"),
        ("clock1.out", "readout1.in"),
        ("readout1.out", "scope1.trigger"),
    ] {
        let patch = Request::Patch {
            from: from.to_string(),
            to: to.to_string(),
            amount: Some(0.5),
        };
        ask(&mut wall, &patch).unwrap();
    }
    play(&mut wall, &mut engine);
    save(&mut wall, "probes", false).unwrap();
    let saved = save_file(&dir, "probes");
    let kinds: Vec<&str> = saved
        .modules
        .iter()
        .map(|module| module.kind.as_str())
        .collect();
    assert!(
        kinds.contains(&"scope") && kinds.contains(&"readout"),
        "{kinds:?}"
    );
    let cables = wall.snapshot().cables;
    // Replaced by an empty wall, then loaded back: the same modules at the
    // same knobs, the same cables.
    ask(&mut wall, &Request::New).unwrap();
    play(&mut wall, &mut engine);
    assert!(wall.snapshot().probes.is_empty());
    load_save(&mut wall, "probes").unwrap();
    play(&mut wall, &mut engine);
    plays_at_its_targets(&wall, &saved);
    assert_eq!(wall.snapshot().cables, cables);
    let names: Vec<Option<String>> = wall
        .snapshot()
        .modules
        .iter()
        .filter(|module| module.kind == "scope" || module.kind == "readout")
        .map(|module| module.name.clone())
        .collect();
    assert_eq!(
        names,
        [Some("my scope".to_string()), Some("my readout".to_string())]
    );
    // And they measure again.
    run_for(&mut wall, &mut engine, 0.6);
    let result = probed(&mut wall, None);
    let scope = result.probes[0].scope.as_ref().unwrap();
    assert_eq!(scope.mode, "free");
    assert!((scope.zoom - 4.0).abs() < 1e-9);
    assert!(scope.input_b.connected);
    let readout = result.probes[1].readout.as_ref().unwrap();
    assert_eq!(readout.mode, "gate");
    assert!(!readout.auto);
}

/// Render `frames` without a tick: captures wait in the probe ring.
fn render_only(engine: &mut Engine, frames: usize) {
    let mut buffer = vec![0.0_f32; frames * 2];
    engine.render(&mut buffer, 2);
}

#[test]
fn what_was_measured_before_a_cable_moved_is_never_shown() {
    let (mut wall, mut engine) =
        probing_wall(&["vco", "noise", "scope"], &[("vco1.out", "scope1.a")]);
    run_for(&mut wall, &mut engine, 0.4);
    let before = probed(&mut wall, Some("scope1")).probes[0]
        .scope
        .clone()
        .unwrap();
    assert!(
        before.input_a.hz.is_some_and(|hz| (hz - 261.6).abs() < 1.0),
        "{before:?}"
    );
    // Captures of the oscillator wait in the ring when the cable moves.
    render_only(&mut engine, 9_600);
    let patch = Request::Patch {
        from: "noise1.out".to_string(),
        to: "scope1.a".to_string(),
        amount: None,
    };
    ask(&mut wall, &patch).unwrap();
    assert!(
        probed(&mut wall, Some("scope1")).probes[0].scope.is_none(),
        "the oscillator's reading is let go at once"
    );
    wall.tick(Instant::now());
    assert!(
        probed(&mut wall, Some("scope1")).probes[0].scope.is_none(),
        "and those waiting in the ring are let go too"
    );
    // What comes after is the noise alone, never the oscillator.
    for _ in 0..20 {
        run_for(&mut wall, &mut engine, 0.04);
        if let Some(scope) = probed(&mut wall, Some("scope1")).probes[0].scope.clone() {
            assert!(scope.input_a.shape.is_none(), "{scope:?}");
            assert!(!scope.input_a.steady(), "{scope:?}");
        }
    }
    assert!(probed(&mut wall, Some("scope1")).probes[0].scope.is_some());
}

#[test]
fn a_removed_scope_s_capture_never_lands_on_what_takes_its_slot() {
    let (mut wall, mut engine) = probing_wall(&["vco", "scope"], &[("vco1.out", "scope1.a")]);
    run_for(&mut wall, &mut engine, 0.3);
    // Captures of the scope wait in the ring as it goes and a readout
    // takes its slot.
    render_only(&mut engine, 9_600);
    let remove = Request::Remove {
        module: "scope1".to_string(),
    };
    ask(&mut wall, &remove).unwrap();
    let add = Request::Add {
        kind: "readout".to_string(),
        name: None,
        place: None,
    };
    ask(&mut wall, &add).unwrap();
    wall.tick(Instant::now());
    let found = probed(&mut wall, Some("readout1"));
    assert!(found.probes[0].scope.is_none() && found.probes[0].readout.is_none());
    assert!(!wall.readings.contains_key("scope1"));
}

#[test]
fn readings_follow_a_load_and_its_undo() {
    let (mut wall, mut engine, _dir) = saving_wall();
    let add = Request::Add {
        kind: "scope".to_string(),
        name: None,
        place: None,
    };
    ask(&mut wall, &add).unwrap();
    let patch = Request::Patch {
        from: "vco1.out".to_string(),
        to: "scope1.a".to_string(),
        amount: None,
    };
    ask(&mut wall, &patch).unwrap();
    save(&mut wall, "measured", false).unwrap();
    ask(&mut wall, &Request::New).unwrap();
    run_for(&mut wall, &mut engine, 0.2);
    assert!(wall.snapshot().probes.is_empty());
    let loaded = load_save(&mut wall, "measured").unwrap();
    // Nothing of the empty wall, nor of the scope before it went.
    assert!(wall.readings.is_empty());
    run_for(&mut wall, &mut engine, 0.5);
    let scope = probed(&mut wall, Some("scope1")).probes[0]
        .scope
        .clone()
        .unwrap();
    assert!(scope.input_a.connected, "{scope:?}");
    // Undone, the empty wall is back: no probes, no readings.
    undo_load(&mut wall, loaded.change.seq);
    assert!(wall.readings.is_empty());
    run_for(&mut wall, &mut engine, 0.2);
    assert!(wall.snapshot().probes.is_empty());
    assert!(wall.readings.is_empty());
}

#[test]
fn a_rebuilt_engine_at_another_rate_measures_afresh() {
    let (mut wall, mut engine) = probing_wall(&["vco", "scope"], &[("vco1.out", "scope1.a")]);
    run_for(&mut wall, &mut engine, 0.3);
    assert!(probed(&mut wall, None).probes[0].scope.is_some());
    // Captures of the old engine wait in its ring, which goes with it.
    render_only(&mut engine, 4_800);
    drop(engine);
    let (mut rebuilt, control) = crate::engine::engine(EngineConfig::new(96_000, 120.0, 0.0), None);
    wall.attach(control, None);
    assert!(wall.readings.is_empty());
    assert!(probed(&mut wall, None).probes[0].scope.is_none());
    let mut buffer = vec![0.0_f32; 1_920 * 2];
    for _ in 0..25 {
        rebuilt.render(&mut buffer, 2);
        wall.tick(Instant::now());
    }
    let scope = probed(&mut wall, None).probes[0].scope.clone().unwrap();
    assert!((scope.span - 0.02).abs() < 1e-3, "{}", scope.span);
    let hz = scope.input_a.hz.unwrap();
    assert!((hz - 261.6).abs() < 1.0, "{hz}");
}

#[test]
fn probing_several_passes_over_one_another_seat_took_away() {
    let (mut wall, mut engine) = probing_wall(&["readout", "readout", "vco"], &[]);
    run_for(&mut wall, &mut engine, 0.3);
    let remove = Request::Remove {
        module: "readout2".to_string(),
    };
    wall.request("Ava", &remove, Instant::now()).unwrap();
    let several = Request::Probe {
        module: None,
        modules: vec![
            "readout1".to_string(),
            "readout2".to_string(),
            "vco1".to_string(),
        ],
    };
    let found: ProbeResult = serde_json::from_value(ask(&mut wall, &several).unwrap()).unwrap();
    let ids: Vec<&str> = found
        .probes
        .iter()
        .map(|probe| probe.module.as_str())
        .collect();
    assert_eq!(ids, ["readout1"]);
    assert_eq!(found.missing, ["readout2", "vco1"]);
    assert!(found.probes[0].readout.is_some());
    // One named alone is still refused.
    let one = Request::Probe {
        module: Some("readout2".to_string()),
        modules: Vec::new(),
    };
    assert_eq!(code(ask(&mut wall, &one)), ErrorCode::UnknownModule);
}

#[test]
fn plugging_b_alone_lets_go_of_b_and_keeps_a() {
    let (mut wall, mut engine) =
        probing_wall(&["vco", "lfo", "scope"], &[("vco1.out", "scope1.a")]);
    run_for(&mut wall, &mut engine, 0.4);
    let before = probed(&mut wall, Some("scope1")).probes[0]
        .scope
        .clone()
        .unwrap();
    assert!(before.input_a.hz.is_some());
    // Captures wait in the ring as b is plugged.
    render_only(&mut engine, 4_800);
    let patch = Request::Patch {
        from: "lfo1.out".to_string(),
        to: "scope1.b".to_string(),
        amount: None,
    };
    ask(&mut wall, &patch).unwrap();
    let kept = probed(&mut wall, Some("scope1")).probes[0]
        .scope
        .clone()
        .unwrap();
    // a's reading stays (newer captures of it may have come in).
    let (kept_hz, before_hz) = (kept.input_a.hz.unwrap(), before.input_a.hz.unwrap());
    assert!(
        kept.input_a.connected && !kept.input_a.waiting && (kept_hz - before_hz).abs() < 1.0,
        "{kept:?}"
    );
    assert!(kept.input_b.waiting && kept.b.is_empty(), "{kept:?}");
    // The waiting captures keep a and say nothing of b.
    wall.tick(Instant::now());
    let taken = probed(&mut wall, Some("scope1")).probes[0]
        .scope
        .clone()
        .unwrap();
    assert!(
        taken.input_a.hz.is_some() && taken.input_b.waiting,
        "{taken:?}"
    );
    // Then b is measured.
    run_for(&mut wall, &mut engine, 0.3);
    let after = probed(&mut wall, Some("scope1")).probes[0]
        .scope
        .clone()
        .unwrap();
    assert!(
        after.input_b.connected && !after.input_b.waiting,
        "{after:?}"
    );
    assert!(!after.b.is_empty());
}

#[test]
fn pitch_through_a_slew_or_a_sample_and_hold_is_still_pitch() {
    let (wall, _engine) = probing_wall(
        &[
            "seq", "quant", "slew", "sh", "sh", "readout", "readout", "readout",
        ],
        &[
            ("seq1.pitch", "slew1.in"),
            ("slew1.out", "readout1.in"),
            ("quant1.out", "sh1.in"),
            ("sh1.out", "readout2.in"),
            // A sample and hold of its own noise is a plain CV.
            ("sh2.out", "readout3.in"),
        ],
    );
    let source = |id: &str| wall.source_show(id, &mut wall.trail());
    assert_eq!(source("readout1"), Some(probe::Show::Pitch));
    assert_eq!(source("readout2"), Some(probe::Show::Pitch));
    assert_eq!(source("readout3"), Some(probe::Show::Cv));
}

#[test]
fn pitch_through_the_routers_and_math_is_still_pitch() {
    let (wall, _engine) = probing_wall(
        &[
            "seq", "lfo", "switch", "select", "select", "math", "math", "readout", "readout",
            "readout", "readout", "readout", "switch", "readout",
        ],
        &[
            // A switch sends its in on.
            ("seq1.pitch", "switch1.in"),
            ("switch1.out3", "readout1.in"),
            // A selector of pitches is pitch; of a pitch and an LFO, its
            // own port's (audio).
            ("seq1.pitch", "select1.in1"),
            ("seq1.pitch", "select1.in3"),
            ("select1.out", "readout2.in"),
            ("seq1.pitch", "select2.in1"),
            ("lfo1.out", "select2.in2"),
            ("select2.out", "readout3.in"),
            // A pitch plus a vibrato is pitch; its absolute value is not.
            ("seq1.pitch", "math1.a"),
            ("lfo1.out", "math1.b"),
            ("math1.sum", "readout4.in"),
            ("lfo1.out", "math2.a"),
            ("math2.sum", "readout5.in"),
            ("math1.abs", "readout6.in"),
        ],
    );
    let source = |id: &str| wall.source_show(id, &mut wall.trail());
    assert_eq!(source("readout1"), Some(probe::Show::Pitch));
    assert_eq!(source("readout2"), Some(probe::Show::Pitch));
    assert_eq!(source("readout3"), Some(probe::Show::Audio));
    assert_eq!(source("readout4"), Some(probe::Show::Pitch));
    assert_eq!(source("readout5"), Some(probe::Show::Cv));
    assert_eq!(source("readout6"), Some(probe::Show::Cv));
    // An empty switch sends a steady 1.0, as its outputs say they carry.
    let (empty, _engine) = probing_wall(&["switch", "readout"], &[("switch1.out1", "readout1.in")]);
    assert_eq!(
        empty.source_show("readout1", &mut empty.trail()),
        Some(probe::Show::Audio)
    );
}

#[test]
fn a_loop_of_routers_reads_the_same_whatever_order_its_cables_went_in() {
    // Two selectors feeding each other, one also fed a pitch; a readout
    // on each, a mixer of the pitch and an LFO, and one on that too.
    let kinds = [
        "seq", "lfo", "select", "select", "mix", "readout", "readout", "readout",
    ];
    let cables = [
        ("seq1.pitch", "select1.in1"),
        ("select2.out", "select1.in2"),
        ("select1.out", "select2.in1"),
        ("select1.out", "readout1.in"),
        ("select2.out", "readout2.in"),
        ("seq1.pitch", "mix1.a"),
        ("lfo1.out", "mix1.b"),
        ("mix1.out", "readout3.in"),
    ];
    let read = |cables: &[(&str, &str)]| {
        let (wall, _engine) = probing_wall(&kinds, cables);
        let mut trail = wall.trail();
        // In both orders of asking, on one trail.
        let forward: Vec<_> = ["readout1", "readout2", "readout3"]
            .iter()
            .map(|id| wall.source_show(id, &mut trail))
            .collect();
        let mut trail = wall.trail();
        let mut backward: Vec<_> = ["readout3", "readout2", "readout1"]
            .iter()
            .map(|id| wall.source_show(id, &mut trail))
            .collect();
        backward.reverse();
        assert_eq!(forward, backward);
        forward
    };
    let pitch = Some(probe::Show::Pitch);
    let expected = vec![pitch, pitch, pitch];
    assert_eq!(read(&cables), expected);
    let mut reversed = cables;
    reversed.reverse();
    assert_eq!(read(&reversed), expected);
    // A loop with nothing else in it shows nothing.
    let (alone, _engine) = probing_wall(
        &["select", "select", "readout"],
        &[
            ("select2.out", "select1.in1"),
            ("select1.out", "select2.in1"),
            ("select1.out", "readout1.in"),
        ],
    );
    assert_eq!(alone.source_show("readout1", &mut alone.trail()), None);
    // A scope's probe asks nothing of the cables: only readouts do.
    let (scoped, _engine) = probing_wall(&["seq", "scope"], &[("seq1.pitch", "scope1.a")]);
    let mut trail = scoped.trail();
    scoped.probes_now(Instant::now());
    let found = scoped.probe_of(&scoped.patch.modules()[1], Instant::now(), &mut trail);
    assert!(found.readout.is_none());
    assert_eq!(trail.steps, 0);
}

#[test]
fn a_wall_that_fits_its_file_fits_a_look() {
    // A look shows every knob with its range, unit and words, where the
    // file keeps only its value: the look is larger, by up to
    // LOOK_PER_FILE (store), for a wall of knob-heavy modules with nothing
    // else to them. Checked on such a wall (every kind, no names, no dye)
    // and on one with names, dye from six seats on every module, and a
    // cable into each.
    // Each kind on its own: a wall of a hundred of it, at its defaults.
    for kind in Kind::all() {
        let mut bare = Patch::empty(120.0);
        for _ in 0..100 {
            bare.add(kind.name(), None, None).unwrap();
        }
        let (_engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
        let bare = Wall::new(bare, control, None, Vec::new(), None);
        let file = crate::store::encode(bare.patch()).unwrap().len();
        let look = serde_json::to_vec(&bare.snapshot()).unwrap().len();
        assert!(
            look <= file * crate::store::LOOK_PER_FILE,
            "{kind}: a look of {look} bytes for a file of {file} ({:.2} times)",
            look as f64 / file as f64
        );
    }
    let (mut worn, _worn_engine) = wall();
    let seats: Vec<String> = (1..=6).map(|n| format!("{:x<23}{n}", "seat")).collect();
    let mut n = 0;
    let start = Instant::now();
    for kind in Kind::all() {
        n += 1;
        // A second apart, so each seat keeps within its flood allowance.
        let at = start + Duration::from_secs(n);
        let added = worn.request(
            &format!("Adder{n}"),
            &Request::Add {
                kind: kind.name().to_string(),
                name: Some("x".repeat(24)),
                place: None,
            },
            at,
        );
        let id = changed(added.unwrap().result).module.unwrap();
        let spec = kind.spec();
        for (step, seat) in seats.iter().enumerate() {
            if let Some(knob) = spec.knobs.first() {
                let value = 0.001_f64.mul_add(step as f64, f64::from(knob.min));
                worn.request(seat, &turn(&id, knob.name, value), at)
                    .unwrap();
            }
        }
        if let (Some(knob), true) = (spec.knobs.first(), n > 1) {
            worn.request(
                &format!("Patcher{n}"),
                &patch("vco1.out", &format!("{id}.{}", knob.name)),
                at,
            )
            .unwrap();
        }
    }
    let file = crate::store::encode(worn.patch()).unwrap().len();
    let look = serde_json::to_vec(&worn.snapshot()).unwrap().len();
    assert!(
        look <= file * crate::store::LOOK_PER_FILE,
        "a look of {look} bytes for a file of {file}"
    );
}

#[test]
fn a_small_heavy_wall_takes_additions_apart_and_says_when_it_is_full() {
    let (mut wall, _engine) = probing_wall(&["vco", "vco", "vco"], &[]);
    wall.set_load_guard(true);
    measured_long_enough(&wall);
    // Too small to cost by share, but working: one addition, then wait.
    wall.engine
        .shared()
        .set_render_load(SMALL_WALL_SETTLE + 0.05);
    ask(&mut wall, &add("vco")).unwrap();
    assert_eq!(code(ask(&mut wall, &add("vco"))), ErrorCode::SlowDown);
    // Past the guard, look says full as the load stands, without anyone
    // trying an addition, and stops once it is well below.
    wall.engine.shared().set_render_load(LOAD_GUARD + 0.05);
    assert!(wall.snapshot().engine.unwrap().refusing);
    wall.engine.shared().set_render_load(SMALL_WALL_SETTLE);
    assert!(!wall.snapshot().engine.unwrap().refusing);
}

mod cost;
mod unheard;

mod loops;
