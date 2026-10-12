//! kazoo-glass on its own: the same instrument `kazoo glass` runs.

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    kazoo_glass::cli(std::env::args().skip(1))
}
