//! kazoo-cs80 on its own: the same instrument `kazoo cs80` runs.

fn main() -> std::process::ExitCode {
    kazoo_host::reports::run("kazoo-cs80", || kazoo_cs80::cli(std::env::args().skip(1)))
}
