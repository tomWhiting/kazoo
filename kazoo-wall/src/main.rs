//! kazoo-wall on its own: the same wall `kazoo wall` runs.

fn main() -> std::process::ExitCode {
    kazoo_host::reports::run("kazoo-wall", || kazoo_wall::cli(std::env::args().skip(1)))
}
