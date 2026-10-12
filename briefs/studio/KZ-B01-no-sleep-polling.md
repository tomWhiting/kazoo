# KZ-B01: audio reaches the desk when it is rendered, not when a sleep ends

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename.

## Outcome

Today two threads sleep 500 µs and look again, two thousand times a second each, whether or not anything happened: the instrument's link thread (`kazoo-core/src/ipc/link.rs:69` `POLL_INTERVAL`, slept at `:793`) and the desk's hub thread (`kazoo-mix/src/hub.rs:66` `POLL_INTERVAL`, slept at `:531`). When this lane is done, the link thread is woken by the audio callback the moment a block is queued (`Thread::unpark`), and the hub thread sleeps in `poll(2)` on its sockets and wakes the moment bytes arrive. Nothing waits for a timer to run out before carrying audio. A test measures it: the wake latency from `send_audio` to the frame on the hub's socket, p50 and p99 over 10,000 blocks, and the same from socket to the hub's count, both reported in BENCH.

RETRY NOTE for KZ-B01 (Cassio, 13 Oct 04:05). Two earlier runs are on branch lane/kz-B01 (692155f, 81af833): the hub side has 5 of the 6 new tests green and the instrument wake measured p50 about 101 us at load about 100. Main has moved a long way since their base b6a1fea (A01 targets, A02 shared host, B07 arp, G01, E01): merge origin/main into the lane first and resolve conflicts in kazoo-core/src/ipc and kazoo-mix/src/hub* by keeping both sides' intent. Then finish what remains, run the six measuring and behaviour tests alone first, and record p50 and p99 with the load average next to each. A p50 assertion that fails only because the machine is loaded above 150 is recorded under FOUND with the load, not chased; the reviewer judges it on a quiet machine. The kazoo-mix jittery-timer hub test (`a_paced_source_with_a_jittery_timer_is_placed_once_and_stays_placed`) fails on main at load 240; say whether this lane changes its result.

## The design (decided; build exactly this)

### Instrument side: `Thread::unpark` from the callback

- `hub_link` (`link.rs:409–473`) spawns the link thread at `:452–454` and builds `HubLinkAudio` after it (`:462–471`). Give `HubLinkAudio` a new field `waker: std::thread::Thread`, set to `join.thread().clone()` before the struct is built. `Thread` is `Clone + Send + Sync`; no new `Arc`.
- `HubLinkAudio::push` (`:645–662`): after a successful `push_block`, call `self.waker.unpark()`. On a failed push (dropped block) do not wake. `HubLinkAudio::request` (`:726–733`): after a successful `try_push`, `self.waker.unpark()`.
- Why `unpark` is allowed on the audio callback, and nothing else is: it is one atomic swap on the parker's state and, only when the link thread is actually parked, one wake system call (a futex wake on Linux; `os_sync_wake_by_address` on macOS 14.4 and later, otherwise a condvar signal whose mutex the parked thread holds only around its own wait). It is constant time, it never waits for the woken thread, and it never blocks on anything the audio thread would have to wait for. Nothing else on the callback may take a syscall; the existing rules (no allocation, no lock, no I/O) stand.
- `HubLink::request` (`:568–578`, the UI half): after a successful `try_send`, `self.join.as_ref().map(JoinHandle::thread).map(Thread::unpark)` written as an `if let`, so a UI request is carried at once instead of at the next tick.
- `LinkWorker::run` (`:767–797`): replace both sleeps.
  - Not connected (`:785`): `thread::park_timeout(IDLE_INTERVAL)` (20 ms, `:72`). A stray unpark here costs one early connect attempt check; harmless.
  - Connected (`:793`): `thread::park_timeout(LINK_TICK)`, new constant `pub const LINK_TICK: Duration = Duration::from_millis(5)` replacing `POLL_INTERVAL`. The tick is the bound on how late the thread reads the socket when no audio is flowing (hub messages, pace, health, shutdown). Five milliseconds is `PACE_INTERVAL` (`kazoo-mix/src/hub/instrument.rs:29`), so a paced instrument's pace is read as often as it is sent. Every `send_audio` wakes the thread earlier than the tick.
  - A `park_timeout` returning spuriously or early is fine: `serve` is idempotent.
