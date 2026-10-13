use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use color_eyre::Result;
use color_eyre::eyre::{WrapErr, eyre};
use kazoo_core::ipc::{client::HubIpcClient, discovery};

use super::{LaunchPlan, TmuxCommand};

const DESK_WAIT: Duration = Duration::from_secs(5);
const PROBE_INTERVAL: Duration = Duration::from_millis(20);

fn output(tmux: &Path, args: &[String]) -> Result<Output> {
    let output = Command::new(tmux)
        .args(args)
        .output()
        .wrap_err_with(|| format!("starting tmux {args:?}"))?;
    if !output.status.success() {
        return Err(eyre!(
            "tmux {args:?} exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output)
}

fn invoke(tmux: &Path, command: &TmuxCommand) -> Result<()> {
    output(tmux, &command.args)?;
    Ok(())
}

pub(super) fn up(tmux: &Path, plan: &LaunchPlan, rate: Option<u32>) -> Result<()> {
    let socket = discovery::default_socket_path();
    if discovery::hub_listening(&socket)? {
        return Err(eyre!(
            "a desk already accepts on {}; stop that desk before starting this studio",
            socket.display()
        ));
    }
    let first = plan
        .commands
        .first()
        .ok_or_else(|| eyre!("no desk launch command"))?;
    let started = output(tmux, &first.args)?;
    let result = start_children(tmux, plan, rate, &socket, &started.stdout);
    if let Err(error) = result {
        return match cleanup(tmux, &plan.session, &plan.children[0]) {
            Ok(()) => Err(error),
            Err(cleanup) => Err(eyre!("{error:#}; studio cleanup also failed: {cleanup:#}")),
        };
    }
    Ok(())
}

fn start_children(
    tmux: &Path,
    plan: &LaunchPlan,
    rate: Option<u32>,
    socket: &Path,
    desk_output: &[u8],
) -> Result<()> {
    let pid = parse_pid(std::str::from_utf8(desk_output)?.trim())?;
    wait_for_desk(tmux, &plan.session, socket, pid, rate)?;
    let (attach, children) = plan.commands[1..]
        .split_last()
        .ok_or_else(|| eyre!("no studio attach command"))?;
    for command in children {
        invoke(tmux, command)?;
    }
    let status = Command::new(tmux)
        .args(&attach.args)
        .status()
        .wrap_err("attaching to the studio")?;
    if status.success() {
        Ok(())
    } else {
        Err(eyre!("tmux attach exited {status}"))
    }
}

fn wait_for_desk(
    tmux: &Path,
    session: &str,
    socket: &Path,
    pid: u32,
    rate: Option<u32>,
) -> Result<()> {
    let deadline = Instant::now() + DESK_WAIT;
    loop {
        // Binding the socket precedes publishing the PID file. Both must
        // agree before an instrument can join this desk.
        let advertised = discovery::read_pid_file()?;
        if advertised
            .as_ref()
            .is_some_and(|record| record.pid == pid && record.socket == socket)
            && discovery::hub_listening(socket)?
        {
            if let Some(rate) = rate {
                // The desk opens at its native device rate. A registration
                // rejects a different requested rate instead of silently
                // starting a session at the wrong rate.
                let probe = HubIpcClient::connect_to(socket, "studio rate check", 2, rate, 256)
                    .wrap_err_with(|| {
                        format!("desk did not accept the configured {rate} Hz rate")
                    })?;
                drop(probe);
            }
            return Ok(());
        }
        let panes = pane_commands(tmux, session)?;
        if !panes
            .iter()
            .any(|(pane_pid, _, dead)| *pane_pid == pid && !dead)
        {
            return Err(eyre!(
                "studio desk PID {pid} exited before accepting connections"
            ));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(eyre!(
                "studio desk did not accept on {} within 5 seconds",
                socket.display()
            ));
        }
        std::thread::sleep(PROBE_INTERVAL.min(remaining));
    }
}

fn parse_pid(text: &str) -> Result<u32> {
    let pid = text
        .parse::<u32>()
        .wrap_err("tmux returned an invalid pane PID")?;
    if pid == 0 || pid > i32::MAX as u32 {
        return Err(eyre!("tmux returned an invalid pane PID: {pid}"));
    }
    Ok(pid)
}

fn pane_commands(tmux: &Path, session: &str) -> Result<Vec<(u32, String, bool)>> {
    let args = [
        "list-panes".into(),
        "-s".into(),
        "-t".into(),
        format!("={session}"),
        "-F".into(),
        "#{pane_pid}\t#{pane_dead}\t#{pane_start_command}".into(),
    ];
    let output = Command::new(tmux)
        .args(&args)
        .output()
        .wrap_err("listing studio panes")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if absent_server(&stderr) || stderr.trim() == format!("can't find session: {session}") {
            return Ok(Vec::new());
        }
        return Err(eyre!(
            "tmux {args:?} exited {}: {}",
            output.status,
            stderr.trim()
        ));
    }
    std::str::from_utf8(&output.stdout)?
        .lines()
        .map(|line| {
            let mut fields = line.splitn(3, '\t');
            let pid = fields.next().ok_or_else(|| eyre!("missing pane PID"))?;
            let dead = match fields.next() {
                Some("0") => false,
                Some("1") => true,
                _ => return Err(eyre!("tmux returned an invalid pane state")),
            };
            let command = fields.next().ok_or_else(|| eyre!("missing pane command"))?;
            Ok((parse_pid(pid)?, command.to_owned(), dead))
        })
        .collect()
}

pub(super) fn down(tmux: &Path, session: &str, desk: &str) -> Result<()> {
    if !session_exists(tmux, session)? {
        return Err(eyre!("tmux studio session {session:?} does not exist"));
    }
    cleanup(tmux, session, desk)
}

fn cleanup(tmux: &Path, session: &str, desk: &str) -> Result<()> {
    let desk_command = format!("exec {desk}");
    let panes = pane_commands(tmux, session)?;
    let desk_pids: Vec<_> = panes
        .iter()
        .filter(|(_, command, dead)| *command == desk_command && !dead)
        .map(|(pid, _, _)| *pid)
        .collect();
    for pid in &desk_pids {
        let output = Command::new("/bin/kill")
            .args(["-TERM", &pid.to_string()])
            .output()
            .wrap_err_with(|| format!("sending SIGTERM to studio desk PID {pid}"))?;
        if !output.status.success() {
            return Err(eyre!(
                "SIGTERM to desk PID {pid} exited {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }
    let deadline = Instant::now() + DESK_WAIT;
    for pid in &desk_pids {
        while pane_commands(tmux, session)?
            .iter()
            .any(|(pane_pid, _, dead)| *pane_pid == *pid && !dead)
        {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(eyre!(
                    "studio desk PID {pid} did not exit after SIGTERM within 5 seconds"
                ));
            }
            std::thread::sleep(PROBE_INTERVAL.min(remaining));
        }
    }
    // The session may disappear with its final pane when the desk exits.
    // Inspect exact session names before killing, so a similarly named
    // session can never be selected by tmux's prefix matching.
    if session_exists(tmux, session)? {
        output(
            tmux,
            &["kill-session".into(), "-t".into(), format!("={session}")],
        )?;
    }
    Ok(())
}

fn session_exists(tmux: &Path, session: &str) -> Result<bool> {
    let output = Command::new(tmux)
        .args(["list-sessions", "-F", "#{session_name}"])
        .output()
        .wrap_err("checking remaining tmux sessions")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if absent_server(&stderr) {
            return Ok(false);
        }
        return Err(eyre!(
            "tmux list-sessions exited {}: {}",
            output.status,
            stderr.trim()
        ));
    }
    Ok(std::str::from_utf8(&output.stdout)?
        .lines()
        .any(|name| name == session))
}

fn absent_server(stderr: &str) -> bool {
    stderr.contains("no server running")
        || stderr.contains("error connecting") && stderr.contains("No such file or directory")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_pids_cannot_signal_a_process_group() {
        for text in ["0", "-1", "2147483648", "not-a-pid", ""] {
            assert!(parse_pid(text).is_err(), "{text}");
        }
        assert_eq!(parse_pid("1234").unwrap(), 1234);
    }
}
