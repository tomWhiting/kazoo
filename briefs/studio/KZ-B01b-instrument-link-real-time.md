# KZ-B01b: an instrument's desk link runs real-time, and says so on its own screen when it cannot

Crates: kazoo-core (`src/ipc/link.rs`, `src/realtime.rs`), every program that calls `hub_link(LinkConfig::new(..))` (grep: kazoo-303, 808, arp, cs80, dx, glass, juno, mini, prophet, play, mix's `source.rs`, and any other), kazoo-wall (`src/daemon/control.rs`). Depends: B01. This lane continues branch lane/kz-B01b, which starts at B01's reviewed head 6597d95: everything B01 and its review did is already there. Line numbers move; find each item by what it does.

## Outcome

B01 wakes the link thread the moment the audio callback queues a block. Its review measured the wake at the priority instruments really run at (ordinary): p50 388 to 488 us at load about 100, against the brief's p50 < 200 us. With the link thread real-time it measured 141 us at the same load. The difference is the thread's priority, not the mechanism. Today only the wall's daemon asks for real-time (`config.real_time = true`), because a refusal is printed with `eprintln!`, which would scribble over an instrument's terminal screen.

Decision (Cassio, 13 Oct 04:35): the desk link carries audio on its way to the desk, so it is an audio thread, and every instrument's link runs real-time. After this lane `LinkConfig::new` sets `real_time: true`; a refusal is never printed to stderr by the link; the program reads `HubLink::real_time()` and shows a refusal where it shows its hub connection (the badge or status line each instrument already draws), and the wall daemon, whose stderr is a log, prints it itself as it effectively does today.

## The design (decided; build exactly this)

- `LinkConfig::new`: `real_time: true`. Doc on the field: on by default; the link thread carries audio to the desk; a refusal is reported through `HubLink::real_time()` and never printed by the link.
- `link.rs` `promote`: remove the `eprintln!` and `TOLD_REFUSED`; store `REFUSED` with the reason. Add `HubLink::real_time_refusal(&self) -> Option<&str>` (or carry the reason in the existing `real_time()` return, e.g. `Option<Result<(), String>>` matching `Hub::real_time()`'s `Option<&Result<(), _>>` shape; pick whichever matches the hub's existing API, and use the same shape for both). The reason is set once, before the first block, never on the audio callback.
- kazoo-wall daemon: where it set `config.real_time = true`, keep it (now the default) and print the refusal itself to its log, once, with the same words the link used to print.
- Each instrument's hub status (find where each draws "hub", "desk", connected or the hub badge; kazoo-mix's `hub_priority(hub.real_time())` at `run.rs` is the model for the desk side): when the link says refused, the status shows "desk link at normal priority" (short form as fits the existing badge). Where an instrument has no hub status on screen, add the words to whatever line already shows the connection state; never print to the terminal outside the drawn UI.
- The wake test (`kazoo-core/src/ipc/link/tests/wake.rs`, `a_queued_block_wakes_the_link_thread_at_once`) uses `LinkConfig::new` unchanged (production's own setting, now real-time) and keeps the p50 < 200 us assertion. If the test machine refuses real-time, the test fails with the refusal reason in its message (it must not silently pass at normal priority).

## Red tests first

- `ipc::link::tests::an_instrument_link_asks_for_real_time_by_default`: `LinkConfig::new(..).real_time` is true; red today.
- `ipc::link::tests::a_refused_promotion_is_reported_not_printed`: drive `promote` through the existing seam with a forced refusal (if `RealTime::promote` has no test seam, add a `#[cfg(test)]` way to force one); `real_time()` reports the refusal with its reason, and nothing is written to stderr (capture with the existing test helpers or assert the code path has no `eprintln!`; a source grep is not proof, so use a seam).
- For one instrument (the 808): `the_hub_badge_says_when_the_link_is_not_real_time`, rendering the status with a refused link and finding the words.
- `a_queued_block_wakes_the_link_thread_at_once` green at production settings; record p50 and p99 with `uptime`.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy --workspace --all-targets -- -D warnings
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-core -p kazoo-mix -p kazoo-wall -p <each instrument crate changed>
cargo fmt --all --check
ast-grep scan
```

## Definition of done

- [ ] Every instrument's link asks for real-time; no link code prints to stderr.
- [ ] A refusal is visible on the instrument's own screen and in the wall daemon's log.
- [ ] The wake test passes at production settings with p50 < 200 us; the red tests above then green.
- [ ] No lint suppression; new files under 1000 lines; no existing file split or moved.
