# KZ-P01: any channel of any audio interface opens at a small fixed buffer, reaches the audio callback through a ring that never drops out, and says honestly how late it is

Line numbers are from the sweep that wrote this brief, at `efef1b6`, and may have moved. Find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Place in the track: Track P (audio inputs), the first lane. Everything else on the input side (P02, P05, P08) reads audio through what this lane builds.

Written by the Track P drafting seat, 13 Oct 2026 (night). Read by Cassio, 13 Oct night, line by line. Changes: a sample beyond full scale is held to ±1.0 after it is counted (a converter cannot give more; a driver that does is broken, and 1e30 must never reach an effect); `raw_peak` is kept by the writer, the only writer, and read by the reader. Accepted otherwise as written.

Time: about 30 to 40 minutes of a builder's work. Crates: kazoo-core.

Depends: nothing in flight. KZ-A02 (`kazoo_core::host::audio`) is on main. This lane edits `host/audio.rs` and `host/audio_open.rs`; if any lane editing those files is in flight, it must have landed before this lane opens.

Files (7): `kazoo-core/src/host/live.rs` (new), `host/live/ring.rs` (new), `host/live/round_trip.rs` (new), `host/live/tests.rs` (new), `host/mod.rs` (one line), `host/audio.rs` (visibility, and `OpenedOutput::fixed_buffer`), `host/audio_open.rs` (the fallback reports itself).

