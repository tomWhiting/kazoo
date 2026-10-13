//! kazoo-string on its own: the same instrument `kazoo string` runs.

fn main() -> color_eyre::Result<()> {
    kazoo_host::reports::install()?;
    kazoo_string::cli(std::env::args().skip(1))
}
