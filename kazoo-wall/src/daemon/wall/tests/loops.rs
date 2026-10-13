//! A whole wall keeps its loops: a save's tape put back on load, erase,
//! remove, their undos and a rebuilt engine; a loop nobody hears records;
//! eight loops at most; the tree keeps every record. Sample-exact checks
//! of the same paths are in `daemon/loops/tests.rs`.
use super::*;
use crate::daemon::loops::Loops;
use crate::daemon::loops::tests::{BAR, put_reel_of, record, reels_dir, saved};
use crate::dsp::looper::{LoopGauges, LoopState};
use crate::patch::{LoopRecord, TapeRecord};
use crate::protocol::LoopOp;

/// A wall that has just loaded `patch`, its reel store at `dir`.
fn loaded(patch: Patch, dir: &std::path::Path) -> (Wall, Engine) {
    let (mut wall, engine) = wall();
    wall.loops = Loops::with_reels_dir(dir.to_path_buf());
    wall.patch = patch;
    wall.swap_engine();
    (wall, engine)
}

/// A saved wall of `loop1` (holding `tape`) patched into `out1`.
fn saved_wall(tape: Option<TapeRecord>, dir: &std::path::Path) -> (Wall, Engine) {
    let (mut patch, notes) = saved(tape);
    assert!(notes.is_empty(), "{notes:?}");
    patch.add("out", None, None).unwrap();
    patch.plug("loop1.left", "out1.left", None, None).unwrap();
    loaded(patch, dir)
}

fn render(engine: &mut Engine, frames: usize) {
    let mut data = vec![0.0; 2 * 4_096];
    let mut left = frames;
    while left > 0 {
        let now = left.min(4_096);
        engine.render(&mut data[..2 * now], 2);
        left -= now;
    }
}

/// The tape thread's answer arrives (within two seconds), a tick hands it
/// to the loop, and the engine renders the sub-block that takes it.
fn tape_arrives(wall: &mut Wall, engine: &mut Engine) {
    let asked = Instant::now();
    wall.loops.wait_for_test();
    assert!(asked.elapsed() < Duration::from_secs(2));
    wall.tick(Instant::now());
    render(engine, 4_096);
    wall.tick(Instant::now());
}

fn gauges(wall: &Wall) -> &LoopGauges {
    wall.loops.gauges("loop1").unwrap()
}

fn state(wall: &Wall) -> u8 {
    gauges(wall).state.load(Ordering::Relaxed)
}

/// The loudest the loop played since this was last asked.
fn peak(wall: &Wall) -> f32 {
    f32::from_bits(gauges(wall).peak_left.swap(0, Ordering::Relaxed))
}

/// The reel's peak, at the level knob's default.
const LOUD: f32 = 0.8 * 0.5;

/// A bar of a 500 Hz sine at half scale: smooth, so the engine's head,
/// which reads between frames, plays its peak within a hair (sample-exact
/// reads are in `daemon/loops/tests.rs`).
fn put_reel(dir: &std::path::Path) -> (TapeRecord, Vec<f32>) {
    put_reel_of(dir, |frame| {
        0.5 * (std::f32::consts::TAU * (frame % 96) as f32 / 96.0).sin()
    })
}

fn holds_the_reel(wall: &Wall, engine: &mut Engine) {
    assert_eq!(state(wall), LoopState::Holding as u8);
    assert_eq!(gauges(wall).frames.load(Ordering::Relaxed), BAR as u64);
    assert_eq!(
        gauges(wall).beats.load(Ordering::Relaxed),
        4.0_f64.to_bits()
    );
    peak(wall);
    render(engine, BAR);
    let loudest = peak(wall);
    assert!(
        (loudest - LOUD).abs() < 0.002,
        "the loop played at most {loudest}, not {LOUD}"
    );
}

fn summary(value: serde_json::Value) -> (u64, String) {
    let result = changed(value);
    (result.change.seq, result.change.summary)
}

#[test]
fn a_loaded_wall_puts_its_loops_tape_back() {
    let dir = reels_dir();
    let (tape, _) = put_reel(dir.path());
    let (mut wall, mut engine) = saved_wall(Some(tape), dir.path());
    assert_eq!(state(&wall), LoopState::Blank as u8);
    tape_arrives(&mut wall, &mut engine);
    holds_the_reel(&wall, &mut engine);
    assert_eq!(wall.loops.notes().count(), 0);
}

#[test]
fn a_missing_reel_on_a_loaded_wall_is_a_note_and_the_record_stays() {
    let dir = reels_dir();
    let (mut wall, mut engine) = saved_wall(Some(record()), dir.path());
    tape_arrives(&mut wall, &mut engine);
    render(&mut engine, BAR);
    assert_eq!(state(&wall), LoopState::Blank as u8);
    assert_eq!(peak(&wall).to_bits(), 0);
    assert!(
        wall.loops
            .notes()
            .any(|note| note.starts_with("loop1's reel 00000000 is missing from "))
    );
    assert_eq!(
        wall.patch.to_file().loops["loop1"].tape.as_ref(),
        Some(&record())
    );
}

