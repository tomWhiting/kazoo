//! kazoo-prophet on its own: the same instrument `kazoo prophet` runs.

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    kazoo_prophet::cli(std::env::args().skip(1))
}
