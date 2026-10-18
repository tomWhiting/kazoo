//! kazoo-mix on its own: the same desk `kazoo mix` runs.

fn main() -> std::process::ExitCode {
    kazoo_host::reports::run("kazoo-mix", || kazoo_mix::cli(std::env::args().skip(1)))
}