- `promote` (`:800–821`): `RealTime::promote(LINK_TICK)`; the period the scheduler is told is now the tick.
- `say_goodbye` (`:1078–1097`): the 100 ms flush loop sleeps `POLL_INTERVAL` at `:1094`; replace with `thread::park_timeout(Duration::from_millis(1))`. Nothing else in core changes.
- `serve` (`:926–1011`) is unchanged in logic; it already drains everything that is there.
- Module doc at `link.rs:1–28`: say the link thread is woken by the callback and ticks at `LINK_TICK` otherwise.

### Hub side: `poll(2)` through rustix

- The lead's note named the `polling` crate. Its `Poller::add` has been `unsafe fn` since polling 3.0 (the caller guarantees the fd outlives its registration), which `unsafe_code = forbid` refuses. `rustix::event::poll` is the same `poll(2)`, its API is safe, and rustix 1.1.3 is already in `Cargo.lock` (kazoo-wall uses it, `kazoo-wall/Cargo.toml:24`). Decided: rustix. `kazoo-mix/Cargo.toml` adds `rustix = { version = "1.1.3", features = ["event"] }`. Read `poll`'s signature in the lock's version before writing it (rustix 1.x takes the timeout as `Option<&Timespec>`).
- `Server::run` (`hub.rs:528–534`) becomes `while running { self.poll_once(); self.wait(); }`. New `fn wait(&self)`:
  - A fixed array `let mut fds: [PollFd<'_>; MAX_POLLED] = std::array::from_fn(|_| PollFd::new(&self.listener, PollFlags::IN));` with `const MAX_POLLED: usize = 1 + DESK_CHANNELS + 2 * DESK_CHANNELS` (the listener, every instrument, up to sixteen registering connections). No `Vec`: the hub thread is real-time and "a poll allocates nothing" (`hub.rs:22`) stays true.
  - Fill: index 0 the listener; then each `self.instruments[i].stream` with `PollFlags::IN`; then `self.pending[..]` streams with `PollFlags::IN` up to the array's end (a seventeenth registering connection waits for the tick; registration has a two second deadline, `:69`). `let n = ...; rustix::event::poll(&mut fds[..n], Some(&timeout))`.
  - `timeout` is `const HUB_TICK: Duration = Duration::from_millis(2)`, replacing `POLL_INTERVAL`. The tick bounds how late the hub sees what has no fd: patchbay answers and song anchors from the callback (`patchbay.rs:135–143`), the desk's shared transport, and pace and health due (`instrument.rs:29`, `:36`). Two milliseconds is a tenth of `SCHEDULE_SLACK_SECONDS` (`studio_clock.rs:25`), so a transport anchor is forwarded well inside the slack that covers it; it is four times fewer wakes than today when idle, and audio is served on readiness, never on the tick.
  - `Err(Errno::INTR)` is a return (poll again next loop); any other error is `self.notify(HubNotice::Fault(...))` once and a `thread::park_timeout(HUB_TICK)` so a broken poll never spins. `Ok(_)` returns: `poll_once` reads everything that is ready.
  - `write`-side readiness is not polled: `flush_instruments` (`:921–929`) runs every pass and the outbox keeps what the socket refused; a socket that stays full is `OutboxError::Stuck` (`outbox.rs:13`), unchanged.
- `Hub::start` (`:362`): `RealTime::promote(HUB_TICK)`.
- `shut_down` (`:1004–1038`): the goodbye flush loop sleeps `POLL_INTERVAL` at `:1022`; replace with `thread::park_timeout(Duration::from_millis(1))`.
- `Pending` (`:446–452`) and `Instrument` (`instrument.rs:118–152`) already own their `UnixStream`s; `PollFd::new` borrows `&UnixStream` (`AsFd`). Nothing else in `instrument.rs` changes.
- Module doc `hub.rs:16–23`: "Everything here runs on one thread that sleeps in `poll(2)` on its sockets and wakes when bytes arrive, or every `HUB_TICK` for what has no socket."

### Not changed

The wire format, the rings, the outbox, the strip path, the stamp. B02 changes the copies and the limits; this lane only changes when the threads run.

## Not this lane

- Fewer copies, the 1 MiB frame, time-bounded backlog and stuck detection: KZ-B02.
- Named streams: KZ-B03a/b. Resampling: KZ-B05.
- The wall's headless render thread and its `BURST_BLOCKS` nap (`kazoo-wall/src/daemon/audio.rs:50`): not touched.
- The desk's UI thread (30 fps) and the demo 808 thread: not touched.

