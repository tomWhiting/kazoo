//! kazoo-tui on its own: the same instrument `kazoo tui` runs.

fn main() -> color_eyre::Result<()> {
    kazoo_host::reports::install()?;
    kazoo_tui::cli(std::env::args().skip(1))
}
