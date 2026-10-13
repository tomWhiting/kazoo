use super::{EngineHandle, ThreadHandles};
use crate::analysis::PitchDetectorConfig;
use crate::engine::{DiskCommand, EngineCommand};
use crate::voice::judge::{self, JudgeResult, OnsetConfig};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[test]
fn the_judge_thread_is_joined_at_shutdown() {
    let (mut feed, mut results, judge) = judge::spawn(
        PitchDetectorConfig::for_rate(48_000),
        128,
        OnsetConfig::default(),
    )
    .unwrap();
    let samples: Vec<f32> = (0..4_096)
        .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.5)
        .collect();
    feed.push(&samples);
    let shutdown = Arc::new(AtomicBool::new(false));
    let worker_shutdown = Arc::clone(&shutdown);
    let stream_holder = std::thread::spawn(move || {
        while !worker_shutdown.load(Ordering::Acquire) {
            std::thread::park();
        }
        drop(feed);
    });
    let (commands, command_rx) = crossbeam_channel::bounded(4);
    let (disk_cmd_tx, disk_rx) = crossbeam_channel::bounded(4);
    let disk = std::thread::spawn(move || {
        assert!(matches!(disk_rx.recv().unwrap(), DiskCommand::Shutdown));
    });
    let mut handle = EngineHandle::detached(commands, 48_000, 128);
    handle.set_thread_handles(ThreadHandles {
        analysis: std::thread::spawn(|| {}),
        judge,
        disk,
        reclaim: std::thread::spawn(|| {}),
        stream_holder,
        stream_shutdown: shutdown,
        disk_cmd_tx,
    });
    drop(handle);
    assert!(matches!(
        command_rx.recv().unwrap(),
        EngineCommand::Shutdown
    ));
    // Joining must finish the queued analysis before returning from Drop.
    let mut pitches = 0;
    while let Some(result) = results.pop() {
        if matches!(result, JudgeResult::Pitch(_)) {
            pitches += 1;
        }
    }
    assert!(
        pitches > 0,
        "shutdown returned before the judge drained its input"
    );
}
