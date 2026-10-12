use std::process::Command;

#[test]
fn cli_help_is_flushed_and_takes_precedence() {
    for args in [
        ["--bogus", "--help"].as_slice(),
        ["serve", "--rate", "-h"].as_slice(),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_kazoo-wall"))
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), kazoo_wall::USAGE);
        assert!(output.stderr.is_empty(), "{output:?}");
    }
}

#[test]
fn cli_rejects_a_bad_rate_before_starting_the_daemon() {
    let output = Command::new(env!("CARGO_BIN_EXE_kazoo-wall"))
        .args(["serve", "--rate", "NaN"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(
        error.contains("--rate takes a sample rate from 8000 to 384000"),
        "{error}"
    );
    for line in kazoo_wall::USAGE.lines().filter(|line| !line.is_empty()) {
        assert!(
            error.contains(line.trim()),
            "missing usage line {line:?}: {error}"
        );
    }
}
