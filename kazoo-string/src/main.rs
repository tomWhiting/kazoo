//! kazoo-string on its own: the same instrument `kazoo string` runs.

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    kazoo_string::cli(std::env::args().skip(1))
}
