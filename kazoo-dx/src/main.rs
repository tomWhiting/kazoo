//! kazoo-dx on its own: the same instrument `kazoo dx` runs.

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    kazoo_dx::cli(std::env::args().skip(1))
}
