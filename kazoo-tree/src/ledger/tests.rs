use super::*;

fn key(limb: &str, index: usize) -> StepKey {
    StepKey {
        limb: limb.to_owned(),
        index,
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
    assert_eq!(ledger.problem, None);
    let mut again = Ledger::of_tree(dir.path());
    again.refresh();
    assert_eq!(again.limbs, ["zebra", "apple"]);
    assert_eq!(again.steps, [key("aa", 0), key("bb", 0), key("aa", 1)]);
    assert_eq!(again.unreadable, 0);
}

#[test]
fn two_views_on_one_tree_each_read_what_the_other_wrote() {
    let dir = tempfile::tempdir().expect("a folder");
    let mut one = Ledger::of_tree(dir.path());
    let mut two = Ledger::of_tree(dir.path());
    one.record(&[], &[key("aa", 0)]);
    two.record(&[], &[key("bb", 0)]);
    one.refresh();
    assert_eq!(one.steps, [key("aa", 0), key("bb", 0)]);
    assert_eq!(
        two.steps,
        [key("bb", 0), key("aa", 0)],
        "its own first, then the other's"
    );
}

#[test]
fn a_line_cut_off_waits_and_a_bad_line_is_counted() {
    let dir = tempfile::tempdir().expect("a folder");
    let path = dir
        .path()
        .join(format!("kazoo-tree-grown-v{GROWTH_VERSION}"));
    std::fs::write(&path, "step\taa\t0\nnonsense\nstep\tbb\tx\nstep\tcc\t").expect("written");
    let mut ledger = Ledger::of_tree(dir.path());
    ledger.refresh();
    assert_eq!(ledger.steps, [key("aa", 0)]);
    assert_eq!(ledger.unreadable, 2);
    append(&path, b"0\n").expect("finished");
    ledger.refresh();
    assert_eq!(ledger.steps, [key("aa", 0), key("cc", 0)]);
}

#[test]
fn a_ledger_that_cannot_be_written_says_so_and_keeps_its_order() {
    let dir = tempfile::tempdir().expect("a folder");
    let mut ledger = Ledger::of_tree(&dir.path().join("no such folder"));
    ledger.record(&[], &[key("aa", 0)]);
    assert!(
        ledger
            .problem
            .as_deref()
            .is_some_and(|p| p.contains("could not be written")),
        "{:?}",
        ledger.problem
    );
    assert_eq!(ledger.steps, [key("aa", 0)]);
}
