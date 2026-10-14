use std::process::Command;

use kazoo_wall::protocol::Request;
use kazoo_wall::protocol::client::WallClient;

#[test]
fn go_and_mark_leave_environment_tree_and_reels_untouched() {
    if std::env::var_os("KAZOO_ISOLATION_CHILD").is_some() {
        let (runtime, state) = super::dirs();
        let tree = state.path().join("tree");
        kazoo_wall::tree::git::plant(&tree).unwrap();
        let daemon = super::start(runtime.path(), state.path());
        let mut wall = WallClient::connect(daemon.socket(), "Tom", "test", true).unwrap();
        wall.turn("vcf1", "cutoff", 800.0, Some(0.0)).unwrap();
        wall.call(Request::Mark {
            name: "dawn".into(),
        })
        .unwrap();
        wall.turn("vcf1", "cutoff", 700.0, Some(0.0)).unwrap();
        wall.call(Request::Go {
            moment: "dawn".into(),
        })
        .unwrap();
        drop(wall);
        daemon.stop();
        daemon.wait().unwrap();
        assert!(state.path().join("reels").is_dir());
        return;
    }
    let sentinel = tempfile::tempdir().unwrap();
    let reels = tempfile::tempdir().unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "tree_isolation::go_and_mark_leave_environment_tree_and_reels_untouched",
            "--nocapture",
        ])
        .env("KAZOO_ISOLATION_CHILD", "1")
        .env("KAZOO_TREE_DIR", sentinel.path())
        .env("KAZOO_REELS_DIR", reels.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(std::fs::read_dir(sentinel.path()).unwrap().count(), 0);
    assert_eq!(std::fs::read_dir(reels.path()).unwrap().count(), 0);
}
