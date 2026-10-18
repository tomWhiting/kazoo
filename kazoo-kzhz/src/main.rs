//! `kzhz`: kazoo by the name it has on npm, so `cargo install kzhz`,
//! `cargo install kzoo` and `npm install -g kzhz` give the same three
//! commands (`kzhz`, `kzoo`, `kazoo`). It is `kzoo`, answering to its own
//! name.

fn main() -> std::process::ExitCode {
    kazoo::entry::main("kzhz")
}
