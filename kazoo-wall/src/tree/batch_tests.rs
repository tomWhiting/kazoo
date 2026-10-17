use super::*;

#[test]
fn batch_commit_preserves_note_and_every_ordered_step() {
    let change: Change = serde_json::from_value(serde_json::json!({"seq":7,"at":"2026-10-14T10:00:00Z","seat":"Tom","summary":"unused","what":{"op":"batch","note":"A voice","changes":[{"op":"tempo","from":120.0,"to":130.0,"desk":false},{"op":"tempo","from":130.0,"to":140.0,"desk":false}]}})).unwrap();
    let commit = Commit::change(&change, Vec::new()).unwrap();
    let steps_json = serde_json::to_value(&change.what).unwrap();
    let descriptions = steps_json["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| crate::change::describe(&serde_json::from_value(value.clone()).unwrap()))
        .collect::<Vec<_>>();
    assert!(
        commit
            .message
            .starts_with("Tom: A voice\n\nKazoo-Kind: batch\nKazoo-Seq: 7\n")
    );
    assert!(commit.message.contains("Kazoo-Note: A voice\n"));
    let steps = commit
        .message
        .lines()
        .filter_map(|line| line.strip_prefix("Kazoo-Step: "))
        .collect::<Vec<_>>();
    assert_eq!(steps, descriptions);
    let mut single = change;
    single.what = serde_json::from_value(
        serde_json::json!({"op":"tempo","from":120.0,"to":130.0,"desk":false}),
    )
    .unwrap();
    let json = serde_json::to_string(&single).unwrap();
    assert_eq!(
        Commit::change(&single, Vec::new()).unwrap().message,
        format!("unused\n\nKazoo-Kind: change\nKazoo-Seq: 7\nKazoo-Change: {json}\n")
    );
}

#[test]
fn an_undone_batch_is_committed_as_its_undo_not_as_the_batch_again() {
    let change: Change = serde_json::from_value(serde_json::json!({"seq":8,"at":"2026-10-14T10:00:00Z","seat":"Tom","undoes":7,"summary":"Tom undid change 7: \"A voice\" (1 change: tempo 130 BPM → 120 BPM)","what":{"op":"batch","note":"A voice","changes":[{"op":"tempo","from":130.0,"to":120.0,"desk":false}]}})).unwrap();
    let json = serde_json::to_string(&change).unwrap();
    assert_eq!(
        Commit::change(&change, Vec::new()).unwrap().message,
        format!(
            "Tom undid change 7: \"A voice\" (1 change: tempo 130 BPM → 120 BPM)\n\nKazoo-Kind: change\nKazoo-Seq: 8\nKazoo-Change: {json}\n"
        )
    );
}

#[test]
fn notes_commit_has_body_trailers_and_no_text_in_change_json() {
    let change: Change = serde_json::from_value(serde_json::json!({"seq":7,"at":"2026-10-14T10:00:00Z","seat":"Tom","summary":"proof","notes":[{"name":"description.md","bytes":5,"sha256":"unused"},{"name":"proof.md","bytes":3,"sha256":"unused"}],"what":{"op":"batch","note":"A voice","changes":[{"op":"tempo","from":120.0,"to":130.0,"desk":false},{"op":"tempo","from":130.0,"to":140.0,"desk":false}]}})).unwrap();
    let files = vec![(
        "notes/7/description.md".into(),
        Some("é".repeat(2001).into_bytes()),
    )];
    let commit = Commit::change(&change, files).unwrap();
    assert!(commit.message.starts_with(&format!(
        "Tom: A voice\n\n{}…\n\nKazoo-Kind:",
        "é".repeat(2000)
    )));
    assert!(
        commit
            .message
            .contains("Kazoo-Attachment: description.md (5)\nKazoo-Attachment: proof.md (3)\n")
    );
    let part = Commit::part(&change, 0, Vec::new()).unwrap();
    assert!(!part.message.contains("Kazoo-Attachment"));
    assert!(!part.message.contains('é'));
    assert!(part.files.is_empty());
}
