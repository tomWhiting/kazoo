//! The loops' tape on the control side, sample-exact on a bench: saved
//! tape put back, erased and put back again, a late reel refused, and the
//! patch's records. The same paths through a whole wall are in
//! `daemon/wall/tests/loops.rs`.
use super::*;
use crate::catalogue::Kind;
use crate::dsp::looper::{LoopState, build_loop};
use crate::dsp::testing::Bench;
use crate::patch::{LoopRecord, Patch, PatchFile, TapeRecord};
use crate::protocol::{ErrorCode, LoopOp, Request, What};
use kazoo_core::reels::ReelStore;
use std::sync::atomic::Ordering;

/// One bar at 120 BPM and 48 kHz.
pub const BAR: usize = 96_000;
/// The seam's tail at 48 kHz.
pub const TAIL: usize = 384;
/// The level knob's default: the loop plays `LEVEL ×` its tape.
const LEVEL: f32 = 0.8;

/// A reel store of the test's own under the workspace's `target`, never
/// in the system's temporary directory and never `~/.kazoo`.
pub fn reels_dir() -> tempfile::TempDir {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/kz-test-reels");
    std::fs::create_dir_all(&root).unwrap();
    tempfile::Builder::new()
        .prefix("loops-")
        .tempdir_in(root)
        .unwrap()
}

/// A one-bar stereo reel whose every frame differs from its neighbours.
pub fn put_reel(dir: &std::path::Path) -> (TapeRecord, Vec<f32>) {
    put_reel_of(dir, |frame| ((frame % 127) as f32 - 63.0) / 128.0)
}

/// A one-bar stereo reel of `left` frame by frame, its right the inverse,
/// as a loop records it: faded in over the seam's length at its start,
/// and out over its tail, so the seam's crossfade sums to no more than
/// the signal.
pub fn put_reel_of(dir: &std::path::Path, left: impl Fn(usize) -> f32) -> (TapeRecord, Vec<f32>) {
    let samples: Vec<f32> = (0..BAR + TAIL)
        .flat_map(|frame| {
            let gain = if frame < TAIL {
                frame as f32 / TAIL as f32
            } else if frame >= BAR {
                1.0 - (frame - BAR) as f32 / TAIL as f32
            } else {
                1.0
            };
            let value = gain * left(frame);
            [value, -value]
        })
        .collect();
    let id = ReelStore::open(dir).unwrap().put(48_000, &samples).unwrap();
    (
        TapeRecord {
            reel: id.to_string(),
            ..record()
        },
        samples,
    )
}

/// A record that reads, naming a reel no store holds.
pub fn record() -> TapeRecord {
    TapeRecord {
        reel: "0".repeat(64),
        rate: 48_000,
        frames: BAR as u64,
        tail: TAIL as u64,
        beats: 4.0,
        anchor: 0.0,
        bars: vec![Some(0)],
    }
}

/// A save holding `loop1` with `tape`.
fn saved_file(tape: Option<TapeRecord>) -> PatchFile {
    let mut patch = Patch::empty(120.0);
    patch.add("loop", None, None).unwrap();
    let mut file = patch.to_file();
    file.loops
        .insert("loop1".into(), LoopRecord { tape, from: None });
    file
}

/// A save holding `loop1` with `tape`, written as text and read back as
/// the wall reads a save.
pub fn saved(tape: Option<TapeRecord>) -> (Patch, Vec<String>) {
    let text = serde_json::to_vec_pretty(&saved_file(tape)).unwrap();
    let (file, mut notes) = crate::migrate::read(&text).unwrap();
    let (patch, more) = Patch::load(&file).unwrap();
    notes.extend(more);
    (patch, notes)
}

fn bench(loops: &mut Loops) -> Bench {
    let mut bench = Bench::new(Kind::LOOP);
    let (module, feed) = build_loop(48_000.0);
    bench.module = module;
    bench.knob("bars", 1.0);
    loops.attach("loop1", feed, 48_000.0);
    bench
}

