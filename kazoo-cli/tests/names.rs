//! One program by two names: each says its own in its help and version.

use std::process::Command;

#[test]
fn kzoo_and_kazoo_each_answer_to_their_own_name() {
    for (program, name) in [
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
