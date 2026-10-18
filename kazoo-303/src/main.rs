//! kazoo-303 on its own: the same instrument `kazoo 303` runs.

fn main() -> std::process::ExitCode {
    kazoo_host::reports::run("kazoo-303", || kazoo_303::cli(std::env::args().skip(1)))
}
