//! kazoo-wall on its own: the same wall `kazoo wall` runs.

fn main() -> color_eyre::Result<()> {
    kazoo_host::reports::install()?;
    kazoo_wall::cli(std::env::args().skip(1))
}