Why: Tom wants to plug an instrument into his audio interface and hear it through kazoo at "guitar standard ... zero latency", and then process it with effects. Today nothing in the desk opens an input. The only input path (kazoo-tui's engine) averages every channel into one (`engine/mod.rs:417-445`) and zero-pads whenever the input is a sample short (`engine/processing.rs:789-807`): a click every time the two callbacks drift in phase. This lane is the input path done properly, as a library the desk (P02) uses.

## Outcome

`kazoo_core::host::live::open_live_input` opens chosen channels (1-based, as the interface labels them) of an input device at the desk's rate and a fixed buffer of 32, 64, 128 or 256 frames. It returns one `LiveInput` per channel to move into the output callback. Each `read` fills exactly the frames asked for:
- with no dropout when the callbacks wobble (an underrun repeats a few milliseconds of the past behind a 1 ms crossfade, and the ring's slack grows by 16 frames, once);
- with clock drift between two devices pulled back by a crossfaded jump, counted;
- with the software latency in frames reported exactly: the input callback's measured length plus the output buffer plus the slack.

It also counts samples at or above full scale before any gain (the converter's clip), reports when the device refused the fixed buffer, and measures the true round trip through a loopback cable.

## The design (decided; build exactly this)

### Why a read-back ring, not `ringbuf`

An SPSC queue can only hand the reader what has not been read, so an underrun can only be silence. `LiveRing` is a history: the writer writes every sample at `written % LIVE_RING_FRAMES` and publishes `written`, and the reader keeps its own position and may read any frame in `[written − LIVE_RING_FRAMES + GUARD, written)`. An underrun can therefore be bridged with a crossfaded repeat of audio it has already played, a few milliseconds back, which is far less audible than silence. Safe Rust with `unsafe_code = forbid`: the samples are `Box<[AtomicU32]>` holding `f32::to_bits`, stored with `Relaxed`. `written: AtomicU64` is stored with `Release` once per input callback and loaded with `Acquire` once per read. A frame below `written` is never rewritten until `LIVE_RING_FRAMES` later, so a read of it is never torn.

### Constants (`host/live.rs`, each with its reason as its doc comment)

| Name | Value | Reason |
|---|---|---|
| `LIVE_RING_FRAMES` | 8192 | 170 ms at 48 kHz, 43 ms at 192 kHz: more than the largest slack (one 256-frame buffer) plus the drift window plus the crossfade, at every rate, and a power of two for masking |
| `GUARD` | 1024 | the reader never goes within 1024 frames of being overwritten, so a stalled reader is caught as an overrun, not read torn |
| `SLACK_STEP` | 16 | an underrun adds 16 frames (0.33 ms at 48 kHz) of slack: small enough that the latency cost of one bad callback is small, large enough that a machine with real jitter settles within a few steps |
| slack cap | the output buffer `n` | slack beyond one buffer means the machine cannot keep the buffer size; the strip says so instead of hiding it in latency |
| `FADE_MS` | 1.0 | every jump crossfades over 1 ms (48 frames at 48 kHz): shorter than a pick attack, long enough that a jump is not a click |
| `DRIFT_WINDOW_MS` | 250 | the reader jumps forward when its lag has stayed more than half a buffer over target for 250 ms (drift), or at once when it is more than 4 buffers over (a stall or a burst) |
| `CLIP_RAW` | 0.999 | the desk's `CLIP_THRESHOLD` (`kazoo-mix/src/engine.rs:49`): the converter has clipped |
| `MAX_LIVE_CHANNELS` | 8 | one interface's inputs; eight strips are the most anyone will tune by ear at once |

### API (`host/live.rs`)

```rust
/// What the input is, for its ranges and (P06) its tracking. Labels here,
/// behaviour in P06.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputProfile { #[default] Line, Voice, Guitar, Bass }
impl InputProfile {
    pub const ALL: [Self; 4];
    pub fn parse(text: &str) -> Option<Self>;          // "line" | "voice" | "guitar" | "bass"
    pub const fn name(self) -> &'static str;
    /// The fundamental range this input plays in, Hz (low, high).
    pub const fn range_hz(self) -> (f32, f32);
}

pub struct LiveInputRequest {
    pub device: DeviceChoice,
    /// 1-based channel numbers, as printed on the interface; 1..=8 of them,
    /// distinct.
    pub channels: Vec<u16>,
    /// The desk's rate: inputs never resample.
    pub rate: u32,
    /// 32, 64, 128 or 256.
    pub frames: usize,
}

pub fn open_live_input(request: &LiveInputRequest)
    -> crate::Result<(LiveInputStream, Vec<LiveInput>)>;

/// The real ring's two halves without a device: the stream uses this, and so
/// do P02's and P05's tests. Not a fake.
pub fn live_pair(rate: u32, frames: usize) -> (LiveWriter, LiveInput);

impl LiveInputStream {
    pub fn device_name(&self) -> &str;
    pub fn device_channels(&self) -> u16;
    /// False when the device refused the fixed buffer and runs at its own size.
    pub const fn fixed_buffer(&self) -> bool;
    pub fn check_error(&self) -> crate::Result<()>;
}

impl LiveWriter {           // the input callback's half
    pub fn write(&mut self, mono: &[f32]);                       // sanitises, counts raw clips
}

impl LiveInput {            // the output callback's half; Send
    pub fn read(&mut self, out: &mut [f32]) -> ReadReport;      // fills all of `out`
    pub fn latency_frames(&self) -> usize;                       // in callback (measured) + n + slack
    pub const fn slack(&self) -> usize;
    pub fn reset_slack(&mut self);                               // back to 0, at the next jump
    pub fn stats(&self) -> LiveStats;  // captured, underruns, jumps, overruns, nonfinite, raw_clips, raw_peak, in_callback_max
}
```

Profile ranges (`range_hz`), each with its reason in the doc:
- `voice` (50, 4000): Track V's `VOICE_MIN_HZ` and `VOICE_MAX_HZ` (V01).
- `guitar` (60, 1400): a 7-string's B1 is 61.7 Hz and drop C is 65.4 Hz; the 24th fret of the top E is 1318.5 Hz, plus a bend.
- `bass` (29, 500): a 5-string's B0 is 30.87 Hz, and tuned down a semitone it is 29.1 Hz; the 24th fret of the G string is 392 Hz, plus a bend.
- `line` (30, 4000): synths, drum machines and anything else: the widest range the tracker covers.

### The input callback (`open_live_input`)

- Requests the device with `channels = max(request.channels)` through `OutputRequest::prepare_input_with_frames` (now `pub`, `host/audio.rs:76`) at `RateChoice::Hz(rate)`. If the opened stream has fewer channels than the highest asked, it fails with `Error::Device("input device '<name>' has <k> channels; channel <c> was asked for")`.
- The callback (`FnMut(&[f32], &InputInfo)`, interleaved f32 from `InputCallback`) de-interleaves each requested channel into a stack scratch of at most 4096 frames, chunked for longer callbacks, and `write`s it to that channel's `LiveWriter`. It never averages: `make_input_callback`'s averaging (`engine/mod.rs:417-445`) is wrong for an interface and is listed under FOUND, not changed here. It records the largest callback length seen (`in_callback_max`, an atomic).
- `write` sanitises (non-finite becomes 0 and is counted in `nonfinite`), counts `|x| ≥ CLIP_RAW` in `raw_clips`, then holds every sample to `[-1.0, 1.0]`, keeps `raw_peak` (the writer keeps the largest `|x|` before holding, as `f32` bits in an `AtomicU32` it alone stores; the reader only loads it), stores the samples and publishes `written`.

### The fixed buffer that was refused (`host/audio_open.rs:251-261`)

`fall_back_when_fixed_is_refused` keeps its behaviour and returns whether it fell back. `OpenedOutput` gains `fixed: bool` and `pub const fn fixed_buffer(&self) -> bool`. Split the decision into `fn apply_fallback(chosen: &mut Chosen, refused: bool) -> bool` so a test can drive it without a device. `OutputInfo::frames` still says what was asked (changing `OutputInfo` breaks every literal in the workspace); the measured `in_callback_max` is the truth for an input. Also make `prepare_output_with_frames` (`host/audio.rs:56`) `pub`, which P02 needs to open the desk at 64 frames.

### The reader's policy (`host/live/ring.rs`)

`read(out)` with `n = out.len()`, `T = n + slack`, `lag = written − pos`:
1. **Not started:** if `written ≥ T`, start at `pos = written − T` with a 1 ms fade-in; else write silence and return `ReadReport::Priming`.
2. **Underrun** (`lag < n`): `slack = min(slack + SLACK_STEP, n)`, `underruns += 1`, and jump to `pos' = written − (n + slack)`. Crossfade over `F = min(fade, lag)` frames from the old position (valid while it stays below `written`) into the new one. When `lag` is 0, fade the new position in from silence over `fade`. The new position is in the past, so the listener hears a short repeat, not a hole.
3. **Too far behind** (lag over `T + n/2` for `DRIFT_WINDOW_MS` of consecutive reads, or over `T + 4n` at once): jump forward to `written − T` with a 1 ms crossfade; `jumps += 1`.
4. **Overrun** (`lag > LIVE_RING_FRAMES − GUARD`, a reader that stalled for 150 ms): restart as in 1, `overruns += 1`.
5. Otherwise copy `n` frames from `pos`, then `pos += n`.

The crossfade is equal-gain linear (the two positions carry the same signal a few ms apart, so they are correlated). `latency_frames = in_callback_max + T`.

### The round trip (`host/live/round_trip.rs`)

```rust
pub struct RoundTrip { /* state */ }
impl RoundTrip {
    pub fn new(rate: u32) -> Self;
    pub fn play(&mut self, out: &mut [f32]);     // output callback: clicks
    pub fn hear(&mut self, input: &[f32]);       // fed from a LiveInput read
    pub fn result(&self) -> Option<Result<RoundTripResult, RoundTripError>>;
}
pub struct RoundTripResult { pub frames: u32, pub spread: u32, pub ms: f32 }
pub enum RoundTripError { NothingCameBack, TooUneven { spread: u32 }, Clipped }
```

- 8 clicks, 0.5 s apart, after 0.5 s of quiet to measure the noise floor (RMS). Each click is one sample at 0.25 (−12 dBFS): loud enough through a loopback cable, quiet enough that a speaker left plugged in is not hurt.
- For each click, the arrival is the first input sample after it whose magnitude exceeds both half the window's maximum and 10 times the noise RMS, within 0.25 s.
- The result is the median of the 8 arrivals, in output frames from the click's write to its arrival in `read` output. That includes both buffers, the ring and the device. A spread over 1 ms is `TooUneven`, and any click missing is `NothingCameBack`.
- P02 runs it for `kazoo mix --round-trip`.

### Expected figures (stated, not asserted against hardware)

At 48 kHz and 64 frames, the software's share is 1.33 ms in plus 1.33 ms out, plus up to one more buffer when the output callback runs before the input one in a device cycle: 2.7 to 4.0 ms. The converters and CoreAudio's safety offsets add typically 1 to 3 ms on a USB interface. They cannot be read through cpal 0.17.1, whose timestamps use the buffer size alone (`cpal-0.17.1/src/host/coreaudio/macos/device.rs:795-808`). The honest total is therefore about 4 to 7 ms, and the round trip measures it.

## Not this lane

The desk's strips, the CLI and the `--round-trip` command (P02). Gain, gates and inserts (P02, P05). Changing kazoo-tui's engine input (FOUND only). Resampling an input at another rate (inputs open at the desk's rate or fail).

## Red tests first

All in `host/live/tests.rs`, driving `live_pair` with simulated callback schedules (a test clock that interleaves `write` and `read` calls with given lengths, phases and rates). Red for all: the module does not exist.

1. `in_before_out_adds_nothing`: 48 kHz, 64 frames, input then output each cycle for 10 s: zero underruns and jumps, `slack() == 0`, `latency_frames() == 128`, and the output equals the input bit for bit with no delay in the schedule's own frame count. The 128 frames are the real-time figure: one buffer captured before the cycle, one played after it.
2. `out_before_in_costs_one_buffer_and_never_drops`: output first in each cycle: zero underruns after priming, and the output equals the input delayed by 64 frames in the schedule's frame count (the extra buffer).
3. `jitter_grows_the_slack_and_then_holds`: callback times jittered ±30% of a buffer (seeded): underruns happen only in the first second, `slack()` ends ≤ 64 and a multiple of 16, and no output run of 8 or more exact zeros after priming.
4. `a_fast_input_clock_is_pulled_back`: input clock 100 ppm fast for 20 s: `jumps` between 1 and 4, and the lag never exceeds `T + n/2 + 5 frames` for longer than the drift window.
5. `a_slow_input_clock_repeats_instead_of_going_silent`: 100 ppm slow for 20 s: underruns counted, no output run of 8 or more zeros, no sample-to-sample step over 0.06 on a 0.5-amplitude 100 Hz sine (the largest natural step is 0.0066).
6. `a_glide_and_a_vibrato_come_through_without_steps`: a sine gliding a perfect fourth (220 to 293.66 Hz) over 2 s, then a 220 Hz vibrato of ±30 cents at 6 Hz for 2 s, through schedules 4 and 5. The pitch of every 10 ms output window (interpolated zero crossings) is within 3 cents of the input's at the same delayed time, except windows holding a counted jump, which are within 20 cents. No window differs from the last by more than the input's own change plus 2 cents, outside a jump.
7. `the_channel_asked_for_is_the_one_heard`: a 4-channel interleaved callback with a sine on channel 3 and noise on the others, channels `[3]`: the reader's output is channel 3 exactly, not an average.
8. `two_channels_read_independently`: channels `[1, 2]` with different sines: each `LiveInput` gives its own.
9. `raw_clipping_is_counted_before_any_gain`: samples at 0.9995 and −1.0 count 2; 0.998 counts 0; `raw_peak` is 1.0.
10. `nonsense_never_leaves_the_ring`: NaN, ±Inf and 1e30 are written; every output sample is finite and within ±1.0; `nonfinite` counts the first three; 1e30 is counted as a raw clip and read back as 1.0.
11. `a_stalled_reader_is_an_overrun_not_a_torn_read`: the reader skips 200 ms: `overruns == 1` and the next read is a clean restart.
12. `the_callbacks_never_allocate`: `write` and `read` over 10 s of schedule 3 inside `assert_no_alloc`.
13. `a_refused_fixed_buffer_is_reported`: `apply_fallback(&mut chosen, true)` sets `BufferSize::Default` and returns true; with `false` it leaves `Fixed(64)`.
14. `a_channel_beyond_the_device_is_refused_with_its_count`: the channel check (split as a pure function of the opened channel count and the request) gives the message above for channel 3 of a 2-channel device.
15. `the_round_trip_finds_a_known_delay`: `RoundTrip` driven through `live_pair` with a simulated device delay of 317 frames and noise at −60 dBFS: `frames` within ±1 of 317 under schedule 1 (where the ring adds nothing in schedule frames), `spread ≤ 2`. And `the_round_trip_says_when_nothing_comes_back`: no loopback gives `NothingCameBack`.
16. `profiles_parse_and_say_their_ranges`: every name in `ALL` round-trips through `parse`; `bass` is (29, 500).

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-core --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-core --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-core/src/host
```

Doc tests: `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-core -j 4`.

## Definition of done

- [ ] All red tests fail first (paste the failures), then pass. Every existing kazoo-core test passes.
- [ ] Every constant in the table exists with its reason as its doc comment.
- [ ] `rg 'pub fn prepare_output_with_frames|pub fn prepare_input_with_frames' kazoo-core/src/host` shows both public.
- [ ] Not the builder's: after P02 lands, Cassio with Tom runs `kazoo mix --round-trip` through a loopback cable on Tom's interface and writes the figure, its buffer and its rate into BENCH.md under `## KZ-P01 round trip`. The builder never opens a device, lists this under NOT CONFIRMED, and does not claim it.

## Code quality

As PLAN.md's STACK RULES. `LiveWriter::write`, `LiveInput::read`, `RoundTrip::play` and `RoundTrip::hear` run on audio callbacks: no allocation, lock, format, I/O or panic. Every ring index is masked by `LIVE_RING_FRAMES − 1`. Atomics are the only shared state. Frame arithmetic is in `u64` and never wraps in practice: 2^64 frames is 12 million years at 48 kHz, and a doc comment says so. No `as` casts clippy pedantic flags. Files under 1000 lines.

## END

```
START KZ-P01 base=<sha> branch=lane/kz-P01 df=<GB> at <time>
END KZ-P01 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: each crate's nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: none asked
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
