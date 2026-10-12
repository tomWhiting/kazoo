#[test]
fn chain_commands_save_the_cursor_group_add_and_list() {
    let mut app = connected();
    let groups = std::collections::BTreeMap::from([(
        "beat".to_string(),
        vec!["vco1".to_string(), "vcf1".to_string()],
    )]);
    app.on_reply(
        super::super::worker::Reply::Event(crate::protocol::Event::Groups { groups }),
        std::time::Instant::now(),
    );
    open(&mut app);
    type_text(&mut app, "group beat");
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Char(':'));
    type_text(&mut app, "chain save beatchain");
    press(&mut app, KeyCode::Enter);
    let requests = calls(&mut app);
    assert_eq!(
        requests,
        vec![Request::ChainSave {
            name: "beatchain".to_string(),
            modules: vec!["vco1".to_string(), "vcf1".to_string()]
        }]
    );
    press(&mut app, KeyCode::Char(':'));
    type_text(&mut app, "chain add beatchain");
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        calls(&mut app),
        vec![Request::ChainAdd {
            name: "beatchain".to_string()
        }]
    );
    press(&mut app, KeyCode::Char(':'));
    type_text(&mut app, "chains");
    press(&mut app, KeyCode::Enter);
    assert_eq!(calls(&mut app), vec![Request::Chains]);
}
