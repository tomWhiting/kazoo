//! kazoo-arp on its own: the same instrument `kazoo arp` runs.

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    kazoo_arp::cli(std::env::args().skip(1))
}
