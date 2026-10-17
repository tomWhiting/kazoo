use std::ffi::OsStr;

use color_eyre::Result;
use color_eyre::eyre::eyre;

pub(super) fn pane_environment(runtime: &str, desk: Option<&OsStr>) -> Result<String> {
    let mut words = vec![format!("XDG_RUNTIME_DIR={runtime}")];
    if let Some(desk) = desk {
        let desk = desk
            .to_str()
            .ok_or_else(|| eyre!("KAZOO_DESK is not valid UTF-8"))?;
        kazoo_core::ipc::link::HubAddress::parse(desk)?;
        // A running tmux server retains its own environment between clients.
        words.push(format!("KAZOO_DESK={desk}"));
    }
    super::shell_command(words.iter().map(String::as_str))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desk_reaches_tmux_panes_with_shell_quoting() {
        assert_eq!(
            pane_environment("/runtime", None).unwrap(),
            "'XDG_RUNTIME_DIR=/runtime'"
        );
        assert_eq!(
            pane_environment("/runtime", Some(OsStr::new("old.local:7700"))).unwrap(),
            "'XDG_RUNTIME_DIR=/runtime' 'KAZOO_DESK=old.local:7700'"
        );
        let environment =
            pane_environment("/runtime", Some(OsStr::new("/tmp/desk's socket"))).unwrap();
        assert!(environment.contains("'\"'\"'"));
        assert!(pane_environment("/runtime", Some(OsStr::new("invalid"))).is_err());
    }
}
