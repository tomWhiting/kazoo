//! kazoo-juno on its own: the same instrument `kazoo juno` runs.

fn main() -> std::process::ExitCode {
    kazoo_host::reports::run("kazoo-juno", || kazoo_juno::cli(std::env::args().skip(1)))
}
