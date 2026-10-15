//! KZ-T10c: a batch's changes are each a moment on the tree.

use super::*;

/// The tree's newest `count` commits on its branch, oldest first: each
/// one's id, `Kazoo-Kind` and subject.
fn newest(tree: &Path, count: usize) -> Vec<(String, String, String)> {
    let text = command(
        tree,
        &[
            "log",
            &format!("-n{count}"),
            "--reverse",
            "--format=%H%x1f%(trailers:key=Kazoo-Kind,valueonly)%x1f%s%x1e",
            "HEAD",
        ],
    );
    text.split('\u{1e}')
        .map(str::trim)
        .filter(|row| !row.is_empty())
        .map(|row| {
            let mut fields = row.split('\u{1f}');
            let id = fields.next().unwrap().to_string();
            let kind = fields.next().unwrap().trim().to_string();
            let subject = fields.next().unwrap().to_string();
            (id, kind, subject)
        })
        .collect()
}

fn cutoff_at(tree: &Path, commit: &str) -> String {
    command(tree, &["show", &format!("{commit}:modules/vcf1.toml")])
        .lines()
        .find(|line| line.trim_start().starts_with("cutoff"))
        .unwrap()
        .trim()
        .to_string()
}

#[test]
fn a_batch_grows_a_moment_for_each_change_under_its_note() {
    let mut rig = Rig::new(None);
    rig.prepare();
    let turn = |value: f64| serde_json::json!({"op":"turn", "module":"vcf1", "knob":"cutoff", "value":value, "glide_beats":0});
    let made = rig
        .ask(
            "Cally",
            false,
            serde_json::json!({"op":"batch", "note":"Three cutoffs", "changes":[turn(300.0), turn(400.0), turn(500.0)]}),
        )
        .unwrap();
    let seq = made.change.unwrap().seq;
    rig.wall.close(rig.now);
    let moments = newest(rig.tree.path(), 3);
    let kinds: Vec<&str> = moments.iter().map(|(_, kind, _)| kind.as_str()).collect();
    assert_eq!(kinds, ["part", "part", "batch"], "{moments:?}");
    assert!(
        moments[0].2.starts_with("Cally: Three cutoffs (1/3) "),
        "{}",
        moments[0].2
    );
    assert!(
        moments[1].2.starts_with("Cally: Three cutoffs (2/3) "),
        "{}",
        moments[1].2
    );
    assert_eq!(moments[2].2, "Cally: Three cutoffs");
    let cutoffs: Vec<String> = moments
        .iter()
        .map(|(id, _, _)| cutoff_at(rig.tree.path(), id))
        .collect();
    assert_eq!(
        cutoffs,
        ["cutoff = 300.0", "cutoff = 400.0", "cutoff = 500.0"]
    );
    // The batch stays one change: one seq in the log, and the tree's
    // change index names only the batch's own commit by it.
    assert_eq!(rig.wall.log.iter().filter(|c| c.seq == seq).count(), 1);
    let part = command(
        rig.tree.path(),
        &[
            "log",
            "-1",
            "--format=%(trailers:key=Kazoo-Seq,valueonly)%x1f%(trailers:key=Kazoo-Batch,valueonly)%x1f%(trailers:key=Kazoo-Part,valueonly)",
            &moments[0].0,
        ],
    );
    assert_eq!(
        part.split('\u{1f}').map(str::trim).collect::<Vec<_>>(),
        ["", &seq.to_string(), "1/3"]
    );
}

#[test]
fn an_import_of_the_log_grows_the_same_moments_as_the_live_tree() {
    let mut rig = Rig::new(None);
    let initial = rig.wall.patch.to_file();
    rig.prepare();
    let turn = |value: f64| serde_json::json!({"op":"turn", "module":"vcf1", "knob":"cutoff", "value":value, "glide_beats":0});
    rig.ask(
        "Cally",
        false,
        serde_json::json!({"op":"batch", "note":"An lfo on the filter", "changes":[
            {"op":"add", "kind":"lfo", "as":"osc"},
            {"op":"patch", "from":"@osc.out", "to":"vcf1.cutoff"},
            turn(700.0)
        ]}),
    )
    .unwrap();
    rig.wall.close(rig.now);
    let live: Vec<String> = newest(rig.tree.path(), 3)
        .iter()
        .map(|(id, _, _)| command(rig.tree.path(), &["rev-parse", &format!("{id}^{{tree}}")]))
        .collect();
    let state = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(state.path().join("saves")).unwrap();
    std::fs::write(
        state.path().join("patch.before-1.json"),
        serde_json::to_vec(&initial).unwrap(),
    )
    .unwrap();
    std::fs::write(
        state.path().join("patch.json"),
        serde_json::to_vec(&rig.wall.patch.to_file()).unwrap(),
    )
    .unwrap();
    std::fs::copy(
        rig.state.path().join("log.jsonl"),
        state.path().join("log.jsonl"),
    )
    .unwrap();
    let tree = state.path().join("tree");
    let grown = crate::tree::rings::grow(state.path(), &tree, false).unwrap();
    assert_eq!(grown.gaps, 0, "{}", grown.report);
    let imported = newest(&tree, 3);
    let kinds: Vec<&str> = imported.iter().map(|(_, kind, _)| kind.as_str()).collect();
    assert_eq!(kinds, ["part", "part", "batch"], "{imported:?}");
    let imported: Vec<String> = imported
        .iter()
        .map(|(id, _, _)| command(&tree, &["rev-parse", &format!("{id}^{{tree}}")]))
        .collect();
    assert_eq!(
        live, imported,
        "each moment holds the same wall, live or imported"
    );
}
