//! kazoo-cs80 on its own: the same instrument `kazoo cs80` runs.

fn main() -> color_eyre::Result<()> {
    kazoo_host::reports::install()?;
    kazoo_cs80::cli(std::env::args().skip(1))
}
