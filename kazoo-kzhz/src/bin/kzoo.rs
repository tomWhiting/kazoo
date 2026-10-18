//! `kzoo`, from the `kzhz` package: every install gives the same three
//! commands, and it answers as the kzoo crate's `kzoo` does.

fn main() -> std::process::ExitCode {
    kazoo::entry::main(kazoo::entry::kzoo_run_as())
}
