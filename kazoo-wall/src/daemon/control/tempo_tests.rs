use super::*;

#[test]
fn hello_refuses_reserved_seat_names() {
    let (_engine, engine_control) = engine(EngineConfig::new(48_000, 120.0, 0.0), None);
    let wall = Wall::new(
        crate::seed::seed().unwrap(),
        engine_control,
        None,
        Vec::new(),
        None,
    );
    let (listen_tx, listen_rx) = std::sync::mpsc::channel();
    let now = Instant::now();
    let mut control = Control {
        wall,
        conns: HashMap::new(),
        audio: AudioMode::Headless {
            sample_rate: 48_000,
        },
        desk: DeskMode::Off,
        backend: None,
        listen_tx,
        listen_rx,
        stop: Arc::new(AtomicBool::new(false)),
        stopping: false,
        heard: false,
        last_tick: now,
        awaiting: HashMap::new(),
        counts: AudioCounts::new(now),
        told_link_refused: false,
        desk_refusals: link_diagnostics::DeskRefusals::default(),
        reported_link_shutdowns: link_diagnostics::LinkShutdowns::default(),
        following_desk: None,
        desk_rate_tried: None,
    };
    for seat in [
        "desk",
        "Desk",
        "DESK",
        "kazoo-wall",
        "Kazoo-Wall",
        "KAZOO-WALL",
    ] {
        let response = control.hello(1, 2, seat, "test", false, false);
        assert!(!response.ok, "{seat} must be reserved");
        let error = response.error.unwrap();
        assert_eq!(error.code, ErrorCode::BadName);
        assert_eq!(
            error.message,
            format!("'{seat}' is a reserved seat name; choose another name.")
        );
    }
}
