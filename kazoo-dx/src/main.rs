//! kazoo-dx on its own: the same instrument `kazoo dx` runs.

fn main() -> color_eyre::Result<()> {
    kazoo_host::reports::install()?;
    kazoo_dx::cli(std::env::args().skip(1))
}
