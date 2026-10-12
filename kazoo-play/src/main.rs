//! kazoo-play on its own: the same tool `kazoo play` runs.

fn main() -> std::process::ExitCode {
    kazoo_play::cli(std::env::args().skip(1))
}
