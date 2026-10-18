fn main() -> std::process::ExitCode {
    kazoo_host::reports::run("kazoo-mouth", || kazoo_mouth::cli(std::env::args().skip(1)))
}