fn render(bench: &mut Bench, frames: usize, input: f32) -> Vec<f32> {
    let mut output = Vec::with_capacity(frames);
    bench.connected[0] = true;
    bench.inputs[0].fill(input);
    for _ in (0..frames).step_by(crate::SUB_BLOCK) {
        assert_no_alloc::assert_no_alloc(|| bench.step());
        output.extend_from_slice(&bench.outputs[0]);
    }
    output
}

fn state(loops: &Loops) -> u8 {
    loops.gauges("loop1").unwrap().state.load(Ordering::Relaxed)
}

/// The loop plays `LEVEL ×` the reel on every frame past the seam's
/// crossfade, wherever the beat puts its head.
fn assert_plays(output: &[f32], samples: &[f32]) {
    for (frame, sample) in output.iter().enumerate() {
        let within = frame % BAR;
        if (TAIL..BAR - 2).contains(&within) {
            let wanted = LEVEL * samples[2 * within];
            assert!(
                (*sample - wanted).abs() < 1e-6,
                "frame {frame}: {sample}, not {wanted}"
            );
        }
    }
}

#[test]
fn a_saved_loop_comes_back_with_its_exact_samples() {
    let dir = reels_dir();
    let (tape, samples) = put_reel(dir.path());
    let mut renders = Vec::new();
    for _ in 0..2 {
        let (patch, notes) = saved(Some(tape.clone()));
        assert!(notes.is_empty(), "{notes:?}");
        let mut loops = Loops::with_reels_dir(dir.path().to_path_buf());
        let mut bench = bench(&mut loops);
        let record = patch.loop_record("loop1").unwrap().tape.clone().unwrap();
        loops.put_back("loop1", record);
        let asked = std::time::Instant::now();
        loops.wait_for_test();
        render(&mut bench, crate::SUB_BLOCK, 0.0);
        assert_eq!(state(&loops), LoopState::Holding as u8);
        assert!(asked.elapsed() < std::time::Duration::from_secs(2));
        bench.beat = 8.0;
        let output = render(&mut bench, 4 * BAR, 0.0);
        assert_plays(&output, &samples);
        renders.push(output.iter().map(|x| x.to_bits()).collect::<Vec<_>>());
    }
    assert_eq!(renders[0], renders[1]);
}

#[test]
fn a_missing_reel_leaves_a_blank_loop_and_a_note_and_the_record_stays() {
    let dir = reels_dir();
    let (patch, _) = saved(Some(record()));
    let mut loops = Loops::with_reels_dir(dir.path().to_path_buf());
    let mut bench = bench(&mut loops);
    loops.put_back("loop1", record());
    loops.wait_for_test();
    bench.beat = 8.0;
    assert!(render(&mut bench, BAR, 0.0).iter().all(|x| *x == 0.0));
    assert_eq!(state(&loops), LoopState::Blank as u8);
    let wanted = format!(
        "loop1's reel 00000000 is missing from {}; it is blank",
        dir.path().display()
    );
    assert!(
        loops.notes().any(|note| note == wanted),
        "{:?}",
        loops.notes().collect::<Vec<_>>()
    );
    assert_eq!(
        patch
            .loop_record("loop1")
            .and_then(|kept| kept.tape.as_ref()),
        Some(&record())
    );
    assert_eq!(
        patch.to_file().loops["loop1"].tape.as_ref(),
        Some(&record())
    );
}

