//! kazoo-303 on its own: the same instrument `kazoo 303` runs.

fn main() -> color_eyre::Result<()> {
    kazoo_host::reports::install()?;
    kazoo_303::cli(std::env::args().skip(1))
}
