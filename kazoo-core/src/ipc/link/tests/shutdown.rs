use super::*;

#[test]
fn wedged_worker_shutdown_is_counted_after_link_drop() {
    let directory = tempfile::tempdir().unwrap();
    let (mut link, audio) =
        start_link(config(&directory.path().join("missing.sock")), None, false).unwrap();
    let (release, blocked) = mpsc::channel();
    let (done, completed) = mpsc::channel();
    let (finished_tx, finished) = mpsc::sync_channel(1);
    link.finished = finished;
    link.join = Some(thread::spawn(move || {
        blocked.recv().unwrap();
        finished_tx.send(()).unwrap();
        done.send(()).unwrap();
    }));
    let before = stuck_shutdowns();
    link.shutdown();
    let after = stuck_shutdowns();
    release.send(()).unwrap();
    completed.recv().unwrap();
    drop(audio);
    drop(link);
    assert_eq!(after, before + 1);
    assert_eq!(stuck_shutdowns(), after);
}

#[test]
fn undelivered_worker_completion_is_counted() {
    let directory = tempfile::tempdir().unwrap();
    let (mut link, audio) = hub_link(config(&directory.path().join("missing.sock"))).unwrap();
    let before = stuck_shutdowns();
    let (sender, receiver) = mpsc::sync_channel(1);
    drop(sender);
    drop(std::mem::replace(&mut link.finished, receiver));
    link.running.store(false, Ordering::Release);
    let worker = link.join.take().unwrap();
    worker.thread().unpark();
    worker.join().unwrap();
    drop(audio);
    drop(link);
    assert_eq!(stuck_shutdowns(), before + 1);
}

#[test]
fn a_wedged_worker_that_finishes_after_the_drop_is_counted_once() {
    let directory = tempfile::tempdir().unwrap();
    let (mut link, audio) =
        start_link(config(&directory.path().join("missing.sock")), None, false).unwrap();
    let (release, blocked) = mpsc::channel();
    let (done, completed) = mpsc::channel();
    let (finished_tx, finished) = mpsc::sync_channel(1);
    link.finished = finished;
    link.join = Some(thread::spawn(move || {
        blocked.recv().unwrap();
        crate::ipc::link::shutdown::report_finished(&finished_tx);
        done.send(()).unwrap();
    }));
    let before = stuck_shutdowns();
    drop(link);
    release.send(()).unwrap();
    completed.recv().unwrap();
    drop(audio);
    assert_eq!(stuck_shutdowns(), before + 1);
}
