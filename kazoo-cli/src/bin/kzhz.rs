//! `kzhz`: the kazoo studio by its npm name, so every install (npm's
//! `kzhz`, cargo's `kzoo` or `kzhz`) gives the same three commands.

fn main() -> std::process::ExitCode {
    kazoo::entry::main("kzhz")
}