#[test]
fn a_damaged_record_is_dropped_with_a_note() {
    for (damaged, why) in [
        (
            TapeRecord {
                beats: f64::NAN,
                ..record()
            },
            "beats outside 1 to 260",
        ),
        (
            TapeRecord {
                frames: 1_000_000_000,
                ..record()
            },
            "frames outside tape capacity",
        ),
        (
            TapeRecord {
                reel: "xyz".into(),
                ..record()
            },
            "64 lowercase hexadecimal digits",
        ),
    ] {
        // NaN has no JSON: a file read from disk says null (below); a file
        // in memory can hold anything.
        let (loaded, notes) = Patch::load(&saved_file(Some(damaged))).unwrap();
        assert_eq!(loaded.module("loop1").unwrap().kind, Kind::LOOP);
        assert!(loaded.loop_record("loop1").is_none());
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(
            notes[0].starts_with("loop1's tape was unreadable in the file (")
                && notes[0].contains(why)
                && notes[0].ends_with("); it is blank"),
            "{notes:?}"
        );
    }
    // On disk, a number that does not read leaves the record out, and
    // the loop stays.
    let text = serde_json::to_string(&saved_file(Some(record())))
        .unwrap()
        .replace("\"beats\":4.0", "\"beats\":null");
    let (file, notes) = crate::migrate::read(text.as_bytes()).unwrap();
    let (loaded, more) = Patch::load(&file).unwrap();
    assert!(more.is_empty(), "{more:?}");
    assert_eq!(loaded.module("loop1").unwrap().kind, Kind::LOOP);
    assert!(loaded.loop_record("loop1").is_none());
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(
        notes[0].starts_with("loop tape 'loop1' could not be read (")
            && notes[0].ends_with("; left out"),
        "{notes:?}"
    );
}

#[test]
fn erase_then_undo_brings_the_tape_back() {
    let dir = reels_dir();
    let (tape, samples) = put_reel(dir.path());
    let (mut patch, _) = saved(Some(tape.clone()));
    let mut loops = Loops::with_reels_dir(dir.path().to_path_buf());
    let mut bench = bench(&mut loops);
    loops.put_back("loop1", tape.clone());
    loops.wait_for_test();
    bench.beat = 8.0;
    assert_plays(&render(&mut bench, BAR, 0.0), &samples);

    let erased = patch.loop_op("loop1", &LoopOp::Erase).unwrap();
    assert!(patch.loop_record("loop1").is_none());
    assert_eq!(
        crate::change::describe(&erased),
        format!("erased loop1 (it held 1 bar, reel {})", &tape.reel[..8])
    );
    loops.replace("loop1", None, false);
    loops.wait_for_test();
    // Silent from the first sub-block after the blank arrives.
    assert!(render(&mut bench, BAR, 0.0).iter().all(|x| *x == 0.0));
    assert_eq!(state(&loops), LoopState::Blank as u8);

    let undone = patch.undo(&erased).unwrap();
    assert_eq!(
        patch
            .loop_record("loop1")
            .and_then(|kept| kept.tape.as_ref()),
        Some(&tape)
    );
    assert_eq!(
        crate::change::describe(&undone),
        format!("put loop1's tape back (1 bar, reel {})", &tape.reel[..8])
    );
    loops.replace("loop1", Some(tape), false);
    loops.wait_for_test();
    bench.beat = 8.0;
    assert_plays(&render(&mut bench, BAR, 0.0), &samples);
    // Undoing the undo erases again.
    patch.undo(&undone).unwrap();
    assert!(patch.loop_record("loop1").is_none());
}

#[test]
fn remove_then_undo_brings_the_loop_and_its_tape_back() {
    let (mut patch, _) = saved(Some(record()));
    let removed = patch.remove("loop1", &[]).unwrap();
    assert!(patch.loop_record("loop1").is_none());
    let file = patch.to_file();
    assert_eq!(file.retired_loops.len(), 1);
    assert_eq!(file.retired_loops[0].module, "loop1");
    assert_eq!(file.retired_loops[0].record.tape, Some(record()));
    // The retired record survives a save and a load.
    let (mut patch, notes) = Patch::load(&file).unwrap();
    assert!(notes.is_empty(), "{notes:?}");
    patch.undo(&removed).unwrap();
    assert_eq!(
        patch
            .loop_record("loop1")
            .and_then(|kept| kept.tape.as_ref()),
        Some(&record())
    );
    assert!(patch.to_file().retired_loops.is_empty());
}

