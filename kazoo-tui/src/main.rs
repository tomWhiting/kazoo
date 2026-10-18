//! kazoo-tui on its own: the same instrument `kazoo tui` runs.

fn main() -> std::process::ExitCode {
    kazoo_host::reports::run("kazoo-tui", || kazoo_tui::cli(std::env::args().skip(1)))
}
