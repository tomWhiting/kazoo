//! kazoo-string — plucked-string physical-model synthesizer for the terminal.

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    kazoo_string::cli(std::env::args().skip(1))
}