#[test]
fn the_ninth_loop_is_refused_by_name() {
    assert_eq!(MAX_LOOPS, 8);
    let mut patch = Patch::empty(120.0);
    for _ in 0..8 {
        patch.add("loop", None, None).unwrap();
    }
    let refused = patch.add("loop", None, None).unwrap_err();
    assert_eq!(refused.code, ErrorCode::Full);
    assert_eq!(
        refused.message,
        "a wall holds eight loops, like the eight-track; remove one, or erase one and use it again"
    );
    // A removed ninth cannot come back by undo while eight are up.
    let removed = patch.remove("loop8", &[]).unwrap();
    patch.add("loop", None, None).unwrap();
    assert_eq!(patch.undo(&removed).unwrap_err().code, ErrorCode::Full);
}

#[test]
fn from_is_kept_in_the_save_and_checked() {
    let (mut patch, _) = saved(None);
    let set = patch
        .loop_op(
            "loop1",
            &LoopOp::From {
                from: Some("mouth/dry".into()),
            },
        )
        .unwrap();
    assert_eq!(
        crate::change::describe(&set),
        "set loop1 to record from mouth/dry"
    );
    let text = serde_json::to_string(&patch.to_file()).unwrap();
    let (loaded, notes) = Patch::load(&serde_json::from_str(&text).unwrap()).unwrap();
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(
        loaded.loop_record("loop1").unwrap().from.as_deref(),
        Some("mouth/dry")
    );
    for name in ["a b".to_string(), "a".repeat(25), String::new()] {
        assert_eq!(
            patch
                .loop_op("loop1", &LoopOp::From { from: Some(name) })
                .unwrap_err()
                .code,
            ErrorCode::BadName
        );
    }
    assert!(
        patch
            .loop_op(
                "loop1",
                &LoopOp::From {
                    from: Some("a".repeat(24))
                }
            )
            .is_ok()
    );
    let mut patch = loaded;
    let undone = patch.undo(&set).unwrap();
    assert!(patch.loop_record("loop1").is_none());
    assert_eq!(
        crate::change::describe(&undone),
        "set loop1 to record what is patched into it"
    );
    assert_eq!(
        Request::Loop {
            module: "loop1".into(),
            op: LoopOp::Erase,
        }
        .op(),
        "loop"
    );
}

#[test]
fn a_reel_arriving_after_a_recording_began_does_not_overwrite_it() {
    let dir = reels_dir();
    let (tape, _) = put_reel(dir.path());
    let mut loops = Loops::with_reels_dir(dir.path().to_path_buf());
    let mut bench = bench(&mut loops);
    // The reel is asked for; the loop has it only at the next poll, after
    // a bar is recorded (the tick that would hand it over is held).
    loops.put_back("loop1", tape);
    bench.beat = -32.0 / 24_000.0;
    bench.knob("record", 1.0);
    render(&mut bench, 32 + BAR, 0.75);
    bench.knob("record", 0.0);
    render(&mut bench, TAIL + 32, 0.75);
    assert_eq!(state(&loops), LoopState::Holding as u8);
    loops.wait_for_test();
    bench.beat = 8.0;
    let output = render(&mut bench, BAR, 0.0);
    loops.collect();
    // What was recorded plays, past the seam's crossfade: 0.75 on every
    // cell, none of it the saved reel.
    for (frame, sample) in output.iter().enumerate().take(BAR - 2).skip(TAIL) {
        assert!(
            LEVEL.mul_add(-0.75, *sample).abs() < 1e-6,
            "frame {frame}: {sample}"
        );
    }
    assert!(loops.notes().any(|note| note
        == "loop1 was recorded before its saved tape arrived; the saved tape was not put back"));
}

