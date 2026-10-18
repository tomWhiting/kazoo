//! kazoo-prophet on its own: the same instrument `kazoo prophet` runs.

fn main() -> std::process::ExitCode {
    kazoo_host::reports::run("kazoo-prophet", || {
        kazoo_prophet::cli(std::env::args().skip(1))
    })
}
