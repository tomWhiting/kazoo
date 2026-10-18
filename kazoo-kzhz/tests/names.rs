//! The kzhz package's three commands: each answers to its own name, as
//! the kzoo package's do.

use std::os::unix::process::CommandExt;
use std::process::Command;

#[test]
fn kzhz_kzoo_and_kazoo_each_answer_to_their_own_name() {
    for (program, name) in [
        (env!("CARGO_BIN_EXE_kzhz"), "kzhz"),
        (env!("CARGO_BIN_EXE_kzoo"), "kzoo"),
        (env!("CARGO_BIN_EXE_kazoo"), "kazoo"),
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
    }
}

#[test]
fn its_kzoo_run_by_npm_answers_as_it_was_run() {
    for name in ["kzhz", "kazoo"] {
        let help = Command::new(env!("CARGO_BIN_EXE_kzoo"))
            .arg("--help")
            .arg0(name)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&help.stdout);
        assert!(text.contains(&format!("Usage: {name} ")), "{name}: {text}");
    }
}
