//! kazoo-dx on its own: the same instrument `kazoo dx` runs.

fn main() -> std::process::ExitCode {
    kazoo_host::reports::run("kazoo-dx", || kazoo_dx::cli(std::env::args().skip(1)))
}
