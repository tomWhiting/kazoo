//! kazoo-string on its own: the same instrument `kazoo string` runs.

fn main() -> std::process::ExitCode {
    kazoo_host::reports::run("kazoo-string", || {
        kazoo_string::cli(std::env::args().skip(1))
    })
}
