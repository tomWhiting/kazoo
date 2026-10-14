use super::*;

fn key(limb: &str, index: usize) -> Saved {
    Saved {
        key: StepKey {
            limb: limb.to_owned(),
            index,
        },
        first: None,
        shape: None,
    }
}

fn shaped(limb: &str, index: usize, shape: Shape) -> Saved {
    Saved {
        shape: Some(shape),
        ..key(limb, index)
    }
}

#[test]
fn what_is_written_is_read_back_in_order_once() {
    let dir = tempfile::tempdir().expect("a folder");
    let mut ledger = Ledger::of_tree(dir.path());
    ledger.record(&["zebra".to_owned()], &[key("aa", 0), key("bb", 0)]);
    ledger.record(
        &["zebra".to_owned(), "apple".to_owned()],
        &[key("aa", 1), key("aa", 0)],
    );
    assert_eq!(ledger.problem(), None);
    let mut again = Ledger::of_tree(dir.path());
    again.refresh();
    assert_eq!(again.limbs, ["zebra", "apple"]);
    assert_eq!(again.steps, [key("aa", 0), key("bb", 0), key("aa", 1)]);
    assert_eq!(again.unreadable, 0);
}

#[test]
fn a_drawn_step_comes_back_exactly_where_it_was_drawn() {
    let dir = tempfile::tempdir().expect("a folder");
    let shape = Shape {
        start: (0.1 + 0.2, -1.0 / 3.0),
        end: (f64::MIN_POSITIVE, 1e300),
        heading: std::f64::consts::FRAC_PI_2 - 1e-12,
    };
    Ledger::of_tree(dir.path()).record(&[], &[shaped("aa", 0, shape)]);
    let mut again = Ledger::of_tree(dir.path());
    again.refresh();
    assert_eq!(again.steps, [shaped("aa", 0, shape)]);
}

#[test]
fn two_views_on_one_tree_hold_the_files_order() {
    let dir = tempfile::tempdir().expect("a folder");
    let mut one = Ledger::of_tree(dir.path());
    let mut two = Ledger::of_tree(dir.path());
    one.record(&[], &[key("aa", 0)]);
    two.record(&[], &[key("bb", 0)]);
    one.refresh();
    assert_eq!(one.steps, [key("aa", 0), key("bb", 0)]);
    assert_eq!(two.steps, [key("aa", 0), key("bb", 0)]);
}

#[test]
fn a_line_cut_off_waits_and_never_swallows_the_next() {
    let dir = tempfile::tempdir().expect("a folder");
    let path = dir.path().join(LEDGER);
    std::fs::write(&path, "step\taa\t0\nnonsense\nstep\tbb\tx\nstep\tcc\t").expect("written");
    let mut ledger = Ledger::of_tree(dir.path());
    ledger.refresh();
    assert_eq!(ledger.steps, [key("aa", 0)]);
    assert_eq!(ledger.unreadable, 2);
    // The cut-off line is ended before the next is written, so the next
    // step is kept whole; the cut-off one never reads.
    ledger.record(&[], &[key("dd", 0)]);
    let mut again = Ledger::of_tree(dir.path());
    again.refresh();
    assert_eq!(again.steps, [key("aa", 0), key("dd", 0)]);
}

#[test]
fn a_replaced_ledger_is_written_again_from_what_is_held() {
    let dir = tempfile::tempdir().expect("a folder");
    let mut ledger = Ledger::of_tree(dir.path());
    ledger.record(&["zebra".to_owned()], &[key("aa", 0), key("bb", 0)]);
    std::fs::write(dir.path().join(LEDGER), "").expect("emptied");
    ledger.refresh();
    let mut again = Ledger::of_tree(dir.path());
    again.refresh();
    assert_eq!(again.limbs, ["zebra"]);
    assert_eq!(again.steps, [key("aa", 0), key("bb", 0)]);
}

#[test]
fn a_ledger_that_cannot_be_written_says_so_and_keeps_its_order() {
    let dir = tempfile::tempdir().expect("a folder");
    let mut ledger = Ledger::of_tree(&dir.path().join("no such folder"));
    ledger.record(&[], &[key("aa", 0)]);
    assert!(
        ledger
            .problem()
            .is_some_and(|p| p.contains("could not be written")),
        "{:?}",
        ledger.problem()
    );
    assert_eq!(ledger.steps, [key("aa", 0)]);
}

