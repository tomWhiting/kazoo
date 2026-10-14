fn main() -> color_eyre::Result<()> {
    kazoo_host::reports::install()?;
    kazoo_mouth::cli(std::env::args().skip(1))
}
