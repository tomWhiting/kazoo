use std::ffi::{OsStr, OsString};
use std::io;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

pub(super) fn apply(address: &str) -> io::Result<()> {
    let current = std::env::var_os("KAZOO_DESK");
    let mut arguments = std::env::args_os();
    // Started again by the name it was started by, which it answers to.
    let started_as = arguments.next();
    if let Some(mut command) = relaunch(
        address,
        current.as_deref(),
        std::env::current_exe()?,
        started_as,
        arguments,
    ) {
        // Re-execution installs the environment before libraries start threads.
        return Err(command.exec());
    }
    Ok(())
}

fn relaunch(
    address: &str,
    current: Option<&OsStr>,
    executable: PathBuf,
    started_as: Option<OsString>,
    arguments: impl IntoIterator<Item = OsString>,
) -> Option<Command> {
    if current == Some(OsStr::new(address)) {
        return None;
    }
    let mut command = Command::new(executable);
    if let Some(name) = started_as {
        command.arg0(name);
    }
    command.args(arguments).env("KAZOO_DESK", address);
    Some(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desk_reexec_preserves_arguments_and_sets_child_environment() {
        if std::env::var_os("KAZOO_DESK_REEXEC_PROBE").is_some() {
            assert_eq!(std::env::var("KAZOO_DESK").unwrap(), "old.local:7700");
            assert_eq!(std::env::args_os().next(), Some(OsString::from("kzhz")));
            println!("desk environment received");
            return;
        }
        let mut command = relaunch(
            "old.local:7700",
            Some(OsStr::new("other.local:8800")),
            std::env::current_exe().unwrap(),
            Some(OsString::from("kzhz")),
            [
                "--exact",
                "entry::desk::tests::desk_reexec_preserves_arguments_and_sets_child_environment",
                "--nocapture",
            ]
            .map(OsString::from),
        )
        .unwrap();
        let output = command
            .env("KAZOO_DESK_REEXEC_PROBE", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("desk environment received"));
        assert!(
            relaunch(
                "old.local:7700",
                Some(OsStr::new("old.local:7700")),
                PathBuf::from("unused"),
                None,
                []
            )
            .is_none()
        );
    }
}
