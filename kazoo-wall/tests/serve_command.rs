use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use kazoo_wall::daemon::{ServeCommand, launch_detached};

#[test]
fn launch_detached_spawns_the_serve_command_as_given() {
    let root = tempfile::tempdir().unwrap();
    let log = root.path().join("daemon.log");
    let command = ServeCommand {
        program: PathBuf::from("/bin/echo"),
        args: [
            "wall",
            "serve",
            "--no-audio",
            "--no-desk",
            "--rate",
            "44100",
        ]
        .map(OsString::from)
        .to_vec(),
    };
    let error = launch_detached(
        &command,
        &root.path().join("wall.sock"),
        &log,
        Duration::from_secs(10),
    )
    .unwrap_err();
    assert!(error.to_string().contains("the daemon exited"), "{error}");
    assert_eq!(
        fs::read_to_string(log).unwrap(),
        "wall serve --no-audio --no-desk --rate 44100\n"
    );
}
