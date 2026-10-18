//! kazoo-mini on its own: the same instrument `kazoo mini` runs.

fn main() -> std::process::ExitCode {
    kazoo_host::reports::run("kazoo-mini", || kazoo_mini::cli(std::env::args().skip(1)))
}
