# KZ-A03: one hub link setup for every instrument

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md with `-p kazoo-core` plus every crate touched.

## Outcome

Every instrument connects to the desk the same way, through one helper, and the copy of the link open, the audio-callback drain and the teardown that each `main.rs` carries today is gone.

## The design (decided; build exactly this)

- `kazoo-core/src/host/link.rs`:
  - `pub struct LinkSetup { pub name: &'static str, pub channels: u16, pub rate: u32, pub block: usize }`.
  - `pub fn connect(setup: &LinkSetup) -> Result<Linked, Error>` wraps `kazoo_core::ipc::link::hub_link` (`kazoo-core/src/ipc/link.rs:409`): it opens the link, retries the socket three times 250 ms apart (the desk may be starting), and returns `Linked { tx: AudioTx, events: EventRx, transport: TransportFollower }`. If no desk answers it returns `Error::NoDesk`, and the caller decides (instruments print "no desk; playing direct" and open the device through KZ-A02).
  - `pub fn drain_events(rx: &mut EventRx, mut on: impl FnMut(HubEvent))`: the `try_recv` loop every instrument's callback carries, written once, non-blocking, bounded to 64 events per call so a callback cannot spin.
  - `impl Drop for Linked` sends `Shutdown` and joins the link thread with a 500 ms bound.
- Replace the per-crate copies in each instrument's `main.rs` / `audio.rs` (808, 303, arp, cs80, mini, juno, prophet, dx, string, glass, play `--jam`, wall `control.rs:432`). The wall keeps its own link shape only where it differs (its stream count changes in KZ-B04); it calls `connect` for the open and `drain_events` for the drain.

## Not this lane

Named streams (KZ-B03), ParameterChange handling (KZ-B06), device opening (KZ-A02).

## Red tests first

- `host::link::tests::connect_with_no_desk_is_no_desk_after_three_tries` (a socket path in a tempdir nobody listens on; a fake clock counts the three sleeps).
- `host::link::tests::drain_events_takes_at_most_sixty_four_per_call`.
- `host::link::tests::drop_sends_shutdown` (a test hub end on a real Unix socket reads the Shutdown frame).

## Definition of done

- [ ] `grep -rn "hub_link(" --include=*.rs` outside kazoo-core returns nothing.
- [ ] Three red tests then green; every converted crate green.
- [ ] One instrument started against a running desk still shows on a strip (`kazoo-mix` in `--demo` is not enough: start the real desk, start kazoo-808, read the strip name from the desk's log).
