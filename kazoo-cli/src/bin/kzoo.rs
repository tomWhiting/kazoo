//! `kzoo`: the kazoo studio in one command.

fn main() -> std::process::ExitCode {
    // npm's `kazoo` and `kzhz` run this same program, and say so (see
    // npm/kzoo/bin/kzoo.js); it answers to that name then.
    let name = match std::env::var("KZOO_RUN_AS").as_deref() {
        Ok("kazoo") => "kazoo",
        Ok("kzhz") => "kzhz",
        Ok(_) | Err(_) => "kzoo",
    };
    kazoo::entry::main(name)
}