#[test]
fn wall_erase_and_undo_bring_the_tape_back() {
    let dir = reels_dir();
    let (tape, _) = put_reel(dir.path());
    let (mut wall, mut engine) = saved_wall(Some(tape.clone()), dir.path());
    tape_arrives(&mut wall, &mut engine);
    holds_the_reel(&wall, &mut engine);

    let erase = Request::Loop {
        module: "loop1".into(),
        op: LoopOp::Erase,
    };
    let (seq, said) = summary(ask(&mut wall, &erase).unwrap());
    assert_eq!(
        said,
        format!("Tom erased loop1 (it held 1 bar, reel {})", &tape.reel[..8])
    );
    assert!(wall.patch.loop_record("loop1").is_none());
    tape_arrives(&mut wall, &mut engine);
    assert_eq!(state(&wall), LoopState::Blank as u8);
    peak(&wall);
    render(&mut engine, BAR);
    assert_eq!(peak(&wall).to_bits(), 0, "an erased loop is silent");

    let (_, said) = summary(ask(&mut wall, &Request::Undo { change: seq }).unwrap());
    assert_eq!(
        said,
        format!(
            "Tom undid change {seq}: put loop1's tape back (1 bar, reel {})",
            &tape.reel[..8]
        )
    );
    assert_eq!(
        wall.patch
            .loop_record("loop1")
            .and_then(|kept| kept.tape.as_ref()),
        Some(&tape)
    );
    tape_arrives(&mut wall, &mut engine);
    holds_the_reel(&wall, &mut engine);
}

#[test]
fn wall_remove_and_undo_bring_the_loop_and_its_tape_back() {
    let dir = reels_dir();
    let (tape, _) = put_reel(dir.path());
    let (mut wall, mut engine) = saved_wall(Some(tape.clone()), dir.path());
    tape_arrives(&mut wall, &mut engine);
    let removed = changed(
        ask(
            &mut wall,
            &Request::Remove {
                module: "loop1".into(),
            },
        )
        .unwrap(),
    );
    let file = wall.patch.to_file();
    assert!(file.loops.is_empty());
    assert_eq!(file.retired_loops.len(), 1);
    assert_eq!(file.retired_loops[0].record.tape, Some(tape));
    assert!(wall.loops.gauges("loop1").is_none());
    render(&mut engine, crate::SUB_BLOCK);
    wall.tick(Instant::now());
    ask(
        &mut wall,
        &Request::Undo {
            change: removed.change.seq,
        },
    )
    .unwrap();
    assert!(wall.patch.to_file().retired_loops.is_empty());
    tape_arrives(&mut wall, &mut engine);
    holds_the_reel(&wall, &mut engine);
}

#[test]
fn the_ninth_loop_on_a_wall_is_refused_by_name() {
    let (mut wall, _engine) = loaded(Patch::empty(120.0), reels_dir().path());
    let add = Request::Add {
        kind: "loop".into(),
        name: None,
        place: None,
    };
    for _ in 0..8 {
        ask(&mut wall, &add).unwrap();
    }
    let before = wall.patch.to_file();
    let refused = ask(&mut wall, &add).unwrap_err();
    assert_eq!(refused.code, ErrorCode::Full);
    assert_eq!(
        refused.message,
        "a wall holds eight loops, like the eight-track; remove one, or erase one and use it again"
    );
    assert_eq!(wall.patch.to_file(), before);
}

#[test]
fn a_seat_sets_and_undoes_where_a_loop_records_from() {
    let dir = reels_dir();
    let (mut wall, _engine) = saved_wall(None, dir.path());
    let from = |name: &str| Request::Loop {
        module: "loop1".into(),
        op: LoopOp::From {
            from: Some(name.into()),
        },
    };
    let (seq, said) = summary(ask(&mut wall, &from("mouth/dry")).unwrap());
    assert_eq!(said, "Tom set loop1 to record from mouth/dry");
    assert_eq!(code(ask(&mut wall, &from("a b"))), ErrorCode::BadName);
    assert_eq!(
        code(ask(&mut wall, &from(&"a".repeat(25)))),
        ErrorCode::BadName
    );
    assert_eq!(
        code(ask(
            &mut wall,
            &Request::Loop {
                module: "out1".into(),
                op: LoopOp::Erase,
            }
        )),
        ErrorCode::NotAllowed
    );
    let text = String::from_utf8(crate::store::encode(&wall.patch).unwrap()).unwrap();
    assert!(text.contains("\"from\": \"mouth/dry\""), "{text}");
    let (_, said) = summary(ask(&mut wall, &Request::Undo { change: seq }).unwrap());
    assert_eq!(
        said,
        format!("Tom undid change {seq}: set loop1 to record what is patched into it")
    );
    assert!(wall.patch.loop_record("loop1").is_none());
}