#[test]
fn an_engine_rebuilt_after_a_device_change_puts_the_tapes_back() {
    let dir = reels_dir();
    let (tape, samples) = put_reel(dir.path());
    let mut loops = Loops::with_reels_dir(dir.path().to_path_buf());
    let mut first = bench(&mut loops);
    loops.put_back("loop1", tape.clone());
    loops.wait_for_test();
    render(&mut first, crate::SUB_BLOCK, 0.0);
    // The device changes: every loop is built again, blank, and given its
    // saved tape again.
    loops.replace_wall();
    let mut second = bench(&mut loops);
    loops.put_back("loop1", tape);
    loops.wait_for_test();
    second.beat = 8.0;
    assert_plays(&render(&mut second, BAR, 0.0), &samples);
    assert_eq!(state(&loops), LoopState::Holding as u8);
}

#[test]
fn an_old_save_without_loops_reads_as_before() {
    const AIRPORTS: &str = include_str!("../../../tests/fixtures/airports-v1.json");
    const EVENING: &[u8] = include_bytes!("../../tree/fixtures/evening.json");
    let (mut airports, _) = crate::migrate::read(AIRPORTS.as_bytes()).unwrap();
    crate::migrate::migrate(&mut airports).unwrap();
    let (evening, _) = crate::migrate::read(EVENING).unwrap();
    for file in [airports, evening] {
        let (patch, notes) = Patch::load(&file).unwrap();
        assert!(
            !notes
                .iter()
                .any(|note| note.contains("loop record") || note.contains("tape was unreadable")),
            "{notes:?}"
        );
        assert!(patch.loops().is_empty());
        let written = String::from_utf8(crate::store::encode(&patch).unwrap()).unwrap();
        assert!(!written.contains("\"loops\""));
        assert!(!written.contains("\"retired_loops\""));
        let tree = crate::tree::to_files(&patch.to_file()).unwrap();
        assert!(
            tree.values().all(|text| {
                let text = String::from_utf8_lossy(text);
                !text.contains("[loop") && !text.contains("retired_loops")
            }),
            "a tree of a wall with no loop has no loop table"
        );
    }
}

#[test]
fn a_reel_that_does_not_fit_the_loop_is_named_and_left_blank() {
    let dir = reels_dir();
    let (tape, _) = put_reel(dir.path());
    let mut loops = Loops::with_reels_dir(dir.path().to_path_buf());
    let mut bench = bench(&mut loops);
    // Two beats: a save may say so, but no loop plays less than a bar.
    let short = TapeRecord { beats: 2.0, ..tape };
    loops.put_back("loop1", short);
    loops.wait_for_test();
    render(&mut bench, crate::SUB_BLOCK, 0.0);
    assert_eq!(state(&loops), LoopState::Blank as u8);
    let notes: Vec<&str> = loops.notes().collect();
    assert!(
        notes.iter().any(|note| note.starts_with("loop1's reel ")
            && note.contains("has a splice no loop plays (2 beats")
            && note.ends_with("; it is blank")),
        "{notes:?}"
    );
    assert!(!notes.iter().any(|note| note.contains("recorded before")));
}

#[test]
fn a_store_that_cannot_open_still_erases() {
    let dir = reels_dir();
    let blocked = dir.path().join("a-file");
    std::fs::write(&blocked, b"not a directory").unwrap();
    let mut loops = Loops::with_reels_dir(blocked.join("reels"));
    let mut bench = bench(&mut loops);
    loops.put_back("loop1", record());
    loops.wait_for_test();
    assert!(loops.notes().any(|note| note.contains("cannot open")));
    loops.replace("loop1", None, false);
    loops.wait_for_test();
    render(&mut bench, crate::SUB_BLOCK, 0.0);
    assert_eq!(state(&loops), LoopState::Blank as u8);
    assert!(
        !loops
            .notes()
            .any(|note| note.contains("keeps the tape it had")),
        "{:?}",
        loops.notes().collect::<Vec<_>>()
    );
}

