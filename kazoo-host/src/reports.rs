//! Error reports, installed once per process.

use std::process::ExitCode;
use std::sync::Once;

/// Install `color_eyre`'s error and panic reports, once per process.
///
/// A panic is a fault, and its report keeps where it happened. An error is
/// the program telling whoever ran it what went wrong, so its report is
/// just that, with no source location or backtrace advice: it must never
/// read as a crash.
///
/// The first call installs them and returns what installing returned;
/// every later call installs nothing and returns `Ok(())`, so each
/// instrument's `run` under the one `kazoo` binary may call it.
///
/// # Errors
///
/// The first call fails if reports were already installed by other means.
pub fn install() -> color_eyre::Result<()> {
    static ONCE: Once = Once::new();
    let mut installed = Ok(());
    ONCE.call_once(|| installed = hooks());
    installed
}

/// Install the reports. An error's report leaves out its location and
/// environment sections. The environment section is the backtrace advice,
/// and `color_eyre` leaves it out of a panic's report too, so a panic's
/// report says it in a section of its own; a panic's report always keeps
/// its location.
fn hooks() -> color_eyre::Result<()> {
    builder().install()
}

fn builder() -> color_eyre::config::HookBuilder {
    color_eyre::config::HookBuilder::default()
        .display_location_section(false)
        .display_env_section(false)
        .panic_section(BACKTRACE_ADVICE)
}

/// What a panic's report says about seeing more.
const BACKTRACE_ADVICE: &str =
    "Run with RUST_BACKTRACE=1 to see the backtrace, or RUST_BACKTRACE=full for all of it.";

/// The exit code for `result`, with its report said under the program's
/// `name` when it failed.
///
/// The report is one line, its causes after it: the program telling
/// whoever ran it what went wrong (a panic has its own report).
#[must_use]
pub fn finish(name: &str, result: color_eyre::Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(report) => {
            eprintln!("{name}: {report:#}");
            ExitCode::FAILURE
        }
    }
}

/// Run a program as `name`: the reports installed, then `body`, finished
/// as [`finish`] says.
#[must_use]
pub fn run(name: &str, body: impl FnOnce() -> color_eyre::Result<()>) -> ExitCode {
    finish(name, install().and_then(|()| body()))
}

#[cfg(test)]
mod tests {
    use super::install;

    #[test]
    fn install_twice_is_fine() {
        install().expect("first install");
        install().expect("second install is a quiet no-op");
    }

    #[test]
    fn an_error_reads_as_an_error_and_not_a_crash() {
        install().expect("install");
        let report = format!("{:?}", color_eyre::eyre::eyre!("no tree here"));
        assert!(report.contains("no tree here"), "{report}");
        for crash in ["Location", "RUST_BACKTRACE", "Backtrace"] {
            assert!(!report.contains(crash), "{crash} in {report}");
        }
        // What a program says: one line, its causes after it.
        let said = format!("{:#}", color_eyre::eyre::eyre!("a").wrap_err("b"));
        assert_eq!(said, "b: a");
        assert_eq!(
            super::finish("kzoo", Err(color_eyre::eyre::eyre!("x"))),
            std::process::ExitCode::FAILURE
        );
        assert_eq!(
            super::finish("kzoo", Ok(())),
            std::process::ExitCode::SUCCESS
        );
    }

    #[test]
    fn a_panic_still_reads_as_one() {
        use std::sync::{Arc, Mutex};
        let (panic, _errors) = super::builder().try_into_hooks().expect("hooks");
        let said = Arc::new(Mutex::new(String::new()));
        let heard = Arc::clone(&said);
        std::panic::set_hook(Box::new(move |info| {
            *heard.lock().expect("unpoisoned") = panic.panic_report(info).to_string();
        }));
        let caught = std::panic::catch_unwind(|| panic!("boom"));
        drop(std::panic::take_hook());
        assert!(caught.is_err());
        let report = said.lock().expect("unpoisoned").clone();
        for part in ["boom", "Location", "RUST_BACKTRACE=1"] {
            assert!(report.contains(part), "{part} not in {report}");
        }
    }
}
