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
fn notes_a_healed_batch_keeps_its_notes_in_the_store_not_on_the_start() {
    // A batch the tree never held is healed into the start, a whole moment
    // with no commit of its own: its notes go on no commit, and read back
    // from the store as ever.
    let state = tempfile::tempdir().unwrap();
    let live = tempfile::tempdir().unwrap();
    git::plant(live.path()).unwrap();
    let (_engine, mut wall) = real_wall(state.path());
    let request: Request = serde_json::from_value(serde_json::json!({"op":"batch","note":"Proof","attachments":[{"name":"proof.md","text":"QED"}],"changes":[{"op":"tempo","bpm":140.0}]})).unwrap();
    let made = ask(&mut wall, &request).change.unwrap();
    wall.attach_tree(TreeHandle::attach(live.path()).unwrap());
    ask(&mut wall, &Request::Tree { recent: None });
    let read: Request =
        serde_json::from_value(serde_json::json!({"op":"notes","seq":made.seq})).unwrap();
    let answer = wall.request("Tom", &read, Instant::now()).unwrap().result;
    assert_eq!(answer["attachments"][0]["text"], "QED");
    wall.close(Instant::now());
    let files = raw_files(live.path(), "HEAD");
    assert!(files.contains("wall.toml"), "{files}");
    assert!(!files.contains("notes/"), "{files}");
}

/// Every commit of `tree`'s trunk, oldest first, as git keeps it, with its
/// parent lines left out.
fn commits_without_parents(tree: &Path) -> Vec<String> {
    command(tree, &["rev-list", "--reverse", "HEAD"])
        .lines()
        .map(|commit| {
            command(tree, &["cat-file", "commit", commit])
                .lines()
                .filter(|line| !line.starts_with("parent "))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect()
}

#[test]
fn notes_import_differs_from_live_only_in_the_start_commits_words() {
    // The live tree's first commit says the wall started again; an import
    // works the wall back from patch.json and says so. That holds of every
    // tree, with notes or without (it is older than KZ-T13), and those
    // words are the only difference: the start's files are the same, and
    // every later commit, the batch's parts and its own commit with its
    // notes, is the same commit byte for byte but for the parent its start
    // gives it.
    for with_notes in [false, true] {
        let state = tempfile::tempdir().unwrap();
        let live = tempfile::tempdir().unwrap();
        git::plant(live.path()).unwrap();
        let (_engine, mut wall) = real_wall(state.path());
        wall.attach_tree(TreeHandle::attach(live.path()).unwrap());
        let mut value = serde_json::json!({"op":"batch","note":"Proof","changes":[{"op":"turn","module":"vcf1","knob":"cutoff","value":300.0},{"op":"turn","module":"vcf1","knob":"cutoff","value":310.0}]});
        if with_notes {
            value["description"] = serde_json::json!("# Why\nA proof");
            value["attachments"] = serde_json::json!([{"name":"proof.md","text":"QED"}]);
        }
        ask(&mut wall, &serde_json::from_value(value).unwrap());
        ask(&mut wall, &Request::Tree { recent: None });
        wall.close(Instant::now());
        drop(wall);
        let imported = tempfile::tempdir().unwrap();
        grow(state.path(), imported.path(), false).unwrap();
        let live_commits = commits_without_parents(live.path());
        let imported_commits = commits_without_parents(imported.path());
        assert_eq!(live_commits.len(), 3, "start, part 1/2, the batch");
        assert_eq!(imported_commits.len(), live_commits.len());
        let tree_line = |commit: &str| commit.lines().next().unwrap().to_string();
        assert_eq!(tree_line(&live_commits[0]), tree_line(&imported_commits[0]));
        assert!(live_commits[0].contains("the wall started again"));
        assert!(imported_commits[0].contains("worked back from patch.json"));
        assert_eq!(live_commits[1..], imported_commits[1..]);
        assert_eq!(
            live_commits[2].contains("Kazoo-Attachment: proof.md (3)"),
            with_notes
        );
    }
}

#[test]
fn notes_an_import_without_a_store_file_leaves_the_commit_without_it() {
    let state = tempfile::tempdir().unwrap();
    let (_engine, mut wall) = real_wall(state.path());
    let request: Request = serde_json::from_value(serde_json::json!({"op":"batch","note":"Proof","description":"Why","attachments":[{"name":"proof.md","text":"QED"}],"changes":[{"op":"tempo","bpm":140.0}]})).unwrap();
    let made = ask(&mut wall, &request).change.unwrap();
    wall.close(Instant::now());
    drop(wall);
    let proof = made
        .notes
        .iter()
        .find(|note| note.name == "proof.md")
        .unwrap();
    std::fs::remove_file(state.path().join("notes").join(&proof.sha256)).unwrap();
    let imported = tempfile::tempdir().unwrap();
    grow(state.path(), imported.path(), false).unwrap();
    let files = raw_files(imported.path(), "HEAD");
    assert!(
        files.contains(&format!("notes/{}/description.md", made.seq)),
        "{files}"
    );
    assert!(!files.contains("proof.md"), "{files}");
    let body = command(imported.path(), &["show", "-s", "--format=%B", "HEAD"]);
    assert!(body.contains("Kazoo-Attachment: proof.md (3)"), "{body}");
}

#[test]
fn notes_of_a_healed_batch_never_land_on_another_changes_commit() {
    // The tree is detached at a moment that already holds the wall as it
    // is after a batch with notes and its undo, both owed: the start is not
    // committed, and the next change's commit is the first the tree gets.
    // The batch has no commit of its own, so its notes go on none.
    let tree = tempfile::tempdir().unwrap();
    git::plant(tree.path()).unwrap();
    let first = tempfile::tempdir().unwrap();
    let (_first_engine, mut wall) = real_wall(first.path());
    wall.attach_tree(TreeHandle::attach(tree.path()).unwrap());
    ask(&mut wall, &Request::Tree { recent: None });
    wall.close(Instant::now());
    drop(wall);
    let start = command(tree.path(), &["rev-parse", "HEAD"]);
    command(tree.path(), &["update-ref", "--no-deref", "HEAD", &start]);
    let state = tempfile::tempdir().unwrap();
    let (_engine, mut wall) = real_wall(state.path());
    let request: Request = serde_json::from_value(serde_json::json!({"op":"batch","note":"Proof","description":"Why","attachments":[{"name":"proof.md","text":"QED"}],"changes":[{"op":"tempo","bpm":140.0}]})).unwrap();
    let made = ask(&mut wall, &request).change.unwrap();
    ask(&mut wall, &Request::Undo { change: made.seq });
    wall.attach_tree(TreeHandle::attach(tree.path()).unwrap());
    ask(
        &mut wall,
        &super::super::fixtures::turn_request("vcf1", "cutoff", 333.0, None),
    );
    ask(&mut wall, &Request::Tree { recent: None });
    wall.close(Instant::now());
    let head = command(tree.path(), &["rev-parse", "HEAD"]);
    assert_ne!(head, start, "the turn has a commit of its own");
    let files = raw_files(tree.path(), &head);
    assert!(!files.contains("notes/"), "{files}");
}