#[test]
fn the_record_a_wall_keeps_is_checked_and_empty_ones_are_not_kept() {
    let (mut patch, _) = saved(None);
    assert!(patch.loops().is_empty());
    patch.loop_op("loop1", &LoopOp::Erase).unwrap();
    assert!(
        patch.loops().is_empty(),
        "an erased blank loop keeps no record"
    );
    assert_eq!(
        patch
            .set_loop(
                "loop1",
                LoopRecord {
                    tape: Some(TapeRecord {
                        beats: f64::INFINITY,
                        ..record()
                    }),
                    from: None,
                },
            )
            .unwrap_err()
            .code,
        ErrorCode::BadRequest
    );
    patch.add("vco", None, None).unwrap();
    assert_eq!(
        patch
            .set_loop("vco1", LoopRecord::default())
            .unwrap_err()
            .code,
        ErrorCode::NotAllowed
    );
    assert!(matches!(
        patch.loop_op("vco1", &LoopOp::Erase).unwrap_err().code,
        ErrorCode::NotAllowed
    ));
    assert!(matches!(
        patch.undo(&What::Erase {
            module: "vco1".into(),
            was: None,
            tape: None,
        }),
        Err(refused) if refused.code == ErrorCode::NotAllowed
    ));
}

use super::pins::{ReelRefs, Retention};
use crate::protocol::{Change, TakeWhy};
use std::time::{Duration, Instant, SystemTime};

fn short_tape(id: kazoo_core::reels::ReelId) -> TapeRecord {
    TapeRecord {
        reel: id.to_string(),
        frames: 1,
        tail: 0,
        ..record()
    }
}

fn take_change(seq: u64, module: &str, tape: TapeRecord, why: TakeWhy) -> Change {
    Change {
        seq,
        at: crate::change::utc(seq),
        seat: "kazoo-wall".into(),
        what: What::Take {
            module: module.into(),
            tape: Some(tape),
            was: None,
            frame: seq * 96_000,
            beat: seq as f64 * 4.0,
            written: vec![0],
            why,
        },
        summary: "take".into(),
        undoes: None,
        clock: None,
    }
}

#[test]
fn a_long_recording_is_checkpointed_once_a_minute() {
    let dir = reels_dir();
    assert_eq!(CHECKPOINT_SECONDS, 60);
    let mut loops = Loops::with_limits(dir.path().to_path_buf(), 4, u64::MAX);
    let mut bench = bench(&mut loops);
    bench.knob("record", 1.0);
    render(&mut bench, BAR, 0.25);
    let start = Instant::now();
    loops.checkpoint(start);
    let mut checkpoints = Vec::new();
    for pass in 1_u64..=6 {
        render(&mut bench, BAR, pass as f32 / 16.0);
        let now = start + Duration::from_secs(2 * pass);
        loops.checkpoint(now);
        let pending = loops.pending_for_test("loop1");
        if let Some(ticket) = pending.first() {
            loops.wait_snapshot_for_test(*ticket);
            render(&mut bench, 4_096, 0.25);
            let snapshot = loops.capture_snapshot_for_test("loop1");
            let splice = snapshot.splice.unwrap();
            let expected: Vec<_> = snapshot.tape[..2 * (splice.frames + splice.tail)]
                .iter()
                .map(|sample| sample.to_bits())
                .collect();
            let frame = snapshot.at.frame;
            loops.store_snapshot_for_test("loop1", snapshot);
            loops.wait_take_for_test(*ticket);
            let take = loops.recordings.finished.pop_front().unwrap();
            assert_eq!(take.why, TakeWhy::Checkpoint);
            assert_eq!(take.mark.frame, frame);
            let reel = ReelStore::open(dir.path())
                .unwrap()
                .get(&take.tape.reel.parse().unwrap())
                .unwrap();
            assert_eq!(
                reel.samples
                    .iter()
                    .map(|sample| sample.to_bits())
                    .collect::<Vec<_>>(),
                expected
            );
            checkpoints.push((now, take.mark.pass, take.mark.frame));
        }
    }
    assert_eq!(checkpoints.len(), 3);
    assert!(
        checkpoints
            .windows(2)
            .all(
                |pair| pair[1].0.duration_since(pair[0].0) >= Duration::from_secs(4)
                    && pair[1].1 > pair[0].1
                    && pair[1].2.saturating_sub(pair[0].2) >= 4 * 48_000
            )
    );
    assert_eq!(state(&loops), LoopState::Rolling as u8);
}

