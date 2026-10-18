//! One program by three names: each says its own in its help and version.

use std::os::unix::process::CommandExt;
use std::process::Command;

#[test]
fn kzoo_kazoo_and_kzhz_each_answer_to_their_own_name() {
    for (program, name) in [
        (env!("CARGO_BIN_EXE_kzoo"), "kzoo"),
        (env!("CARGO_BIN_EXE_kazoo"), "kazoo"),
        (env!("CARGO_BIN_EXE_kzhz"), "kzhz"),
    ] {
        let help = Command::new(program).arg("--help").output().unwrap();
        assert_eq!(help.status.code(), Some(0), "{name}");
        let text = String::from_utf8_lossy(&help.stdout);
        assert!(text.contains(&format!("Usage: {name} ")), "{name}: {text}");
        let version = Command::new(program).arg("--version").output().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&version.stdout).trim(),
            format!("{name} {}", env!("CARGO_PKG_VERSION"))
        );
        // A wrong argument is refused under the name it was run by.
        let wrong = Command::new(program)
            .arg("--no-such-flag")
            .output()
            .unwrap();
        assert_eq!(wrong.status.code(), Some(2), "{name}");
        let text = String::from_utf8_lossy(&wrong.stderr);
        assert!(text.contains(&format!("Usage: {name} ")), "{name}: {text}");
    }
}

#[test]
fn kzoo_run_by_npm_as_kazoo_answers_to_kazoo() {
    let help = Command::new(env!("CARGO_BIN_EXE_kzoo"))
        .arg("--help")
        .arg0("kazoo")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&help.stdout);
    assert!(text.contains("Usage: kazoo "), "{text}");
    // And as kzhz, npm's own name.
    let help = Command::new(env!("CARGO_BIN_EXE_kzoo"))
        .arg("--help")
        .arg0("kzhz")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&help.stdout);
    assert!(text.contains("Usage: kzhz "), "{text}");
    // Anything else it is told is no name of its own.
    let help = Command::new(env!("CARGO_BIN_EXE_kzoo"))
        .arg("--help")
        .arg0("other")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&help.stdout);
    assert!(text.contains("Usage: kzoo "), "{text}");
    // What 0.3.3's launcher left in the environment names nothing now.
    let help = Command::new(env!("CARGO_BIN_EXE_kzoo"))
        .arg("--help")
        .env("KZOO_RUN_AS", "kzhz")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&help.stdout);
    assert!(text.contains("Usage: kzoo "), "{text}");
}