#[test]
fn a_loop_nobody_hears_is_still_rendered() {
    let dir = reels_dir();
    let (mut wall, mut engine) = loaded(Patch::empty(120.0), dir.path());
    for kind in ["loop", "vco"] {
        ask(
            &mut wall,
            &Request::Add {
                kind: kind.into(),
                name: None,
                place: None,
            },
        )
        .unwrap();
    }
    ask(
        &mut wall,
        &Request::Patch {
            from: "vco1.out".into(),
            to: "loop1.left".into(),
            amount: None,
        },
    )
    .unwrap();
    ask(&mut wall, &turn("loop1", "bars", 1.0)).unwrap();
    ask(&mut wall, &turn("loop1", "record", 1.0)).unwrap();
    // Nothing is patched out of the loop: it and the vco feeding it are
    // heard all the same.
    assert!(
        wall.patch
            .cables()
            .iter()
            .all(|cable| cable.from_module != "loop1")
    );
    assert!(wall.unheard.is_empty(), "{:?}", wall.unheard);
    // Armed through bar 0, recording through bar 1.
    render(&mut engine, BAR + BAR / 2);
    assert_eq!(state(&wall), LoopState::Rolling as u8);
    assert_eq!(gauges(&wall).written[0].load(Ordering::Relaxed) & 1, 1);
    ask(&mut wall, &turn("loop1", "record", 0.0)).unwrap();
    // The punch runs on to the bar line and fades out after it.
    let mut rendered = 0;
    while state(&wall) != LoopState::Holding as u8 {
        assert!(rendered < BAR, "still {} a bar later", state(&wall));
        render(&mut engine, 1_024);
        rendered += 1_024;
    }
    assert_eq!(gauges(&wall).frames.load(Ordering::Relaxed), BAR as u64);
    wall.tick(Instant::now());
    assert_eq!(wall.loops.takes(), 1);
    peak(&wall);
    render(&mut engine, BAR);
    assert!(peak(&wall) > 0.1, "the loop plays the vco it recorded");
}

#[test]
fn an_engine_rebuilt_after_a_device_change_puts_the_tapes_back() {
    let dir = reels_dir();
    let (tape, _) = put_reel(dir.path());
    let (mut wall, mut engine) = saved_wall(Some(tape), dir.path());
    tape_arrives(&mut wall, &mut engine);
    let (mut replacement, control) =
        crate::engine::engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    wall.attach(control, None);
    assert_eq!(state(&wall), LoopState::Blank as u8);
    tape_arrives(&mut wall, &mut replacement);
    holds_the_reel(&wall, &mut replacement);
    drop(engine);
}

#[test]
fn the_tree_keeps_active_and_retired_loop_records() {
    let dir = reels_dir();
    let tape = TapeRecord {
        beats: 12.0,
        anchor: -4.5,
        bars: vec![Some(3), None, Some(-1)],
        ..record()
    };
    let (mut patch, _) = saved(Some(tape));
    patch
        .loop_op(
            "loop1",
            &LoopOp::From {
                from: Some("808/kick".into()),
            },
        )
        .unwrap();
    patch.add("loop", None, None).unwrap();
    patch
        .set_loop(
            "loop2",
            LoopRecord {
                tape: None,
                from: Some("master".into()),
            },
        )
        .unwrap();
    let (wall, _engine) = loaded(patch, dir.path());
    let file = wall.patch.to_file();
    let tree = crate::tree::to_files(&file).unwrap();
    let loop1 = String::from_utf8(tree["modules/loop1.toml"].clone()).unwrap();
    assert!(
        loop1.contains("\n[loop]\nfrom = \"808/kick\"\n\n[loop.tape]\n"),
        "{loop1}"
    );
    assert!(
        loop1.contains("bars = [{ pass = 3 }, {}, { pass = -1 }]"),
        "{loop1}"
    );
    assert_eq!(crate::tree::from_files(&tree).unwrap(), file);
    let mut patch = wall.patch;
    patch.remove("loop1", &[]).unwrap();
    patch.remove("loop2", &[]).unwrap();
    let file = patch.to_file();
    assert_eq!(file.retired_loops.len(), 2);
    let tree = crate::tree::to_files(&file).unwrap();
    let text = String::from_utf8(tree["wall.toml"].clone()).unwrap();
    assert!(
        text.contains("[[retired_loops]]\nmodule = \"loop1\""),
        "{text}"
    );
    assert!(text.contains("[retired_loops.tape]"), "{text}");
    assert_eq!(crate::tree::from_files(&tree).unwrap(), file);
    // A loop's tape for a module the wall does not hold is no tree.
    let mut stray = file;
    stray.loops.insert("loop9".into(), LoopRecord::default());
    assert!(crate::tree::to_files(&stray).is_err());
}
