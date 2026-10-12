mod tests {
    #[tokio::test]
    async fn wall_chain_tool_lists_saves() {
        let tools = crate::tools::Seat::tool_router().list_all();
        let tool = tools
            .iter()
            .find(|tool| tool.name == "wall_chain")
            .expect("wall_chain is registered");
        let schema = serde_json::Value::Object(tool.input_schema.as_ref().clone());
        assert!(schema.to_string().contains("\"list\""));

        let log_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/kz-logs");
        let log_dir = std::fs::canonicalize(log_dir).unwrap();
        let runtime = tempfile::Builder::new()
            .prefix("mc")
            .tempdir_in(&log_dir)
            .unwrap();
        let state = tempfile::Builder::new()
            .prefix("chains")
            .tempdir_in(&log_dir)
            .unwrap();
        let daemon = kazoo_wall::daemon::Daemon::start(kazoo_wall::daemon::DaemonConfig {
            load_guard: false,
            ..kazoo_wall::daemon::DaemonConfig::headless(runtime.path(), state.path())
        })
        .unwrap();
        let wall = std::sync::Arc::new(crate::link::Wall::new(
            daemon.socket().to_path_buf(),
            "ChainTester".to_string(),
        ));
        let (feed, mut heard) = tokio::sync::mpsc::unbounded_channel();
        let (peer, receiver) = tokio::sync::watch::channel(None);
        let seat = crate::tools::Seat::new(wall, feed, peer);
        let args = |action, name, modules| {
            rmcp::handler::server::wrapper::Parameters(crate::tools::ChainArgs {
                action,
                name,
                modules,
                group: None,
            })
        };
        let saved = seat
            .wall_chain(args(
                crate::tools::ChainAction::Save,
                Some("voice".to_string()),
                vec!["vco1".to_string()],
            ))
            .await;
        assert_ne!(saved.is_error, Some(true), "{saved:?}");
        let listed = seat
            .wall_chain(args(crate::tools::ChainAction::List, None, Vec::new()))
            .await;
        assert_ne!(listed.is_error, Some(true), "{listed:?}");
        assert!(listed.content.iter().any(|block| matches!(block, rmcp::model::ContentBlock::Text(text) if text.text == "Chains: voice.")), "{listed:?}");
        let added = seat
            .wall_chain(args(
                crate::tools::ChainAction::Add,
                Some("voice".to_string()),
                Vec::new(),
            ))
            .await;
        assert_ne!(added.is_error, Some(true), "{added:?}");
        assert!(matches!(
            heard.recv().await,
            Some(crate::channel::Feed::Own(_))
        ));
        drop(receiver);
        daemon.stop();
        daemon.wait().unwrap();
    }
}
