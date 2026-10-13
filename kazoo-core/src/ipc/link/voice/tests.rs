use super::*;
use crate::ipc::link::{LinkConfig, start_link};

#[test]
fn an_incomplete_batch_counts_as_dropped() {
    let (link, audio) = start_link(
        LinkConfig::new("voice-counter", 2, 48_000, 256),
        None,
        false,
    )
    .unwrap();
    let (sender, receiver, mut worker) = rings();
    worker.pending = Some(PendingBatch {
        end: 100,
        count: 32,
    });
    worker.settle(99, &link.state);
    assert_eq!(link.voice_stats().sent, 0);
    worker.discard(None, &link.state);
    assert_eq!(link.voice_stats().dropped, 32);
    worker.discard(None, &link.state);
    assert_eq!(link.voice_stats().dropped, 32);
    drop(sender);
    drop(receiver);
    drop(audio);
}

#[test]
fn a_completed_batch_is_counted_once_across_byte_counter_wrap() {
    let (link, audio) = start_link(
        LinkConfig::new("voice-counter", 2, 48_000, 256),
        None,
        false,
    )
    .unwrap();
    let (sender, receiver, mut worker) = rings();
    worker.pending = Some(PendingBatch {
        end: u64::MAX - 2,
        count: 32,
    });
    worker.settle(u64::MAX - 3, &link.state);
    assert_eq!(link.voice_stats().sent, 0);
    worker.settle(3, &link.state);
    assert_eq!(link.voice_stats().sent, 32);
    worker.settle(4, &link.state);
    worker.discard(None, &link.state);
    assert_eq!(link.voice_stats().sent, 32);
    assert_eq!(link.voice_stats().dropped, 0);
    drop(sender);
    drop(receiver);
    drop(audio);
}
