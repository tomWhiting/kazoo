//! kazoo-juno on its own: the same instrument `kazoo juno` runs.

fn main() -> color_eyre::Result<()> {
    kazoo_host::reports::install()?;
    kazoo_juno::cli(std::env::args().skip(1))
}