#[test]
fn a_view_holds_the_ledger_until_it_lets_go() {
    let dir = tempfile::tempdir().expect("a folder");
    let one = Ledger::of_tree(dir.path());
    let two = Ledger::of_tree(dir.path());
    let held = one.hold().expect("held").expect("a file");
    let lock = dir.path().join(LEDGER).with_extension("lock");
    let other = File::open(&lock).expect("the lock file");
    assert_eq!(
        rustix::fs::flock(&other, rustix::fs::FlockOperation::NonBlockingLockExclusive),
        Err(rustix::io::Errno::WOULDBLOCK)
    );
    drop(held);
    assert!(two.hold().expect("held after").is_some());
}

#[test]
fn a_later_line_fills_in_what_an_earlier_one_lacked_and_nothing_else() {
    let dir = tempfile::tempdir().expect("a folder");
    let shape = Shape {
        start: (1.0, 2.0),
        end: (3.0, 4.0),
        heading: 0.5,
    };
    let other = Shape {
        start: (9.0, 9.0),
        ..shape
    };
    let mut ledger = Ledger::of_tree(dir.path());
    // A line from before the first moment and shape were kept.
    ledger.record(&[], &[key("aa", 0)]);
    let full = Saved {
        first: Some("c0ffee".to_owned()),
        ..shaped("aa", 0, shape)
    };
    ledger.record(&[], std::slice::from_ref(&full));
    let length = std::fs::metadata(dir.path().join(LEDGER))
        .expect("written")
        .len();
    // Again, and with another shape: nothing more is written.
    ledger.record(&[], std::slice::from_ref(&full));
    ledger.record(
        &[],
        &[Saved {
            first: Some("c0ffee".to_owned()),
            ..shaped("aa", 0, other)
        }],
    );
    assert_eq!(
        std::fs::metadata(dir.path().join(LEDGER))
            .expect("written")
            .len(),
        length
    );
    let mut again = Ledger::of_tree(dir.path());
    again.refresh();
    assert_eq!(again.steps, [full]);
    assert_eq!(again.unreadable, 0);
    // A first= with no commit does not read.
    let mut broken = Ledger::in_memory();
    broken.take("step\taa\t1\tfirst=\t1\t2\t3\t4\t5");
    assert_eq!(broken.unreadable, 1);
}

#[test]
fn a_step_drawn_again_for_another_first_moment_takes_its_place() {
    let dir = tempfile::tempdir().expect("a folder");
    let shape = |x: f64| Shape {
        start: (x, 0.0),
        end: (x, 1.0),
        heading: 0.5,
    };
    let step = |first: &str, x: f64| Saved {
        first: Some(first.to_owned()),
        ..shaped("aa", 1, shape(x))
    };
    let mut ledger = Ledger::of_tree(dir.path());
    ledger.record(&[], &[step("c0ffee", 1.0), key("bb", 0)]);
    ledger.record(&[], &[step("beef", 2.0)]);
    let mut again = Ledger::of_tree(dir.path());
    again.refresh();
    assert_eq!(again.steps, [step("beef", 2.0), key("bb", 0)]);
    // first= comes last on the line, after the shape.
    let text = std::fs::read_to_string(dir.path().join(LEDGER)).expect("written");
    assert!(
        text.lines()
            .next()
            .is_some_and(|line| line.ends_with("\t0.5\tfirst=c0ffee")),
        "{text}"
    );
}

#[test]
fn a_line_with_its_first_moment_before_its_shape_still_reads() {
    // The order a build wrote for a while (92e4798): first= straight after
    // the step's number.
    let mut ledger = Ledger::in_memory();
    ledger.take("step\taa\t1\tfirst=c0ffee\t1\t2\t3\t4\t0.5");
    assert_eq!(ledger.unreadable, 0);
    assert_eq!(
        ledger.steps,
        [Saved {
            first: Some("c0ffee".to_owned()),
            ..shaped(
                "aa",
                1,
                Shape {
                    start: (1.0, 2.0),
                    end: (3.0, 4.0),
                    heading: 0.5,
                }
            )
        }]
    );
}
