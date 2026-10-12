# KZ-A03b: the shared link setup hands over its parts without a throwaway link, keeps the transport follower, and wastes no sleep

Crate: kazoo-core (`src/host/link.rs`, `src/ipc/link.rs`, `src/engine/mod.rs`), plus any caller whose types change. Depends: A03 (landed) and B01b (run after it lands, since both touch `ipc/link.rs`). Line numbers move; find each item by what it does.

## Outcome

The A03 review left four items, closed here:

1. `into_parts` (`host/link.rs`, used by every `connect_or_direct`) builds a whole throwaway link (rings allocated) only to move fields out of a type with a `Drop`, and drops the `TransportFollower` inside `Linked`. After this lane the parts move out without building a second link (destructure through a private method that takes the fields with `std::mem::replace` on cheap placeholders only if no allocation is involved, or restructure so the `Drop` lives on an inner type and the outer is plain), and the `TransportFollower` reaches the instrument (returned alongside `(HubLink, HubLinkAudio)`, or kept inside whatever the instrument holds) so nothing that A03 used to keep is thrown away. If no instrument used the follower before A03 either, say so under FOUND and remove it from `Linked` instead of passing it on.
2. `open` (`host/link.rs`) is a public pass-through to `hub_link` that only tests call. Remove it; the tests call what production calls (`connect_or_direct` or `hub_link` directly in kazoo-core tests).
3. The kazoo-tui engine (`kazoo-core/src/engine/mod.rs`) still calls `hub_link` directly. Move it onto `connect_or_direct` like every other program, so there is one setup path.
4. Connect with no desk sleeps after the last of its three tries (about 250 ms wasted on the UI thread). Sleep only between tries: two sleeps for three tries. Update the three-tries test to count two sleeps.

## Red tests first

- `host::link::tests::connecting_builds_one_link_not_two`: count ring allocations (or link constructions) through an existing counter or a `#[cfg(test)]` counter; red today (two).
- `host::link::tests::no_desk_sleeps_only_between_tries`: the injected clock records two sleeps; red today (three).
- The tui engine uses the shared setup: a test in `engine` that the engine's link reports the same "no desk" state through the shared path (or, if the engine has no test seam, the reviewer checks it by reading; say which).

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy --workspace --all-targets -- -D warnings
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-core -p kazoo-mix -p kazoo-play -p <each crate changed>
cargo fmt --all --check
ast-grep scan
```

## Definition of done

- [ ] One link built per connect; the transport follower is kept or deliberately removed with the reason.
- [ ] No public `open`; the tui engine uses `connect_or_direct`.
- [ ] Two sleeps for three tries.
- [ ] Red tests then green; no lint suppression; new files under 1000 lines; no existing file split.

## Also from the B01b review (Cassio, 13 Oct 05:50)

5. `kazoo-core/src/ipc/link.rs` near 619: "desk link did not stop within 500 ms" is written into the status of the link being dropped, which nobody can read after the drop (before B01b it went to stderr). Make a link thread left running past shutdown visible: a process-wide count (`kazoo_core::ipc::link::stuck_shutdowns()`) that the wall daemon logs and an instrument shows in its hub badge, with a test that forces a wedged worker.
6. Same file near 469: "desk link completion could not be delivered" has the same reach problem; count it the same way.
7. `kazoo-wall/src/daemon/control.rs` near 800: the once-only refusal log has no test and `told_link_refused` has no doc comment; add both.
8. Only the 808 has a test that renders the "desk link at normal priority" words; add one each for arp, dx, glass, string and the tui header.
