//! `kzoo`: the kazoo studio in one command.

fn main() -> std::process::ExitCode {
    // npm's `kazoo` and `kzhz` start this same program by their own names.
    kazoo::entry::main(kazoo::entry::kzoo_run_as())
}
