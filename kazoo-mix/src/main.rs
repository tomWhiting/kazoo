//! kazoo-mix on its own: the same desk `kazoo mix` runs.

fn main() -> color_eyre::Result<()> {
    kazoo_host::reports::install()?;
    kazoo_mix::cli(std::env::args().skip(1))
}
