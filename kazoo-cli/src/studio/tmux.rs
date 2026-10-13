use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use color_eyre::Result;
use color_eyre::eyre::{WrapErr, eyre};
use kazoo_core::ipc::{client::HubIpcClient, discovery};

use super::{DESK_OPTION, LaunchPlan, TmuxCommand};

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
    if session_exists(tmux, &plan.session)? {
        return Err(eyre!(
            "tmux session {:?} already exists; stop it before starting this studio",
            plan.session
        ));
    }
    let first = plan
        .commands
        .first()
        .ok_or_else(|| eyre!("no desk launch command"))?;
    // The session did not exist a moment ago, so whatever of it a failed
    // creation left behind is this studio's own to remove.
    let started = match output(tmux, &first.args) {
        Ok(started) => started,
        // Another client created the name since the check: theirs to keep.
        Err(error) if error.to_string().contains("duplicate session") => return Err(error),
        Err(error) => return abandon(tmux, &plan.session, None, error),
    };
    let pid = match std::str::from_utf8(&started.stdout)
        .map_err(color_eyre::Report::from)
        .and_then(|text| parse_pid(text.trim()))
    {
        Ok(pid) => pid,
        Err(error) => return abandon(tmux, &plan.session, None, error),
    };
    match start_children(tmux, plan, rate, &socket, pid) {
        Ok(()) => Ok(()),
        Err(error) => abandon(tmux, &plan.session, Some(pid), error),
    }
}

/// Stop a studio that failed to start, keeping the failure that stopped it.
fn abandon(
    tmux: &Path,
    session: &str,
    desk: Option<u32>,
    error: color_eyre::Report,
) -> Result<()> {
    match stop(tmux, session, desk) {
        Ok(()) => Err(error),
        Err(cleanup) => Err(eyre!("{error:#}; studio cleanup also failed: {cleanup:#}")),
    }
}