#[test]
fn pins_name_every_reel_the_wall_names_and_the_sweep_keeps_them() {
    let dir = reels_dir();
    let state_dir = reels_dir();
    let store = ReelStore::open(dir.path()).unwrap();
    let tapes: Vec<_> = (1_u16..=6)
        .map(|value| {
            let id = store.put(48_000, &[f32::from(value) / 8.0; 2]).unwrap();
            short_tape(id)
        })
        .collect();
    let (patch, _) = saved(Some(tapes[0].clone()));
    let (saved_patch, _) = saved(Some(tapes[1].clone()));
    let mut wall_store = crate::store::Store::open(state_dir.path()).unwrap();
    wall_store
        .write_save("saved tape", &saved_patch, false)
        .unwrap();
    wall_store
        .append_synced(&take_change(1, "loop1", tapes[2].clone(), TakeWhy::Punch))
        .unwrap();
    std::fs::write(
        dir.path().join("pins/session-jam.txt"),
        format!("{}\nend\n", tapes[4].reel),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("pins/wall-history.txt"),
        format!("{}\n", tapes[5].reel),
    )
    .unwrap();
    let refs = ReelRefs::read(&patch.to_file(), Some(&mut wall_store), &[]).unwrap();
    let mut retention = Retention::new(dir.path(), u64::MAX).unwrap();
    retention.pin(&store, refs).unwrap();
    let now = SystemTime::now();
    for tape in &tapes {
        std::fs::File::open(store.path(&tape.reel.parse().unwrap()))
            .unwrap()
            .set_modified(now - Duration::from_secs(2 * 86_400))
            .unwrap();
    }
    retention.sweep(&store, dir.path(), now).unwrap();
    for (index, tape) in tapes.iter().enumerate() {
        assert_eq!(store.contains(&tape.reel.parse().unwrap()), index != 3);
    }
    let pin = std::fs::read_to_string(dir.path().join("pins/wall.txt")).unwrap();
    for index in [0, 1, 2, 5] {
        assert!(pin.contains(&tapes[index].reel));
    }
    std::fs::write(dir.path().join("pins/session-broken.txt"), b"invalid reel").unwrap();
    let orphan = store.put(48_000, &[0.875; 2]).unwrap();
    std::fs::File::open(store.path(&orphan))
        .unwrap()
        .set_modified(now - Duration::from_secs(2 * 86_400))
        .unwrap();
    assert!(
        retention
            .sweep(&store, dir.path(), now)
            .unwrap_err()
            .contains("session-broken")
    );
    assert!(store.contains(&orphan));
}

