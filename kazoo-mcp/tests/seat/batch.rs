use super::*;

#[tokio::test]
async fn batch_tool_lists_label_id_and_notifies_other_seat_once() {
    let (runtime, state) = dirs();
    let daemon = start(runtime.path(), state.path());
    let socket = daemon.socket().to_path_buf();
    let (client, mut notifications) = seat(&socket, "Listener", 1, None).await;
    works(&client, "wall_look", serde_json::json!({})).await;
    let (maker, maker_notifications) = seat(&socket, "Maker", 1, None).await;
    works(&maker, "wall_look", serde_json::json!({})).await;
    let result = works(&maker, "wall_batch", serde_json::json!({"note":"A moving voice","description":"# Why this voice", "attachments":[{"name":"proof.md","text":"QED"}],"changes":[{"op":"add","kind":"lfo","as":"osc"},{"op":"turn","module":"@osc","knob":"rate","value":3.0,"glide_beats":4.0}]})).await;
    let seq: u64 = result
        .strip_prefix("Change #")
        .unwrap()
        .split(':')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let notes = works(&maker, "wall_notes", serde_json::json!({"seq":seq})).await;
    assert!(notes.contains("# Why this voice"), "{notes}");
    assert!(notes.contains("## proof.md\nQED"), "{notes}");
    assert!(result.contains("osc"), "{result}");
    assert!(result.contains("lfo3"), "{result}");
    // Steps are numbered as a refusal numbers them: from 0, the index in
    // the changes the seat sent.
    assert!(result.contains("Step 0: "), "{result}");
    assert!(result.contains("Step 1: "), "{result}");
    assert!(!result.contains("Step 2"), "{result}");
    loop {
        let told = notifications.recv().await.unwrap();
        let (content, _) = channel_shape(&told);
        if content.contains("A moving voice") {
            assert_eq!(
                content
                    .lines()
                    .filter(|line| line.contains("A moving voice"))
                    .count(),
                1
            );
            assert!(content.contains("2 changes"), "{content}");
            assert!(
                content.contains("with notes: description.md, proof.md"),
                "{content}"
            );
            break;
        }
    }
    maker.cancel().await.unwrap();
    drop(maker_notifications);
    client.cancel().await.unwrap();
    stop(daemon);
}
