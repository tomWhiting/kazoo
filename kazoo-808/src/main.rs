//! kazoo-808 on its own: the same instrument `kazoo 808` runs.

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    kazoo_808::cli(std::env::args().skip(1))
}
