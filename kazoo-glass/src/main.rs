//! kazoo-glass on its own: the same instrument `kazoo glass` runs.

fn main() -> std::process::ExitCode {
    kazoo_host::reports::run("kazoo-glass", || kazoo_glass::cli(std::env::args().skip(1)))
}
