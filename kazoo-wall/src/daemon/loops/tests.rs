use super::*;
use crate::catalogue::Kind;
use crate::patch::{LoopRecord, Patch, TapeRecord};
use crate::protocol::{ErrorCode, LoopOp, Request};

fn record() -> TapeRecord {
    TapeRecord {
        reel: "0".repeat(64),
        rate: 48_000,
        frames: 96_000,
        tail: 384,
        beats: 4.0,
        anchor: 0.0,
        bars: vec![Some(0)],
    }
}
fn patch() -> Patch {
    let mut patch = Patch::empty(120.0);
    patch.add("loop", None, None).unwrap();
    patch.set_loop(
        "loop1",
        LoopRecord {
            tape: Some(record()),
            from: None,
        },
    );
    patch
}
#[test]
fn a_saved_loop_comes_back_with_its_exact_samples() {
    saved_samples();
}
#[test]
fn a_missing_reel_leaves_a_blank_loop_and_a_note_and_the_record_stays() {
    missing_reel();
}
#[test]
fn a_damaged_record_is_dropped_with_a_note() {
    for damaged in [
        TapeRecord {
            beats: f64::NAN,
            ..record()
        },
        TapeRecord {
            frames: 1_000_000_000,
            ..record()
        },
        TapeRecord {
            reel: "xyz".into(),
            ..record()
        },
    ] {
        let mut file = patch().to_file();
        file.loops.get_mut("loop1").unwrap().tape = Some(damaged);
        let (loaded, notes) = Patch::load(&file).unwrap();
        assert_eq!(loaded.module("loop1").unwrap().kind, Kind::LOOP);
        assert!(loaded.loop_record("loop1").is_none());
        assert!(
            notes
                .iter()
                .any(|note| note.contains("tape was unreadable"))
        );
    }
}
#[test]
fn erase_then_undo_brings_the_tape_back() {
    let mut patch = patch();
    let what = patch.loop_op("loop1", &LoopOp::Erase).unwrap();
    assert!(patch.loop_record("loop1").unwrap().tape.is_none());
    patch.undo(&what).unwrap();
    assert_eq!(patch.loop_record("loop1").unwrap().tape, Some(record()));
}
#[test]
fn remove_then_undo_brings_the_loop_and_its_tape_back() {
    let mut patch = patch();
    let what = patch.remove("loop1", &[]).unwrap();
    assert_eq!(patch.to_file().retired_loops.len(), 1);
    patch.undo(&what).unwrap();
    assert_eq!(patch.loop_record("loop1").unwrap().tape, Some(record()));
    assert!(patch.to_file().retired_loops.is_empty());
}
#[test]
fn the_ninth_loop_is_refused_by_name() {
    let mut patch = Patch::empty(120.0);
    for _ in 0..MAX_LOOPS {
        patch.add("loop", None, None).unwrap();
    }
    let error = patch.add("loop", None, None).unwrap_err();
    assert_eq!(error.code, ErrorCode::Full);
    assert!(
        error
            .message
            .contains("a wall holds eight loops, like the eight-track")
    );
}
#[test]
fn from_is_kept_in_the_save_and_checked() {
    let mut patch = patch();
    let what = patch
        .loop_op(
            "loop1",
            &LoopOp::From {
                from: Some("mouth/dry".into()),
            },
        )
        .unwrap();
    let (loaded, notes) = Patch::load(&patch.to_file()).unwrap();
    assert!(notes.is_empty());
    assert_eq!(
        loaded.loop_record("loop1").unwrap().from.as_deref(),
        Some("mouth/dry")
    );
    for name in ["a b".to_string(), "a".repeat(25)] {
        assert_eq!(
            patch
                .loop_op("loop1", &LoopOp::From { from: Some(name) })
                .unwrap_err()
                .code,
            ErrorCode::BadName
        );
    }
    patch.undo(&what).unwrap();
    assert!(patch.loop_record("loop1").unwrap().from.is_none());
    let request = Request::Loop {
        module: "loop1".into(),
        op: LoopOp::Erase,
    };
    assert_eq!(request.op(), "loop");
}
#[test]
fn a_loop_nobody_hears_is_still_rendered() {
    assert!(crate::daemon::wall::unheard::is_sink(Kind::LOOP));
    let dir = tempfile::tempdir().unwrap();
    let mut loops = Loops::with_reels_dir(dir.path().to_path_buf());
    let mut bench = bench(&mut loops);
    bench.beat = -32.0 / 24_000.0;
    bench.knob("record", 1.0);
    render(&mut bench, 96_032, 0.5);
    assert_eq!(
        loops.gauges("loop1").unwrap().written[0].load(Ordering::Relaxed) & 1,
        1
    );
    bench.knob("record", 0.0);
    render(&mut bench, 416, 0.5);
    assert_eq!(
        loops.gauges("loop1").unwrap().state.load(Ordering::Relaxed),
        LoopState::Holding as u8
    );
    loops.collect();
    assert_eq!(loops.takes, 1);
}
#[test]
fn a_reel_arriving_after_a_recording_began_does_not_overwrite_it() {
    late_reel();
}
#[test]
fn an_engine_rebuilt_after_a_device_change_puts_the_tapes_back() {
    rebuilt();
}
#[test]
fn an_old_save_without_loops_reads_as_before() {
    let file = Patch::empty(120.0).to_file();
    let text = serde_json::to_string(&file).unwrap();
    assert!(!text.contains("loops"));
    let (patch, notes) = Patch::load(&serde_json::from_str(&text).unwrap()).unwrap();
    assert!(notes.is_empty());
    assert!(patch.loops().is_empty());
}

