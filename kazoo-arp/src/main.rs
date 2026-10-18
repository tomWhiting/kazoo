//! kazoo-arp on its own: the same instrument `kazoo arp` runs.

fn main() -> std::process::ExitCode {
    kazoo_host::reports::run("kazoo-arp", || kazoo_arp::cli(std::env::args().skip(1)))
}
