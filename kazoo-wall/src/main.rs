//! kazoo-wall on its own: the same wall `kazoo wall` runs.

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    kazoo_wall::cli(std::env::args().skip(1))
}
