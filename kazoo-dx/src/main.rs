//! kazoo-dx — four-operator FM synthesizer for the terminal.

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    kazoo_dx::cli(std::env::args().skip(1))
}
