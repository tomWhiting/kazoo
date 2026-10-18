//! kazoo-808 on its own: the same instrument `kazoo 808` runs.

fn main() -> std::process::ExitCode {
    kazoo_host::reports::run("kazoo-808", || kazoo_808::cli(std::env::args().skip(1)))
}
