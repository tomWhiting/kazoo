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
