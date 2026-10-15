use super::*;

#[test]
fn an_unverified_attach_keeps_a_save() {
    let mut rig = Rig::new(None);
    let from = rig.listing().here.unwrap();
    let source = Source::Moment {
        to: from.clone(),
        from,
    };
    let (backup, bytes) = rig.wall.keeping_before(&source).unwrap();
    assert!(bytes.is_some(), "unverified attach used {}", backup.name);
    assert!(backup.name.starts_with("before-go-"));
    rig.wall.close(rig.now);
}

#[test]
fn a_desk_tempo_move_between_goes_is_kept_and_undone_exactly() {
    use kazoo_core::host::link::connect_config;
    use kazoo_core::ipc::link::{HubAddress, LinkConfig};
    use kazoo_core::ipc::protocol::FrameBuffer;
    use kazoo_core::ipc::types::{MSG_REGISTER, MSG_REGISTERED, RegisteredMsg};
    let mut rig = Rig::new(None);
    rig.ask("Tom", true, serde_json::json!({"op":"tempo", "bpm":120.0}))
        .unwrap();
    rig.prepare();
    let path = rig.state.path().join("desk.sock");
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    let linking = std::thread::spawn(move || {
        connect_config(LinkConfig {
            address: HubAddress::Socket(path),
            ..LinkConfig::new("kazoo-wall", 2, 48_000, 256)
        })
        .unwrap()
        .into_parts()
    });
    let (mut stream, _) = listener.accept().unwrap();
    let mut frame = FrameBuffer::new();
    assert_eq!(
        frame.read_frame(&mut stream).unwrap().msg_type,
        MSG_REGISTER
    );
    RegisteredMsg {
        strips: Vec::new(),
        strip_index: 1,
        hub_sample_rate: 48_000,
        hub_buffer_size: 256,
        transport_state: 0,
        bpm: 120.0,
        position: 0,
    }
    .encode(frame.payload_mut());
    frame
        .write_frame(MSG_REGISTERED, 0, RegisteredMsg::WIRE_SIZE, &mut stream)
        .unwrap();
    let (link, audio) = linking.join().unwrap();
    assert!(link.is_connected());
    rig.go("#4");
    assert_eq!(rig.wall.patch.tempo.to_bits(), 120.0_f64.to_bits());
    // The desk is linked while the go decides whether to keep a save, and
    // has moved the tempo off the moment's own; it is gone before the go
    // lands, so the go settles on its landing without waiting for a reply.
    rig.wall.desk = Some(link);
    rig.wall.patch.tempo = 128.0;
    let before = rig.wall.patch.fingerprint();
    let count = rig.wall.saves_store().unwrap().list_saves().unwrap().len();
    let ticket = rig
        .wall
        .request_as(
            "Tom",
            true,
            &request(serde_json::json!({"op":"go", "moment":"#5"})),
            rig.now,
        )
        .unwrap()
        .pending
        .unwrap();
    rig.wall.desk = None;
    let go = loop {
        rig.wall.tree_test_wait();
        rig.land();
        let answers = rig.wall.tree_answers(rig.now);
        if let Some((_, result)) = answers.into_iter().find(|(t, _)| *t == ticket) {
            break result.unwrap().change.unwrap();
        }
    };
    rig.ask(
        "Tom",
        true,
        serde_json::json!({"op":"undo", "change":go.seq}),
    )
    .unwrap();
    assert_eq!(rig.wall.patch.fingerprint(), before);
    assert_eq!(rig.wall.patch.tempo.to_bits(), 128.0_f64.to_bits());
    assert!(
        rig.wall.tree_kept.is_none(),
        "a save is not a verified tree moment"
    );
    assert_eq!(
        rig.wall.saves_store().unwrap().list_saves().unwrap().len(),
        count + 1
    );
    rig.wall.close(rig.now);
    drop(audio);
    drop(stream);
}

#[test]
fn two_goes_from_tree_moments_keep_no_extra_save_and_undo_returns() {
    let mut rig = Rig::new(None);
    rig.prepare();
    rig.go("#4");
    let before = rig.wall.patch.fingerprint();
    let at = rig.listing().here.unwrap();
    let count = rig.wall.saves_store().unwrap().list_saves().unwrap().len();
    let go = rig.go("#5").change.unwrap();
    let What::Go { backup, .. } = &go.what else {
        panic!("not a go")
    };
    assert_eq!(backup, &format!("tree:{}", &at.commit[..12]));
    rig.go("#6");
    assert_eq!(
        rig.wall.saves_store().unwrap().list_saves().unwrap().len(),
        count
    );
    rig.ask(
        "Tom",
        true,
        serde_json::json!({"op":"undo", "change":go.seq}),
    )
    .unwrap();
    assert_eq!(rig.wall.patch.fingerprint(), before);
    assert_eq!(rig.listing().here.unwrap().commit, at.commit);
    rig.wall.close(rig.now);
}

#[test]
fn a_go_after_an_unsaved_edit_keeps_one_save() {
    let mut rig = Rig::new(None);
    rig.prepare();
    rig.go("#4");
    let second = rig.go("#5").change.unwrap();
    assert!(matches!(second.what, What::Go { backup, .. } if backup.starts_with("tree:")));
    let count = rig.wall.saves_store().unwrap().list_saves().unwrap().len();
    rig.turn(3500.0);
    let go = rig.go("#6").change.unwrap();
    let What::Go { backup, .. } = go.what else {
        panic!("not a go")
    };
    assert!(backup.starts_with("before-go-"));
    assert_eq!(
        rig.wall.saves_store().unwrap().list_saves().unwrap().len(),
        count + 1
    );
    rig.wall.close(rig.now);
}

#[test]
fn undoing_a_tree_backup_survives_a_restart() {
    let mut rig = Rig::new(None);
    rig.prepare();
    rig.go("#4");
    let before = rig.wall.patch.fingerprint();
    let go = rig.go("#5").change.unwrap();
    let What::Go { backup, .. } = &go.what else {
        panic!("not a go")
    };
    assert!(backup.starts_with("tree:"));
    rig.wall.close(rig.now);
    let store = Store::open(rig.state.path()).unwrap();
    let crate::store::Loaded::Patch { patch, .. } = store.load_patch().unwrap() else {
        panic!("no wall")
    };
    let history = store.load_log().unwrap().changes;
    let (engine, control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    rig.engine = engine;
    rig.wall = Wall::new(*patch, control, None, history, Some(store));
    rig.wall
        .attach_tree(TreeHandle::attach(rig.tree.path()).unwrap());
    rig.ask(
        "Tom",
        true,
        serde_json::json!({"op":"undo", "change":go.seq}),
    )
    .unwrap();
    assert_eq!(rig.wall.patch.fingerprint(), before);
    rig.wall.close(rig.now);
}
