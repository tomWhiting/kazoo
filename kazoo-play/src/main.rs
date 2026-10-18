//! kazoo-play on its own: the same tool `kazoo play` runs.

fn main() -> std::process::ExitCode {
    if let Err(report) = kazoo_host::reports::install() {
        eprintln!("kazoo-play: {report:#}");
        return std::process::ExitCode::FAILURE;
    }
    kazoo_play::cli(std::env::args().skip(1))
}
