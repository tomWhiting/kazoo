use super::*;

#[test]
fn notes_news_keeps_the_names_after_a_long_batch_title() {
    let mut news = News::new("Listener".into());
    let change: kazoo_wall::protocol::Change = serde_json::from_value(serde_json::json!({"seq":1,"at":"2026-10-17T00:00:00Z","seat":"Tom","summary":format!("Tom: \"{}\" (4096 changes, with notes: description.md, proof.md (12 KB): many changes)", "x".repeat(200)),"notes":[{"name":"proof.md","bytes":12288,"sha256":"unused"}],"what":{"op":"batch","note":"Proof","changes":[]}})).unwrap();
    news.take(
        Feed::Event(Event::Change {
            change: Box::new(change),
        }),
        Instant::now(),
    );
    let text = news.params()["content"].as_str().unwrap().to_string();
    assert!(text.contains("proof.md (12 KB)"), "{text}");
}
