# KZ-D15: the wall says whether it kept its deadline, so a crackle is never a guess

Line numbers are from the sweep that wrote this brief, at `1400f3c`, and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Read by Cassio, 13 Oct evening. Two changes: the checks take the lane form (cargo lock, four jobs), and test 3 reads `windows` through a named reader. The branch is whatever `lane.sh open` names (`lane/kz-D15`). Since this brief was written, a second cause of crackles was measured: the grimy save overruns the deadline on efficiency cores (KZ-D16a's first section). That makes this lane more useful, not less: its worst and late figures are exactly what tells that case from the simulator one. Base on the latest main; if D16a or D16b has landed and touched `engine/mod.rs`, rebase onto it.

Time: about 40 minutes of a builder's work. One mechanism: the wall measuring and reporting its own audio deadline. Crates: kazoo-wall, kazoo-mcp (the `Rendering:` line in `render.rs`). Depends: nothing in flight. (D05 to D14 are taken; this is the next free D number.)

## Why this lane exists (measured 13 Oct 2026, 16:29 to 16:45, by Cassio)

Tom hears crackles when the wall plays aloud on "MacBook Pro Speakers at 48000 Hz". A 13 s `wall_record` of the same output is clean, so the samples are right and the breakup happens between the render and the speaker. The wall cannot say which side of that line the fault is on today, for three reasons:

- **`Rendering: N%` is not a deadline measure.** It is thread CPU time, not elapsed time (`engine/mod.rs:758-766`, `thread_cpu` at `:732`), folded per callback into an exponential average with a one-second time constant (`measure`, `:774-790`; `LOAD_SECONDS = 1.0`, `:68`). By design it leaves out every wait: preemption, a page fault waiting on disk, a lock. One late callback in 94 moves it by about 1%.
- **The glitch count is always zero on this Mac.** cpal 0.17.1's CoreAudio backend never raises `StreamError::BufferUnderrun`. Only ALSA and JACK do (`cpal-0.17.1/src/host/alsa/mod.rs:810`, `host/jack/stream.rs:468`). So `StreamHealth::glitches` (`daemon/audio.rs:144`, counted at `:428-430`) and the 10-second "audio glitches so far" log line (`daemon/control.rs:155`) can never fire on macOS.
- **Nothing measures a single callback.** There is no worst case, no p99 and no late count. The callback ignores the device timestamps cpal hands it (`audio.rs:287`, `|data, _: &cpal::OutputCallbackInfo|`), so a cycle the device skipped goes unseen.

The measurements below show the wall keeps its deadline with room to spare. The crackles come from outside it.

**The render, offline.** Tom's patch (`~/.kazoo/wall/patch.json`, 85 modules and 121 cables, copied read-only) was loaded through `Store::load_patch` and `Wall::new`, and rendered by the engine in 512-frame callbacks at 48 kHz. The wall ticked every 20 ms, as the daemon does. Load average was 350 to 378, swap was 16.4 of 17.4 GB used, and about 60 MB was free.

| run | callbacks | mean | p50 | p99 | p99.9 | max | over 10.67 ms |
|---|---:|---:|---:|---:|---:|---:|---:|
| paced at real time, thread promoted with `RealTime::promote` (like the CoreAudio IO thread), 60 s: elapsed | 5,625 | 2.546 ms | 2.514 | 2.833 | 2.961 | 3.895 | 0 |
| same run, thread CPU | 5,625 | 2.545 ms | 2.514 | 2.831 | 2.933 | 3.088 | 0 |
| unpaced, normal priority, 20 s: elapsed | 1,875 | 2.062 ms | 1.976 | 2.727 | 9.964 | 57.928 | 1 |
| same run, thread CPU | 1,875 | 1.987 ms | 1.967 | 2.331 | 2.788 | 3.207 | 0 |

The mean CPU share was 18.6% to 23.9%, which matches the live "19%". The worst callback at real-time priority used 36% of its 10.67 ms. CPU time per callback is flat (worst 1.6× the mean), so no module has an occasional expensive block. The paced run's waits on a wake-up were at most 0.17 ms. While it ran, the process took 60 to 80 soft faults a second and no page-ins. The one late block in the unpaced run came from scheduling at normal priority: its CPU time stayed under 3.3 ms.

**The device, from CoreAudio's own log** (`log show --predicate 'process == "coreaudiod"'`, 15:15 to 16:45):

- 7,673 "Audio IO Overload" reports. 7,667 of them are on HAL thread `0x2a291d2`. That thread is the IO work loop that coreaudiod started at 16:13:23 for context 5480, "BuiltInSpeakerDevice (iOSSimulatorAudioDevice-67839-1)". That is the speaker context of the iOS 18.5 Simulator `PHONE-THREADS-2`, booted at 16:12:41, whose `callservicesd` (pid 67839) registered the device at 16:13:22. The first overload came at 16:13:27. Since then there have been about 300 a minute, 4 to 9 a second.
- In the 58 minutes before 16:13 there were 6 overloads, on other threads, with the wall listed among the speaker's active clients. The wall daemon that ran then (started before 12:50) was the same build and was launched the same way.
- CoreAudio's stated cause, every time: "client running as an adaptive unboosted daemon" and "HAL client proc exceeding io cycle budget". A simulated iOS daemon under `launchd_sim` is an adaptive daemon. The wall is not one.
- `io_page_faults_duration` is 0 in all 3,832 analytics records that carry it. `other_page_faults` is 0 in 2,165 of 2,172. So paging plays no part.

**Conclusion.** The wall's render meets its deadline at every percentile, under this machine's pressure. The crackles began at 16:13:27, when the simulator's audio context on the same speakers began overloading. The relief is outside this lane: shut the simulator down or move its audio off the speakers, then count the overloads again. In code, the job is to make the wall say all this itself, so the next crackle takes one look to place rather than an afternoon.

## Outcome

After this lane, `wall_look`'s Rendering line, the console's render badge and the daemon log say three things:
- how long the wall's longest and 99th-percentile callbacks took against their deadline over the last 10 seconds;
- how many callbacks were late (took longer than their own period) in the last minute;
- how many cycles the device itself skipped in the last minute, read from the timestamps CoreAudio gives each callback.

When the device skips cycles while the wall was never late, the line says that the breakup is outside the wall.

## The design (decided; build exactly this)

**1. `engine/deadline.rs` (new, under 400 lines): two allocation-free counters, owned by the engine.**

- `pub struct CallbackTimes`. It holds a histogram `[u32; 129]` of each callback's elapsed time as a share of its own period. There are 128 buckets of 1/64 of the period, from 0 to 2×, and bucket 128 holds everything at 2× or more. It also holds `frames_in_window: u64`, `worst: f32`, and `late: u64` (cumulative).
- `fn observe(&mut self, elapsed: Duration, frames: usize, sample_rate: f32) -> Option<Window>`:
  - share = elapsed ÷ (frames ÷ rate);
  - bump the bucket;
  - raise `worst`;
  - if share > 1.0, `late += 1`;
  - once `frames_in_window` reaches `WINDOW_SECONDS × rate`, return a `Window { worst, p99, callbacks }` and clear the histogram and `worst`.
  - p99 is the upper edge of the bucket where the running count from the top first reaches 1% of the callbacks (rounded up, at least 1). That is a walk over at most 129 entries, once per window.
  - Non-finite or zero periods (zero frames, rate 0) are ignored.
- `pub struct Cadence { last: Option<(cpal::StreamInstant, usize)> }`.
- `fn observe(&mut self, at: cpal::StreamInstant, frames: usize, sample_rate: f32) -> u32` returns the dropouts seen at this callback, 0 or 1:
  - expected = previous frames ÷ rate;
  - gap = `at.duration_since(&last)`;
  - a `None` gap (the clock went backwards) re-anchors and counts nothing;
  - a gap over `SKIP_FACTOR × expected` counts one dropout, however long the gap;
  - then store `(at, frames)`.
  - The first callback counts nothing.
- Constants, each with its one-line reason in its doc:
  - `WINDOW_SECONDS = 10.0`: long enough that a crackle Tom has just heard is still in the window when he looks, and short enough that the numbers clear within seconds of the cause going away. At 512 frames that is about 938 callbacks, so p99 is the 9th worst, not the worst again.
  - `SKIP_FACTOR = 1.5`: halfway between one period and two. The device clock drifts by hundredths of a percent and host stamps jitter by microseconds, so a real skip is about 2.0 and a healthy cycle about 1.0.
  - One dropout per gap, however long: each gap is heard as one break, and a suspended machine is not a thousand crackles.
  - `TIGHT_SHARE = 0.75`: the console turns amber above it. CoreAudio's IO budget leaves the HAL time to mix after the client returns, and on this machine the overloads blamed on a client came from one "exceeding io cycle budget".

**2. The engine measures and publishes (`engine/mod.rs`).**

- `Engine` gains `times: CallbackTimes` and `cadence: Cadence`, both built in `engine()` (`:624`).
- `Engine::render` (`:758`) also reads `Instant::now()` at entry and exit. On macOS that is `mach_absolute_time`, with no allocation and no lock. `measure` (`:774`) hands the elapsed time to `times.observe`. On `Some(window)` it stores `worst` and `p99` as f32 bits into two new `Shared` atomics and bumps a `windows: AtomicU64`. `late` goes to a `Shared` `AtomicU64` every callback.
- New `pub fn device_cycle(&mut self, at: cpal::StreamInstant, frames: usize)`. It runs `cadence.observe` and adds the result to a `Shared` `dropouts: AtomicU64`.
- New `Shared` readers: `callback_worst() -> f32` and `callback_p99() -> f32` (the last finished window; 0 before the first), `late_callbacks() -> u64`, `device_dropouts() -> u64`, and `device_measured() -> bool`. The last is set by the first `device_cycle`, so a headless wall says it has no device rather than reading 0 dropouts as clean.
- The existing CPU `load` and the load guard are unchanged. The guard keeps costing CPU, as it should.

**3. The device callback passes its timestamp (`daemon/audio.rs`).**

- In `build_f32` (`:278-291`) and `build` (`:293-320`), the closure takes `info: &cpal::OutputCallbackInfo`. Before rendering, it calls `engine.device_cycle(info.timestamp().callback, data.len() / channels)` once per callback, not per conversion chunk.
- The `StreamHealth::glitches` doc (`:143`) gains one sentence: CoreAudio never reports underruns through cpal, so on macOS this stays 0 and the device's skipped cycles are counted by the engine (`Shared::device_dropouts`).
- The headless loop does not call `device_cycle`.

**4. A minute watch on the control side (`daemon/deadline.rs`, new, under 300 lines).**

- `pub struct MinuteWatch` holds `{ started: Instant, late_at_start: u64, dropouts_at_start: u64, last: Minute }`.
- `Minute { late: u64, dropouts: u64, worst: f32 }` describes the last full minute.
- `fn step(&mut self, now: Instant, shared: &Shared) -> Option<String>`. Once 60 s have passed, it fills `last` from the deltas and the worst of the windows published in that minute. To read those, it keeps the maximum of `callback_worst()` seen at each step; at 20 ms ticks every window is seen. It returns a log line only when `late > 0 || dropouts > 0`:
  - when late is 0: `kazoo-wall: in the last minute the audio device skipped {dropouts} cycles; the wall's longest callback took {worst}% of its time and none was late, so the breakup is outside the wall`
  - when late is above 0: `kazoo-wall: in the last minute {late} of the wall's callbacks were late (the longest took {worst}% of its time) and the audio device skipped {dropouts} cycles`
  - The control side does not know the device's name, so the lines say "the audio device"; the daemon's start line names it.
- `Wall` gains one field, `minute: MinuteWatch`. `Wall::tick` (`daemon/wall.rs:4383`) calls `step` and `eprintln!`s a returned line. Nothing else goes into `wall.rs`: it is 4,691 lines.
- One minute: Tom asked for once a minute, and a device skipping 300 cycles a minute must not flood the log.

**5. Shown where the load is shown.**

- `EngineHealth` (`protocol/mod.rs:1218`) gains these fields, each `#[serde(default)]` so older consoles and seats still read the snapshot:
  - `callback_worst: f64` and `callback_p99: f64` (shares of the deadline, last 10 s);
  - `deadline_ms: f64` (the period of the last callback; 10.67 at 512 frames and 48 kHz);
  - `late_last_minute: u64` and `dropouts_last_minute: u64`;
  - `device_measured: bool`.
  - The snapshot fills them at `wall.rs:3963`, from `Shared` and `self.minute.last`.
- **MCP** (`kazoo-mcp/src/render.rs:180-183`, `engine_line`): the line becomes `Rendering: {load}% of the time the wall has; callbacks took at most {worst}% of their {deadline} ms (p99 {p99}%) over the last 10 s; {device}; {room}.`
  - `{device}`, when `device_measured` is true and the minute is clean: `no callback late and no cycle skipped in the last minute`.
  - With dropouts and no late: `the audio device skipped {n} cycles in the last minute while no callback was late: the breakup is outside the wall`.
  - With late callbacks: `{late} callbacks were late and the device skipped {n} cycles in the last minute`.
  - Headless: `no audio device (the desk or nothing plays it)`.
- **Console** (`tui/draw/panes.rs:154`, `render_load`): `render 19% · worst 36%`.
  - It turns amber when the worst is above `TIGHT_SHARE`, or when the device skipped cycles in the last minute; ` · {n} skips/min` is then added.
  - It turns red with ` · {late} late` when any callback was late in the last minute.
  - This keeps today's amber, full and measuring rules, and red wins.

**Why visibility and nothing else. Each candidate fix was weighed against the measurements:**
- **Locking the engine's memory (mlock):** no. CoreAudio recorded zero IO page-fault time on every overload, the probe took no page-ins, and `rustix::mm::mlock` is `unsafe`, which the workspace forbids. It would need a new dependency for no measured gain.
- **Pre-faulting buffers:** no, for the same evidence.
- **A larger device buffer:** no. The wall's worst callback uses 36% of a 512-frame cycle. The other clients on the speakers (CoreSpeech, dot-seat) ask for 512 too. The overloads are in another client's context, so a larger buffer here would add 10.7 ms of latency and leave them where they are.
- **A render-ahead thread feeding a ring:** no. The render is never late. CLAUDE.md makes the output callback the processing engine. The ring would add a buffer of latency and a second real-time thread to a machine already at load 350.

## Not this lane

- No change to the load guard, `LOAD_SECONDS` or the CPU measure.
- No CoreAudio property listener (`kAudioDeviceProcessorOverload` needs `unsafe` or a new binding).
- No change to the desk's own counts or the headless loop's pacing.
- Nothing that touches the iOS Simulator: that is the operator's to shut down.

## Red tests first

1. `engine::deadline::tests::a_skipped_cycle_is_one_dropout`: callbacks at 0, 10.667, 21.333 and 42.667 ms (512 frames, 48 kHz) give 0, 0, 0, 1. A gap of 5 s gives 1. A timestamp earlier than the last gives 0 and re-anchors. The first callback gives 0. Red: no `Cadence`.
2. `engine::deadline::tests::a_window_reports_its_worst_and_p99`: 1,000 callbacks of 512 frames. 990 take 20% of the period, 9 take 50% and 1 takes 120%. The returned window has worst 1.2, p99 within 1/64 above 0.5, and `late == 1`. Exactly one window is returned per 10 s of frames. Red: no `CallbackTimes`.
3. `engine::tests::render_publishes_callback_times_without_allocating`: render 11 s of 512-frame callbacks under `assert_no_alloc`. Then `callback_worst() > 0`, `callback_p99() <= callback_worst()`, and `callback_windows() == 1` (a `Shared` reader of the `windows` counter, added with the others). Red: no readers.
4. `engine::tests::device_cycles_count_dropouts`: `device_cycle` with one doubled gap gives `device_dropouts() == 1` and `device_measured()`. A fresh engine gives `!device_measured()`. Red: no method.
5. `daemon::deadline::tests::the_minute_says_whose_fault_it_is`, driven with a fake `Shared` (built through `engine()`) and set counters:
   - 287 dropouts and 0 late give the "outside the wall" line;
   - 3 late give the "late" line;
   - nothing gives `None`;
   - before 60 s it gives `None`.
   Red: no `MinuteWatch`.
6. `kazoo-mcp` `render::tests::the_rendering_line_says_the_deadline`: covers the four `{device}` cases above, and an old snapshot without the new fields still renders. Red: no fields.
7. `tui::draw::panes::tests::the_render_badge_says_skips_and_late`: checks the amber, skips and red late cases. Red: no fields.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-wall -p kazoo-mcp --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-wall -p kazoo-mcp --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-wall kazoo-mcp
```

## Definition of done

- [ ] Tests 1 to 7 fail first, then pass. All existing tests pass.
- [ ] On a release build with a 512-frame device, the console shows `render N% · worst M%` and `wall_look`'s Rendering line carries the deadline clause. Paste both. A builder without a device pastes the headless form and says so under NOT CONFIRMED.
- [ ] `StreamHealth::glitches` is documented as zero on CoreAudio.
- [ ] `BENCH.md` gains "live patch callback deadline, 13 Oct 16:3x", with the table and the CoreAudio counts from this brief's first section, copied as they are.

## Code quality

Tree rules. Every counter is a fixed array or an atomic. Nothing allocates, locks, formats or does I/O on the callback; the log line is formatted on the control side. `Instant::now()` is the only new call on the callback. Unsafe is forbidden. NaN and Inf are defended: a non-finite share is dropped, not binned. Some files are already over 1,000 lines: `engine/mod.rs` (1,513), `daemon/wall.rs` (4,691), `daemon/control.rs` (1,102), `protocol/mod.rs` (1,861) and `kazoo-mcp/src/render.rs` (1,935). Add to them only the lines named here, put the logic in the two new files, and list each of these files under FOUND with its line count. One behaviour per commit.

## END

```
START KZ-D15 base=<sha> branch=lane/kz-d15 df=<GB> at <time>
END KZ-D15 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: the BENCH.md section added, as written
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
