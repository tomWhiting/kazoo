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
    let stuck = stuck_shutdowns();
    let before = undelivered_completions();
    let (sender, receiver) = mpsc::sync_channel(1);
    drop(sender);
    drop(std::mem::replace(&mut link.finished, receiver));
    link.running.store(false, Ordering::Release);
    let worker = link.join.take().unwrap();
    worker.thread().unpark();
    worker.join().unwrap();
    drop(audio);
    drop(link);
    assert_eq!(undelivered_completions(), before + 1);
    assert_eq!(stuck_shutdowns(), stuck);
}

#[test]
fn a_wedged_worker_that_finishes_after_the_drop_is_counted_once() {
    let directory = tempfile::tempdir().unwrap();
    let before = stuck_shutdowns();
    let late = undelivered_completions();
    for count in 1..=2 {
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
        drop(link);
        assert_eq!(stuck_shutdowns() - before, count);
        assert_eq!(undelivered_completions() - late, count - 1);
        let plural = if count == 1 { "" } else { "s" };
        let expected = format!("kazoo: {count} desk link{plural} did not stop within 500 ms");
        let pending = crate::host::link::exit_report_from(count, count - 1).unwrap();
        assert!(pending.starts_with(&expected));
        assert_eq!(pending.contains("finished later"), count > 1);
        release.send(()).unwrap();
        completed.recv().unwrap();
        drop(audio);
        assert_eq!(stuck_shutdowns() - before, count);
        assert_eq!(undelivered_completions() - late, count);
        assert_eq!(
            crate::host::link::exit_report_from(
                stuck_shutdowns() - before,
                undelivered_completions() - late,
            ),
            Some(format!("{expected}; {count} finished later")),
        );
        assert_eq!(
            crate::host::link::exit_report(),
            crate::host::link::exit_report_from(stuck_shutdowns(), undelivered_completions()),
        );
    }
}
