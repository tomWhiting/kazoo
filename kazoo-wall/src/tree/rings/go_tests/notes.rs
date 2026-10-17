use super::*;

#[test]
fn notes_live_tree_and_import_preserve_batch_content() {
    let state = tempfile::tempdir().unwrap();
    let live = tempfile::tempdir().unwrap();
    git::plant(live.path()).unwrap();
    let (_engine, mut wall) = real_wall(state.path());
    wall.attach_tree(TreeHandle::attach(live.path()).unwrap());
    let request: Request = serde_json::from_value(serde_json::json!({"op":"batch","note":"Proof","description":"# Why\nA proof", "attachments":[{"name":"proof.md","text":"QED"}],"changes":[{"op":"turn","module":"vcf1","knob":"cutoff","value":300.0},{"op":"turn","module":"vcf1","knob":"cutoff","value":310.0}]})).unwrap();
    let made = ask(&mut wall, &request).change.unwrap();
    ask(&mut wall, &Request::Tree { recent: None });
    wall.close(Instant::now());
    drop(wall);
    let own = command(live.path(), &["rev-parse", "HEAD"]);
    let body = command(live.path(), &["show", "-s", "--format=%B", &own]);
    assert!(
        body.contains("Proof\n\n# Why\nA proof\n\nKazoo-Kind:"),
        "{body}"
    );
    assert!(body.contains("Kazoo-Attachment: proof.md (3)"));
    assert_eq!(
        command(
            live.path(),
            &["show", &format!("{own}:notes/{}/proof.md", made.seq)]
        ),
        "QED"
    );
    let parent = command(live.path(), &["rev-parse", &format!("{own}^")]);
    assert!(!raw_files(live.path(), &parent).contains("notes/"));
    let imported = tempfile::tempdir().unwrap();
    grow(state.path(), imported.path(), false).unwrap();
    let other = command(imported.path(), &["rev-parse", "HEAD"]);
    let content = |tree: &Path, commit: &str| {
        command(
            tree,
            &[
                "show",
                "-s",
                "--format=%T|%an|%ae|%at|%cn|%ce|%ct|%B",
                commit,
            ],
        )
    };
    assert_eq!(content(live.path(), &own), content(imported.path(), &other));
}

#[test]
fn notes_healing_reads_store_before_whole_snapshot() {
    let state = tempfile::tempdir().unwrap();
    let live = tempfile::tempdir().unwrap();
    git::plant(live.path()).unwrap();
    let (_engine, mut wall) = real_wall(state.path());
    let request: Request = serde_json::from_value(serde_json::json!({"op":"batch","note":"Proof","attachments":[{"name":"proof.md","text":"QED"}],"changes":[{"op":"tempo","bpm":140.0}]})).unwrap();
    let made = ask(&mut wall, &request).change.unwrap();
    wall.attach_tree(TreeHandle::attach(live.path()).unwrap());
    ask(&mut wall, &Request::Tree { recent: None });
    wall.close(Instant::now());
    assert_eq!(
        command(
            live.path(),
            &["show", &format!("HEAD:notes/{}/proof.md", made.seq)]
        ),
        "QED"
    );
}
