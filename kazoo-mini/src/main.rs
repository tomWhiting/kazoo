//! kazoo-mini on its own: the same instrument `kazoo mini` runs.

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    kazoo_mini::cli(std::env::args().skip(1))
}