## Red tests first

Behavioural only: two ends of a real socket in every test.

1. `kazoo-core/src/ipc/link.rs` tests (the module's `accept`, `socket`, `wait` helpers at `:1135–1180` already exist):
   - `a_queued_block_wakes_the_link_thread_at_once`: link connected to the stand-in hub; push one 256-frame block and read one frame on the hub end; repeat 10,000 times recording `Instant` before `send_audio` and after `read_frame`. Sort; assert p50 < 200 µs (today's floor is a uniform 0–500 µs sleep, p50 about 250 µs, so this is red now); print `p50` and `p99` in microseconds with `eprintln!` for BENCH (p99 is reported, not asserted: it is the machine's load).
   - `a_ui_request_is_carried_without_waiting_for_the_tick`: connected link; `link.request_tempo(100.0)`; the hub end's `read_frame` returns a `MSG_TRANSPORT_REQUEST` within 2 ms of the call (assert elapsed < `LINK_TICK`, which the old 500 µs poll also passes: this test guards the unpark in `HubLink::request`, so make it strict: 10 requests, every one under 1 ms).
   - `an_idle_link_still_reads_the_hubs_messages_within_a_tick`: connected link, no audio; the hub end writes a `MSG_TRANSPORT_SYNC`; `audio.try_recv()` has it within `2 * LINK_TICK` (fails if the tick were missing and the thread parked forever).
2. `kazoo-mix/src/hub/tests.rs` (the `Rig` at `:27–97`):
   - `an_audio_message_is_taken_the_moment_it_arrives`: connect a client; send 10,000 audio messages of `frame_count = 0` (12-byte payload, counted in `stats.blocks` at `instrument.rs:232–234` without touching the strip ring); after each, spin on `rig.hub.snapshot().blocks` until it increments, recording the elapsed time; assert p50 < 200 µs; print p50 and p99 for BENCH.
   - `a_transport_anchor_still_reaches_an_instrument_within_the_tick_budget`: as `tempo_and_play_state_reach_every_instrument_both_ways` (`:283–330`) but asserting the sync arrives within 50 ms of `set_playing(true)` plus one `render()`; proves the tick serves the patchbay.
   - `a_registering_connection_beyond_the_poll_set_is_still_registered`: open 20 connections that send nothing, then a 21st that registers; it is `Joined` within 3 s (the tick serves the overflow).

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-core -p kazoo-mix
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-core -p kazoo-mix --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-core/src/ipc kazoo-mix/src
```

Doc tests only if doc comments changed: `nice -n 10 env CARGO_BUILD_JOBS=4 cargo test --doc -p kazoo-core -p kazoo-mix`.

## Definition of done

- [ ] No `thread::sleep` remains in `link.rs` or `hub.rs` (grep is the builder's check; the tests are the proof).
- [ ] `POLL_INTERVAL` is gone from both files; `LINK_TICK` (5 ms) and `HUB_TICK` (2 ms) exist with the reasons above in their doc comments.
- [ ] `HubLinkAudio::push` and `request` unpark; `HubLink::request` unparks.
- [ ] The hub waits in `rustix::event::poll` over a fixed array; no allocation in `wait`.
- [ ] All six red tests red first (paste their failing output), then green.
- [ ] BENCH: instrument-side p50/p99 and hub-side p50/p99 in microseconds over 10,000 blocks, from the two measuring tests, before (stash the change, run once) and after.
- [ ] Every existing test in both crates still green.

## Code quality

The tree's rules: `unsafe_code` forbidden (so no mmap, no shared memory, no `polling` crate); no `#[allow]`/`#[expect]`; no `let _ =`; no `.ok()` discards; no empty `Err` arms; no `if let Ok` without `else`; files under 1000 lines (`link.rs` is 1645 and `hub.rs` 1143 already: do not grow them by more than the lines this lane needs; put `wait` and `MAX_POLLED` in `hub.rs` next to `run`); no TODO, no stubs. On the audio callback nothing allocates, locks or makes a system call except `Thread::unpark`, for the reason written above; say it in the doc comment of `push`. Every audio path still survives NaN and Inf (untouched here). Red first, then green, one behaviour per commit.

## END

```
START KZ-B01 base=<sha> branch=lane/kz-b01 df=<GB> at <time>
END KZ-B01 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: before and after numbers where the brief asks for them
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