#[test]
fn checkpoints_are_pruned_for_space_but_takes_and_saves_never_are() {
    let dir = reels_dir();
    let store = ReelStore::open(dir.path()).unwrap();
    let state_dir = reels_dir();
    let mut wall_store = crate::store::Store::open(state_dir.path()).unwrap();
    let bytes = 76;
    let mut loops = Loops::with_limits(dir.path().to_path_buf(), 60, bytes * 3);
    let mut retention = Retention::new(dir.path(), loops.recordings.budget).unwrap();
    let mut changes = Vec::new();
    let mut checkpoints = Vec::new();
    for seq in 1_u16..=6 {
        let id = store.put(48_000, &[f32::from(seq) / 16.0; 2]).unwrap();
        let tape = short_tape(id);
        changes.push(take_change(
            u64::from(seq),
            "loop1",
            tape,
            TakeWhy::Checkpoint,
        ));
        checkpoints.push(id);
    }
    let punch_id = store.put(48_000, &[0.75; 2]).unwrap();
    let save_id = store.put(48_000, &[0.875; 2]).unwrap();
    changes.push(take_change(
        7,
        "loop2",
        short_tape(punch_id),
        TakeWhy::Punch,
    ));
    changes.push(take_change(8, "loop3", short_tape(save_id), TakeWhy::Save));
    let (saved_patch, _) = saved(Some(short_tape(save_id)));
    wall_store
        .write_save("kept music", &saved_patch, false)
        .unwrap();
    let refs = ReelRefs::read(
        &Patch::empty(120.0).to_file(),
        Some(&mut wall_store),
        &changes,
    )
    .unwrap();
    retention.pin(&store, refs.clone()).unwrap();
    retention
        .budget(&store, dir.path(), SystemTime::now())
        .unwrap();
    assert!(checkpoints[..5].iter().all(|id| !store.contains(id)));
    assert!(store.contains(&checkpoints[5]));
    assert!(store.contains(&punch_id));
    assert!(store.contains(&save_id));
    assert_eq!(retention.pruned.len(), 5);
    assert!(!retention.paused);
    let next_punch = store.put(48_000, &[0.9375; 2]).unwrap();
    retention
        .stored(&store, dir.path(), "loop2", next_punch, TakeWhy::Punch)
        .unwrap();
    assert!(store.contains(&next_punch));
    retention
        .budget(&store, dir.path(), SystemTime::now())
        .unwrap();
    assert!(retention.paused);
    let restored = Retention::new(dir.path(), bytes * 3).unwrap();
    assert_eq!(restored.pruned, retention.pruned);
    let mut bench = bench(&mut loops);
    loops.queue_pin(refs, false);
    loops.poll();
    loops.wait_for_test();
    assert!(loops.checkpoints_paused);
    bench.knob("record", 1.0);
    render(&mut bench, BAR + BAR / 2, 0.375);
    bench.knob("record", 0.0);
    render(&mut bench, BAR, 0.375);
    loops.poll();
    let ticket = loops.pending_for_test("loop1")[0];
    loops.wait_snapshot_for_test(ticket);
    render(&mut bench, 4_096, 0.0);
    loops.wait_take_for_test(ticket);
    let take = loops.recordings.finished.pop_front().unwrap();
    assert_eq!(take.why, TakeWhy::Punch);
    let live_punch = take.tape.reel.parse().unwrap();
    assert!(store.contains(&live_punch));
    store.remove(&punch_id).unwrap();
    store.remove(&save_id).unwrap();
    store.remove(&next_punch).unwrap();
    store.remove(&live_punch).unwrap();
    retention
        .budget(&store, dir.path(), SystemTime::now())
        .unwrap();
    assert!(!retention.paused);
    loops.poll();
    while loops.checkpoints_paused {
        loops.wait_for_test();
    }
    assert!(!loops.checkpoints_paused);
}

#[test]
fn a_take_whose_reel_cannot_be_written_says_so_and_the_loop_plays_on() {
    use std::os::unix::fs::PermissionsExt;
    let dir = reels_dir();
    let mut loops = Loops::with_reels_dir(dir.path().to_path_buf());
    let mut bench = bench(&mut loops);
    loops.queue_pin(ReelRefs::default(), false);
    loops.poll();
    loops.wait_for_test();
    bench.knob("record", 1.0);
    render(&mut bench, BAR + BAR / 2, 0.25);
    bench.knob("record", 0.0);
    render(&mut bench, BAR, 0.25);
    loops.poll();
    let ticket = loops.pending_for_test("loop1")[0];
    loops.wait_snapshot_for_test(ticket);
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
    render(&mut bench, 4_096, 0.0);
    loops.wait_take_for_test(ticket);
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(loops.recordings.finished.is_empty());
    assert!(
        loops
            .notes()
            .any(|note| note.contains("loop1's take could not be kept:")
                && note.contains("it is still playing, and is only in memory"))
    );
    assert_eq!(state(&loops), LoopState::Holding as u8);
    assert!(
        render(&mut bench, BAR, 0.0)
            .iter()
            .any(|sample| sample.abs() > 0.1)
    );
}
