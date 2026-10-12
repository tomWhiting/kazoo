# KZ-G02: tests make the folders they use

Crates: kazoo-arp and kazoo-wall, tests only. Depends: nothing. Line numbers move; find each item by what it does.

## Outcome

On a clean checkout (no `target/kz-logs` folder) every test passes. Today two tests assume `target/kz-logs` exists because the build lanes happen to write logs there, and the main gate on 13 Oct went red on a fresh target:

- `kazoo-arp/src/arp/wire_tests.rs`, `scheduled_notes_reach_the_real_link_with_the_selected_destination`: `canonicalize()` of `../target/kz-logs` fails with NotFound, then binds a socket `w-<pid>.sock` there and never removes it.
- `kazoo-wall/src/tui/tests/mod.rs`, `fold_state_survives_a_restart`: `tempfile::tempdir_in(".../target/kz-logs")` fails when the folder is absent.

## The design (decided; build exactly this)

- Each of the two tests calls `std::fs::create_dir_all` on `concat!(env!("CARGO_MANIFEST_DIR"), "/../target/kz-logs")` with `.expect("test folder")` (or `.unwrap()` matching the file's style) before using it. Nothing goes in the system temp folder.
- The arp wire test then makes its own `tempfile::tempdir_in` of that folder and binds its socket inside it, so the socket is removed when the test ends and two runs never collide. Add `tempfile = "3"` to kazoo-arp's `[dev-dependencies]` (the workspace already uses it in kazoo-wall). Keep the socket path short: the folder plus the tempdir name plus `w.sock` stays well under the 104-byte Unix socket limit; assert that limit in the test with a clear message.
- `kazoo-arp/src/arp.rs` `rig()` names an absent socket `target/kz-logs/absent-...sock` on purpose (the link must find no desk); it needs no folder and stays as it is.
- No `#[allow]`, `#[expect]`, `.ok()`, `let _ =`. No change outside these test files and kazoo-arp's Cargo.toml.

## Red first

Before the change: move `target/kz-logs` aside if it exists (rename it to `target/kz-logs.aside`, put it back at the end), run the two tests and save the failing output to `red-test-folders.txt`. After the change both pass with the folder absent and with it present.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-arp -p kazoo-wall --all-targets -- -D warnings
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-arp -p kazoo-wall
cargo fmt --all --check
ast-grep scan kazoo-arp kazoo-wall
```

## Definition of done

- [ ] Both tests pass with `target/kz-logs` absent and present; red output saved.
- [ ] The arp wire test leaves no socket behind.
- [ ] No other file changed; no lint suppression.
