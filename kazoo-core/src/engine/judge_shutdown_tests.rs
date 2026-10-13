//! Shutdown joins the judge worker: `EngineHandle`'s drop waits for it.

use super::{EngineHandle, ThreadHandles};
use crate::engine::{DiskCommand, EngineCommand, EngineConfig, judge_config};
use crate::voice::judge::{self, JudgeResult, OnsetConfig};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The judge's last input arrives as the streams stop, so only a join can
/// see it judged before `Drop` returns: a detached worker is still busy
/// with pYIN, or still parked, when the handle is gone.
#[test]
fn the_judge_thread_is_joined_at_shutdown() {
    let config = judge_config(&EngineConfig::default().pitch, 48_000);
    let hop = config.hop_length;
    let (mut feed, mut results, judge) = judge::spawn(config, 128, OnsetConfig::default()).unwrap();
    // Two whole hops: the most the judge's input ring holds at 48 kHz.
    let samples: Vec<f32> = (0..2 * hop)
        .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.5)
        .collect();
    let shutdown = Arc::new(AtomicBool::new(false));
    let worker_shutdown = Arc::clone(&shutdown);
    let stream_holder = std::thread::spawn(move || {
        while !worker_shutdown.load(Ordering::Acquire) {
            std::thread::park();
        }
        // The callback's last block, then the callback is gone.
        feed.push(&samples);
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
    let mut pitches = 0;
    while let Some(result) = results.pop() {
        if matches!(result, JudgeResult::Pitch(_)) {
            pitches += 1;
        }
    }
    assert_eq!(
        pitches, 2,
        "shutdown returned before the judge finished its last input"
    );
}
