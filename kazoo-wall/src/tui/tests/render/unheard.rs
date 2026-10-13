use super::*;

#[test]
fn an_unheard_panel_says_so_in_dim_text() {
    let mut app = connected();
    assert!(!text(&render(&mut app, 160, 50)).contains("unheard"));
    let mut silent = snapshot();
    silent.timing.unheard = vec!["vcf1".to_string()];
    app.on_reply(Reply::Snapshot(Box::new(silent)), Instant::now());
    let buffer = render(&mut app, 160, 50);
    let (x, y) = find(&buffer, "unheard").unwrap();
    assert_eq!(buffer[(x, y)].fg, super::super::super::theme::TEXT_DIM);
    assert!(text(&buffer).contains("vcf"));
}