use crate::dsp::looper::{LoopState, build_loop};
use crate::dsp::testing::Bench;
use kazoo_core::reels::ReelStore;
use std::sync::atomic::Ordering;

fn fixture() -> (tempfile::TempDir, TapeRecord, Vec<f32>) {
    let dir = tempfile::tempdir().unwrap();
    let samples: Vec<f32> = (0..96_384)
        .flat_map(|frame| {
            let value = ((frame % 127) as f32 - 63.0) / 128.0;
            [value, -value]
        })
        .collect();
    let id = ReelStore::open(dir.path())
        .unwrap()
        .put(48_000, &samples)
        .unwrap();
    (
        dir,
        TapeRecord {
            reel: id.to_string(),
            ..record()
        },
        samples,
    )
}
fn bench(loops: &mut Loops) -> Bench {
    let mut bench = Bench::new(Kind::LOOP);
    let (module, feed) = build_loop(48_000.0);
    bench.module = module;
    bench.knob("level", 1.0).knob("bars", 1.0);
    loops.attach("loop1", feed, 48_000.0);
    bench
}
fn render(bench: &mut Bench, frames: usize, input: f32) -> Vec<f32> {
    let mut output = Vec::new();
    bench.connected[0] = true;
    bench.inputs[0].fill(input);
    for _ in (0..frames).step_by(crate::SUB_BLOCK) {
        assert_no_alloc::assert_no_alloc(|| bench.step());
        output.extend_from_slice(&bench.outputs[0]);
    }
    output
}
fn saved_samples() {
    let (dir, tape, samples) = fixture();
    let mut renders = Vec::new();
    for _ in 0..2 {
        let mut loops = Loops::with_reels_dir(dir.path().to_path_buf());
        let mut bench = bench(&mut loops);
        loops.put_back("loop1", tape.clone());
        loops.wait_for_test();
        bench.beat = 8.0;
        let output = render(&mut bench, 4 * 96_000, 0.0);
        assert_eq!(
            loops.gauges("loop1").unwrap().state.load(Ordering::Relaxed),
            LoopState::Holding as u8
        );
        for (frame, sample) in output.iter().enumerate() {
            let within = frame % 96_000;
            if (384..95_616).contains(&within) {
                assert!(
                    (*sample - samples[2 * within]).abs() < 1e-6,
                    "frame {frame}"
                );
            }
        }
        renders.push(
            output
                .iter()
                .map(|sample| sample.to_bits())
                .collect::<Vec<_>>(),
        );
    }
    assert_eq!(renders[0], renders[1]);
}
fn missing_reel() {
    let dir = tempfile::tempdir().unwrap();
    let mut loops = Loops::with_reels_dir(dir.path().to_path_buf());
    let mut bench = bench(&mut loops);
    let patch = patch();
    loops.put_back("loop1", record());
    loops.wait_for_test();
    assert!(
        render(&mut bench, 32, 0.0)
            .iter()
            .all(|sample| *sample == 0.0)
    );
    assert!(loops.notes().iter().any(|note| note.contains("missing")));
    assert_eq!(patch.loop_record("loop1").unwrap().tape, Some(record()));
}
fn late_reel() {
    let (dir, tape, _) = fixture();
    let mut loops = Loops::with_reels_dir(dir.path().to_path_buf());
    let mut bench = bench(&mut loops);
    loops.put_back("loop1", tape);
    bench.beat = -32.0 / 24_000.0;
    bench.knob("record", 1.0);
    render(&mut bench, 32 + 96_000, 0.75);
    bench.knob("record", 0.0);
    render(&mut bench, 416, 0.75);
    loops.wait_for_test();
    bench.beat = 8.0;
    let output = render(&mut bench, 1024, 0.0);
    loops.collect();
    assert!(
        output[384..]
            .iter()
            .all(|sample| (*sample - 0.75).abs() < 1e-6)
    );
    assert!(
        loops
            .notes()
            .iter()
            .any(|note| note.contains("recorded before its saved tape arrived"))
    );
}
fn rebuilt() {
    let (dir, tape, _) = fixture();
    let mut loops = Loops::with_reels_dir(dir.path().to_path_buf());
    let mut first = bench(&mut loops);
    loops.put_back("loop1", tape.clone());
    loops.wait_for_test();
    render(&mut first, 32, 0.0);
    loops.replace_wall();
    let mut second = bench(&mut loops);
    loops.put_back("loop1", tape);
    loops.wait_for_test();
    render(&mut second, 32, 0.0);
    assert_eq!(
        loops.gauges("loop1").unwrap().state.load(Ordering::Relaxed),
        LoopState::Holding as u8
    );
}