fn start_children(
    tmux: &Path,
    plan: &LaunchPlan,
    rate: Option<u32>,
    socket: &Path,
    pid: u32,
) -> Result<()> {
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
        if !desk_alive(tmux, session, pid)? {
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

/// One pane of a studio session, as tmux lists it.
struct Pane {
    pid: u32,
    dead: bool,
}

fn panes(tmux: &Path, session: &str) -> Result<Vec<Pane>> {
    let args = [
        "list-panes".into(),
        "-s".into(),
        "-t".into(),
        format!("={session}"),
        "-F".into(),
        "#{pane_pid}\t#{pane_dead}".into(),
    ];
    let output = Command::new(tmux)
        .args(&args)
        .output()
        .wrap_err("listing studio panes")?;
    if !output.status.success() {
        // tmux words a vanished session several ways as it closes (can't
        // find session, no current target, server exited unexpectedly), so
        // the session list decides instead of the message.
        if !session_exists(tmux, session)? {
            return Ok(Vec::new());
        }
        return Err(eyre!(
            "tmux {args:?} exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    std::str::from_utf8(&output.stdout)?
        .lines()
        .map(|line| {
            let (pid, dead) = line
                .split_once('\t')
                .ok_or_else(|| eyre!("tmux returned an invalid pane line {line:?}"))?;
            let dead = match dead {
                "0" => false,
                "1" => true,
                _ => return Err(eyre!("tmux returned an invalid pane state")),
            };
            Ok(Pane {
                pid: parse_pid(pid)?,
                dead,
            })
        })
        .collect()
}

/// Whether a live pane of `session` runs process `pid`. Only such a PID is
/// ever signalled, so a recycled PID outside the session is never touched.
fn desk_alive(tmux: &Path, session: &str, pid: u32) -> Result<bool> {
    Ok(panes(tmux, session)?
        .iter()
        .any(|pane| pane.pid == pid && !pane.dead))
}

/// The desk PID that `up` recorded on `session`, or `None` when the session
/// does not carry the studio's mark.
fn studio_desk(tmux: &Path, session: &str) -> Result<Option<u32>> {
    let shown = output(
        tmux,
        &[
            "display-message".into(),
            "-p".into(),
            "-t".into(),
            format!("={session}:"),
            format!("#{{{DESK_OPTION}}}"),
        ],
    )?;
    let text = std::str::from_utf8(&shown.stdout)?.trim();
    if text.is_empty() {
        Ok(None)
    } else {
        parse_pid(text).map(Some)
    }
}

pub(super) fn down(tmux: &Path, session: &str) -> Result<()> {
    if !session_exists(tmux, session)? {
        return Err(eyre!("tmux studio session {session:?} does not exist"));
    }
    let desk = studio_desk(tmux, session)?.ok_or_else(|| {
        eyre!(
            "tmux session {session:?} was not started by kazoo studio up, so it was left running"
        )
    })?;
    stop(tmux, session, Some(desk))
}

/// SIGTERM the desk, wait for it, then kill whatever remains of the session.
fn stop(tmux: &Path, session: &str, desk: Option<u32>) -> Result<()> {
    if let Some(pid) = desk {
        if desk_alive(tmux, session, pid)? {
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
    }
    let lingering = match desk {
        Some(pid) => desk_lingers(tmux, session, pid)?,
        None => None,
    };
    // The session may disappear with its final pane when the desk exits.
    // Inspect exact session names before killing, so a similarly named
    // session can never be selected by tmux's prefix matching. A desk that
    // outlived SIGTERM is killed with the session rather than left running.
    if session_exists(tmux, session)? {
        output(
            tmux,
            &["kill-session".into(), "-t".into(), format!("={session}")],
        )?;
    }
    match lingering {
        Some(pid) => Err(eyre!(
            "studio desk PID {pid} did not exit within 5 seconds of SIGTERM, \
             so the session was killed with it still running"
        )),
        None => Ok(()),
    }
}

/// Wait up to five seconds for the desk to leave; `Some(pid)` if it did not.
fn desk_lingers(tmux: &Path, session: &str, pid: u32) -> Result<Option<u32>> {
    let deadline = Instant::now() + DESK_WAIT;
    while desk_alive(tmux, session, pid)? {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(Some(pid));
        }
        std::thread::sleep(PROBE_INTERVAL.min(remaining));
    }
    Ok(None)
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

/// Whether tmux failed because no server is there: none running, or the
/// server exiting as its last session closed while this client connected.
fn absent_server(stderr: &str) -> bool {
    stderr.contains("no server running")
        || stderr.contains("server exited unexpectedly")
        || stderr.contains("error connecting") && stderr.contains("No such file or directory")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use super::*;
    use crate::studio::{StudioFile, commands, find_tmux};

    /// A private tmux server under the lane's target folder, reached through a
    /// wrapper script so the code under test runs the real tmux unchanged.
    struct Harness {
        directory: PathBuf,
        tmux: PathBuf,
    }

    impl Harness {
        fn new(label: &str) -> Self {
            let target = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../target")
                .canonicalize()
                .unwrap();
            let directory = target
                .join("kz-logs")
                .join(format!("tm-{label}-{}", std::process::id()));
            fs::create_dir_all(&directory).unwrap();
            let real = find_tmux(std::env::var_os("PATH").as_deref())
                .unwrap()
                .expect("these tests drive the real tmux, which must be on PATH");
            let tmux = directory.join("tmux");
            write_script(
                &tmux,
                &format!(
                    "exec '{}' -S '{}' \"$@\"\n",
                    real.display(),
                    directory.join("s").display()
                ),
            );
            Self { directory, tmux }
        }

        fn script(&self, name: &str, body: &str) -> PathBuf {
            let path = self.directory.join(name);
            write_script(&path, body);
            path
        }

        fn session_exists(&self, session: &str) -> bool {
            session_exists(&self.tmux, session).unwrap()
        }

        fn wait_for_file(&self, name: &str) -> String {
            let deadline = Instant::now() + DESK_WAIT;
            loop {
                match fs::read_to_string(self.directory.join(name)) {
                    Ok(contents) => return contents,
                    Err(error) => {
                        assert!(Instant::now() < deadline, "{name} never appeared: {error}");
                        std::thread::sleep(PROBE_INTERVAL);
                    }
                }
            }
        }
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            match Command::new(&self.tmux).arg("kill-server").output() {
                Ok(output) if output.status.success() => {}
                Ok(output) => eprintln!(
                    "harness kill-server: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
                Err(error) => eprintln!("harness kill-server: {error}"),
            }
            if let Err(error) = fs::remove_dir_all(&self.directory) {
                eprintln!("harness cleanup {}: {error}", self.directory.display());
            }
        }
    }

    fn write_script(path: &Path, body: &str) {
        fs::write(path, format!("#!/bin/sh\n{body}")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Start a studio whose desk is `desk`, exactly as `up` starts one.
    fn start_desk(harness: &Harness, desk: &Path) -> (LaunchPlan, u32) {
        let plan = commands("demo", &StudioFile::default(), desk, &harness.directory).unwrap();
        let started = output(&harness.tmux, &plan.commands[0].args).unwrap();
        let pid = parse_pid(std::str::from_utf8(&started.stdout).unwrap().trim()).unwrap();
        (plan, pid)
    }

    #[test]
    fn invalid_pids_cannot_signal_a_process_group() {
        for text in ["0", "-1", "2147483648", "not-a-pid", ""] {
            assert!(parse_pid(text).is_err(), "{text}");
        }
        assert_eq!(parse_pid("1234").unwrap(), 1234);
    }

    #[test]
    fn every_planned_command_runs_on_real_tmux_and_opens_one_pane_per_child() {
        let harness = Harness::new("plan");
        let child = harness.script("child", "exec sleep 30\n");
        let file =
            StudioFile::parse("[[instrument]]\nname = '808'\n[[instrument]]\nname = 'mini'\n")
                .unwrap();
        let plan = commands("demo", &file, &child, &harness.directory).unwrap();
        let (attach, launch) = plan.commands.split_last().unwrap();
        assert_eq!(attach.args[0], "attach");
        for command in launch {
            output(&harness.tmux, &command.args).unwrap();
        }
        let listed = output(
            &harness.tmux,
            &[
                "list-panes".into(),
                "-s".into(),
                "-t".into(),
                format!("={}", plan.session),
                "-F".into(),
                "#{pane_dead}".into(),
            ],
        )
        .unwrap();
        assert_eq!(String::from_utf8(listed.stdout).unwrap(), "0\n0\n0\n");
    }

    #[test]
    fn down_sends_sigterm_to_the_desk_before_killing_the_session() {
        let harness = Harness::new("term");
        let directory = harness.directory.display().to_string();
        let desk = harness.script(
            "desk",
            &format!(
                "trap 'echo term > \"{directory}/term\"; exit 0' TERM\n\
                 echo up > \"{directory}/ready\"\n\
                 while :; do sleep 0.05; done\n"
            ),
        );
        let (plan, _) = start_desk(&harness, &desk);
        harness.wait_for_file("ready");
        down(&harness.tmux, &plan.session).unwrap();
        assert_eq!(harness.wait_for_file("term"), "term\n");
        assert!(!harness.session_exists(&plan.session));
    }

    #[test]
    fn down_leaves_a_session_studio_did_not_start() {
        let harness = Harness::new("foreign");
        output(
            &harness.tmux,
            &[
                "new-session".into(),
                "-d".into(),
                "-s".into(),
                "kazoo-demo".into(),
                "exec sleep 30".into(),
            ],
        )
        .unwrap();
        let desk = harness.script("desk", "exit 0\n");
        let plan =
            commands("demo", &StudioFile::default(), &desk, &harness.directory).unwrap();
        let error = down(&harness.tmux, &plan.session).unwrap_err();
        assert!(error.to_string().contains("not started by kazoo studio"), "{error}");
        assert!(harness.session_exists("kazoo-demo"));
    }

    #[test]
    fn down_kills_the_session_even_when_the_desk_ignores_sigterm() {
        let harness = Harness::new("stubborn");
        let directory = harness.directory.display().to_string();
        let desk = harness.script(
            "desk",
            &format!(
                "trap '' TERM\n\
                 echo up > \"{directory}/ready\"\n\
                 while :; do sleep 0.05; done\n"
            ),
        );
        let (plan, _) = start_desk(&harness, &desk);
        harness.wait_for_file("ready");
        let error = down(&harness.tmux, &plan.session).unwrap_err();
        assert!(error.to_string().contains("SIGTERM"), "{error}");
        assert!(!harness.session_exists(&plan.session));
    }

    #[test]
    fn a_desk_that_dies_shows_its_output_and_runs_in_the_studio_runtime() {
        let harness = Harness::new("dies");
        let desk = harness.script(
            "desk",
            "echo \"runtime=$XDG_RUNTIME_DIR\"\necho 'cannot open the output device' >&2\nexit 1\n",
        );
        let (plan, pid) = start_desk(&harness, &desk);
        let error = wait_for_desk(
            &harness.tmux,
            &plan.session,
            &harness.directory.join("hub.sock"),
            pid,
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("cannot open the output device"), "{error}");
        assert!(
            error.contains(&format!("runtime={}", harness.directory.display())),
            "{error}"
        );
    }
}
