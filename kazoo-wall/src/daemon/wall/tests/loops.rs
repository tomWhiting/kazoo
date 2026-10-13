use super::*;
use crate::daemon::loops::Loops;
use crate::dsp::looper::LoopState;
use crate::patch::{LoopRecord, TapeRecord};
use crate::protocol::LoopOp;
use kazoo_core::reels::ReelStore;

fn saved_wall() -> (Wall, Engine, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let samples = vec![0.25; 2 * 96_384];
    let reel = ReelStore::open(dir.path())
        .unwrap()
        .put(48_000, &samples)
        .unwrap();
    let mut patch = Patch::empty(120.0);
    patch.add("loop", None, None).unwrap();
    patch.add("out", None, None).unwrap();
    patch.plug("loop1.left", "out1.left", None, None).unwrap();
    patch.set_loop(
        "loop1",
        LoopRecord {
            tape: Some(TapeRecord {
                reel: reel.to_string(),
                rate: 48_000,
                frames: 96_000,
                tail: 384,
                beats: 4.0,
                anchor: 0.0,
                bars: vec![Some(0)],
            }),
            from: None,
        },
    );
    let (mut wall, engine) = wall();
    wall.loops = Loops::with_reels_dir(dir.path().to_path_buf());
    wall.patch = patch;
    wall.swap_engine();
    (wall, engine, dir)
}
fn finish_tape(wall: &mut Wall, engine: &mut Engine) {
    wall.loops.wait_for_test();
    let mut output = [0.0; 64];
    engine.render(&mut output, 2);
    wall.tick(Instant::now());
}
#[test]
fn wall_erase_and_undo_reinstall_the_saved_reel() {
    let (mut wall, mut engine, _dir) = saved_wall();
    finish_tape(&mut wall, &mut engine);
    assert_eq!(
        wall.loops
            .gauges("loop1")
            .unwrap()
            .state
            .load(Ordering::Relaxed),
        LoopState::Holding as u8
    );
    let erased = changed(
        ask(
            &mut wall,
            &Request::Loop {
                module: "loop1".into(),
                op: LoopOp::Erase,
            },
        )
        .unwrap(),
    );
    finish_tape(&mut wall, &mut engine);
    assert_eq!(
        wall.loops
            .gauges("loop1")
            .unwrap()
            .state
            .load(Ordering::Relaxed),
        LoopState::Blank as u8
    );
    ask(
        &mut wall,
        &Request::Undo {
            change: erased.change.seq,
        },
    )
    .unwrap();
    finish_tape(&mut wall, &mut engine);
    assert_eq!(
        wall.loops
            .gauges("loop1")
            .unwrap()
            .state
            .load(Ordering::Relaxed),
        LoopState::Holding as u8
    );
}
#[test]
fn wall_remove_and_undo_reinstall_the_saved_reel() {
    let (mut wall, mut engine, _dir) = saved_wall();
    finish_tape(&mut wall, &mut engine);
    let removed = changed(
        ask(
            &mut wall,
            &Request::Remove {
                module: "loop1".into(),
            },
        )
        .unwrap(),
    );
    assert_eq!(wall.patch.to_file().retired_loops.len(), 1);
    engine.render(&mut [0.0; 64], 2);
    wall.tick(Instant::now());
    ask(
        &mut wall,
        &Request::Undo {
            change: removed.change.seq,
        },
    )
    .unwrap();
    finish_tape(&mut wall, &mut engine);
    assert_eq!(
        wall.loops
            .gauges("loop1")
            .unwrap()
            .state
            .load(Ordering::Relaxed),
        LoopState::Holding as u8
    );
}
#[test]
fn wall_device_attach_reinstalls_the_saved_reel() {
    let (mut wall, mut engine, _dir) = saved_wall();
    finish_tape(&mut wall, &mut engine);
    let (mut replacement, control) =
        crate::engine::engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    wall.attach(control, None);
    finish_tape(&mut wall, &mut replacement);
    assert_eq!(
        wall.loops
            .gauges("loop1")
            .unwrap()
            .state
            .load(Ordering::Relaxed),
        LoopState::Holding as u8
    );
}
#[test]
fn wall_renders_a_loop_without_any_output_cable() {
    let (mut wall, mut engine, _dir) = saved_wall();
    finish_tape(&mut wall, &mut engine);
    ask(
        &mut wall,
        &Request::Remove {
            module: "out1".into(),
        },
    )
    .unwrap();
    assert!(!wall.unheard.iter().any(|module| module == "loop1"));
    engine.render(&mut [0.0; 64], 2);
    assert_eq!(
        wall.loops
            .gauges("loop1")
            .unwrap()
            .state
            .load(Ordering::Relaxed),
        LoopState::Holding as u8
    );
}
#[test]
fn tree_text_keeps_active_and_retired_loop_records() {
    let (wall, _engine, _dir) = saved_wall();
    let file = wall.patch.to_file();
    let tree = crate::tree::to_files(&file).unwrap();
    assert_eq!(crate::tree::from_files(&tree).unwrap(), file);
    let mut patch = wall.patch;
    patch.remove("loop1", &[]).unwrap();
    let file = patch.to_file();
    let tree = crate::tree::to_files(&file).unwrap();
    assert_eq!(crate::tree::from_files(&tree).unwrap(), file);
}
