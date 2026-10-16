//! `kzoo`: the kazoo studio in one command.

fn main() -> std::process::ExitCode {
    // npm's `kazoo` runs this same program, and says so (see
    // npm/kzoo/bin/kzoo.js); it answers to that name then.
    let name = match std::env::var("KZOO_RUN_AS").as_deref() {
        Ok("kazoo") => "kazoo",
        Ok(_) | Err(_) => "kzoo",
    };
    kazoo::entry::main(name)
}
