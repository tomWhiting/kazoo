use super::*;

#[test]
fn a_walk_is_orphaned_once_its_parent_changes() {
    assert!(!orphaned(4_242, 4_242));
    assert!(orphaned(4_242, 1));
    assert!(!orphaned(
        std::os::unix::process::parent_id(),
        std::os::unix::process::parent_id()
    ));
}
