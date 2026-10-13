# KZ-V11: any input at any rate: the mic opens at its own rate and width, is resampled into the engine on the input side, follows a lost or changed device, and can be chosen by name

Line numbers are from `main` at `508b869` and may have moved. Find each item by what it does, and never rename anything this brief does not rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Place: Track V, after V06 (merged). Drafted 14 Oct 2026, 09:4x, for Cassio to check and commit. Every number below is decided under Tom's rule that internal calls are not parked on him.

Time: about 40 minutes. Crates: kazoo-core, kazoo-tui, kazoo-cli.

Depends: KZ-P01 (merged; this lane reuses nothing of its ring, see Not this lane), KZ-V06 (merged: the judge thread and the tracker are fed from the engine's mic block). May run beside KZ-V12: both edit `kazoo-core/src/engine/mod.rs`, this lane in `start`, `hold_streams`, `make_input_callback` and `RingBufferCapacities`, V12 in `EngineConfig::default`, `judge_config` and the judge tests, so the merge is line by line. Must land before KZ-V08a opens: V08a's audio bullet ("open_input at RateChoice::Hz(<output rate>) ... fails start") must then use `kazoo_core::io::mic` instead (Cassio amends V08a; see the end of this brief).

Files (12, over the ten-file guide: `io/mod.rs` is one `pub mod` line and one re-export line, and `kazoo-cli/src/cli/tests.rs` and `kazoo-cli/src/studio.rs` change only `Command::Mouth` to `Command::Mouth(_)`; say so under CHANGED):
`kazoo-core/src/io/mic.rs` (new), `kazoo-core/src/io/mic/tests.rs` (new), `kazoo-core/src/io/mod.rs`, `kazoo-core/src/io/device.rs`, `kazoo-core/src/engine/mod.rs`, `kazoo-core/src/engine/stats.rs`, `kazoo-tui/src/run.rs`, `kazoo-tui/src/ui/header.rs`, `kazoo-cli/src/cli.rs`, `kazoo-cli/src/main.rs`, `kazoo-cli/src/cli/tests.rs`, `kazoo-cli/src/studio.rs`.

## Why

Tom, 14 Oct 2026, 09:0x to 09:2x, after his first hum of `kazoo mouth` (AirPods out, Mac mic in): "it's actually really, really quite good." Then: "we need to be flexible on input ... as soon as I plug in my audio interface, we're going to be getting like ... at least 96 kilohertz." With the AirPods mic as the default input, `kazoo mouth` refused to start:

```
Device error: input device 'default' at 48000 Hz: no supported f32, i16 or u16 configuration for 1 channels at 48000 Hz; device offers channel counts [1]
```

The AirPods mic runs at 24 kHz. The engine opens the input at the output's rate: `build_streams_with_output` builds the input request with `rate: RateChoice::Hz(sample_rate)` (`kazoo-core/src/io/device.rs:276-283`), `configure` finds no range holding 48 kHz and says so (`kazoo-core/src/host/audio.rs:331`), and `kazoo-tui/src/run.rs:85` passes the error out before the screen opens. Asking cpal for a rate also makes CoreAudio switch the device's nominal rate where it can (`cpal-0.17.1/src/host/coreaudio/macos/device.rs:51-200`, `set_sample_rate`), which changes the rate under every other program using that mic.

Three more faults sit on the same path:
- An input stream error (an unplugged mic) is fatal: `hold_streams` puts the input's `StreamHealth` beside the output's (`engine/mod.rs:361`), and kazoo-tui's loop returns the first error (`kazoo-tui/src/app.rs:451`), so pulling the AirPods out ends the program.
- The mic cannot be chosen: `StreamConfig::input_device` exists (`io/device.rs:27`) but nothing sets it, and `kazoo mouth` takes no arguments (`kazoo-cli/src/cli.rs:59-60`, `kazoo-tui/src/run.rs:24-37`).
- Nothing on screen says which mic is in use or at what rate.

## Outcome

`kazoo mouth` starts with any input device at the device's own rate, from 8 kHz to 192 kHz, and at its own channel count. When the mic's rate differs from the engine's, the input callback resamples it into the engine's rate before it reaches the mic ring, so the output callback, the cycle tracker and the judge see exactly what they see today: engine-rate mono. When the rates are equal nothing is resampled and the samples pass bit for bit. A mic that is unplugged, changes rate, or (with no device named) is replaced by a new system default is reopened by a supervisor thread without stopping the engine. `kazoo mouth --input "<name>"` chooses the device, `--input-channel <n>` one of its channels, and `--list-inputs` prints the names to choose from. The header says which mic, at what rate, and whether it is lost.

## The design (decided; build exactly this)

### Decisions, with reasons

- **The device's own rate, never a requested one.** The mic opens with `RateChoice::Native` (the rate `default_input_config()` reports, `host/audio.rs:352-358`). Reason: asking for another rate either fails (the AirPods) or retunes the device for every other program; the device's own rate always opens.
- **Range 8,000 to 192,000 Hz.** Outside it the mic is refused in words. Reason: the same range B05 gives the desk; below 8 kHz a voice's harmonics are gone, and above 192 kHz no interface Tom owns runs.
- **Resample on the input callback, not the output callback, and not on a thread.** Reason: the output callback is the engine (CLAUDE.md rule 1) and must not carry a filter whose cost depends on the mic; a thread would add a wake (50 to 200 µs, and a whole output callback whenever it misses one) to every block of a path whose whole budget at 2 kHz is 2 ms. The input callback is already real-time and already does per-sample work (`make_input_callback`, `engine/mod.rs:421-449`).
- **The tracker and the judge see the engine-rate signal.** Nothing in V03, V04 or V06 changes: the judge is built at the output's rate (`engine/mod.rs:203-207`), the tracker at the engine's rate inside `ProcessingState`, and both read `state.mic_block`, which the output callback fills from the mic ring at the engine's rate. Reason: one clock for the tracker, the judge, the synths, the desk link and the voice frame's `at`; a second rate would need a second tracker configuration and a conversion of every frame time. What this costs: an upsampled mic (24 kHz into 48 kHz) has nothing above 12 kHz, which the voice path never uses (`VOICE_MAX_HZ` is 4 kHz); a downsampled one (96 kHz into 48 kHz) loses only what the engine could never play.
- **The resampler: rubato's asynchronous sinc resampler, mono, fixed input chunk.** `rubato::Async::<f32>::new_sinc(engine_rate / mic_rate, 1.0, &params, chunk, 1, FixedAsync::Input)` with `SincInterpolationParameters { sinc_len: 64, f_cutoff: rubato::calculate_cutoff(64, WindowFunction::BlackmanHarris2), oversampling_factor: 256, interpolation: SincInterpolationType::Cubic, window: WindowFunction::BlackmanHarris2 }` (all in `rubato-1.0.1/src/asynchro.rs:242`, `asynchro_sinc.rs:23-80`, `windows.rs:88`). Reasons: it takes any ratio (44.1 to 48, 24 to 48, 96 to 48, 11.025 to 96) with one code path; `process_into_buffer` copies into its own preallocated buffer and allocates nothing (`asynchro.rs:448-538`; rubato's `log` feature is off, so its `trace!` compiles away, `lib.rs:13-17`); 64 taps put the filter's delay at 32 mic samples (1.3 ms at 24 kHz, 0.33 ms at 96 kHz), and the filter is built at the lower of the two Nyquists, so a 96 kHz mic is anti-aliased on the way down. The FFT resampler (`Fft`, used offline at `io/resample.rs:60-68` and by B05's design) was turned down: its delay is a whole FFT chunk, several milliseconds at small chunks. Polynomial `Async::new_poly` was turned down: it has no anti-aliasing filter, so a 96 kHz interface would alias into the voice band.
- **Chunk:** `chunk = clamp(round(mic_rate / 1000), 32, 256)` mic frames: 1 ms at 32 kHz and above (44 at 44.1 kHz, 48, 96, 192), 32 frames below (4 ms at 8 kHz, 1.3 ms at 24 kHz). Reason: the chunk is latency the resampler adds on top of its filter; 1 ms is under the tracker's whole budget for a whistle, and 32 frames keeps each `process_into_buffer` call well over the filter's own length.
- **Channels:** the mic opens at the device's own channel count (`default_input_config().channels()`). With no channel named, the samples reaching the engine are the mean of every channel, as today (`io/channel_adapt.rs:65-108`, `InputAdapter` with one configured channel). With `--input-channel <n>`, only channel n (1-based, as printed on an interface) reaches the engine. Reason: a mono mic (the AirPods, the MacBook's) and a stereo USB mic are unchanged; on a two-input interface the mean is 6 dB down with the other input's noise added (PLAN.md, Track P, "Where we are"), and naming the channel fixes it without guessing.
- **Where the choice lives:** `EngineConfig.stream` (`io::StreamConfig`, `io/device.rs:21-48`). `input_device: Option<String>` already exists. `input_channels: u16` (`:31`, the width asked of the device) is replaced by `input_channel: Option<u16>` (`None` = the mean of all channels; `Some(n)` = channel n). This is a deliberate change of a field name: the old field asked the device for a width, which this lane stops doing. The only readers are `engine/mod.rs:253` and `io/device.rs:281`, `:286`, `:386`.
- **Device loss, rate change, default change:** a supervisor thread, `kazoo-mic`, owns the input stream. Every 250 ms it checks the stream's health; every second it asks the host for the chosen device's facts (name, native rate, channel count). It reopens the mic when the stream reports `DeviceNotAvailable`, `StreamInvalidated` or a backend error; when the device's native rate is no longer the rate the stream opened at; and, when no device was named, when the system default input is now a different device. A named device that disappears is waited for by name, never replaced by the default: a named choice is a promise, and moving to another mic without being asked would be a surprise in the middle of a take. `BufferUnderrun` is counted, not treated as a loss. Reason for polling the rate: cpal 0.17.1 on macOS listens only for `kAudioDevicePropertyDeviceIsAlive` (`cpal-0.17.1/src/host/coreaudio/macos/mod.rs:85-170`), so a rate change made in Audio MIDI Setup reaches the program only if it asks.
- **One mic ring for the engine's life.** The output callback keeps reading `mic_cons` exactly as today (`processing.rs:466-478`, `:792-809`); this lane does not touch `processing.rs`. The ring's producer is lent to each input stream in turn: the capture state moved into a stream's callback holds it, and the capture's `Drop` sends it back to the supervisor over a `crossbeam_channel::bounded(1)`. When a stream is dropped (on the supervisor thread), cpal drops its callback and the producer comes home; the next stream gets it. Reason: the output callback never learns that the mic changed, so nothing on it allocates, frees, swaps or waits. If the producer is not back within 1 s of dropping a stream, the supervisor marks the mic `Stuck` and stops reopening: two writers on one SPSC ring is never allowed, and a restart is the honest answer.
- **The mic ring's size:** `max(buffer_size × 4, engine_rate / 5)` (200 ms; 9,600 at 48 kHz). Reason: a mic at another rate delivers its own callback sizes (1,024 frames at 24 kHz is 2,048 engine frames, over today's 512); the output callback already skips a stale backlog down to one block (`processing.rs:473-478`), so a larger ring adds no latency, only room.
- **The input callback's size:** ask for `clamp(round(engine_frames × mic_rate / engine_rate), 32, 1024)` mic frames, so one mic callback is about one engine block. The host falls back to the device's own size when refused (`host/audio_open.rs`, `fall_back_when_fixed_is_refused`), and the capture handles any size.
- **Start errors stay start errors.** A named device that does not exist, a default with no input device at all, a rate outside 8 to 192 kHz, or a channel the device does not have fails `engine::start` with words that name the device and say `kazoo mouth --list-inputs` lists the inputs. Reason: a misspelt name must not wait forever; once running, losses are the supervisor's.
- **Status is shown.** `EngineStatsSnapshot` gains `mic: MicStatus` (`Copy`, so the snapshot stays `Copy`), and the header's first row shows it.

### `kazoo-core/src/io/mic.rs` (`pub mod mic;` in `io/mod.rs`), tests in `io/mic/tests.rs`

```rust
/// What to open: which device, and which of its channels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicRequest {
    pub device: crate::host::audio::DeviceChoice,
    /// `None`: the mean of every channel. `Some(n)`: channel n, 1-based.
    pub channel: Option<u16>,
}

/// The engine side the mic feeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MicTarget { pub rate: u32, pub frames: usize }

pub const MIC_MIN_RATE: u32 = 8_000;
pub const MIC_MAX_RATE: u32 = 192_000;
pub const SINC_LEN: usize = 64;
pub const OVERSAMPLING: usize = 256;
pub const HEALTH_EVERY: Duration = Duration::from_millis(250);
pub const FACTS_EVERY: Duration = Duration::from_secs(1);
pub const RELEASE_WAIT: Duration = Duration::from_secs(1);
/// Device names are cut to this many bytes (at a char boundary) in the status.
pub const NAME_BYTES: usize = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MicState {
    /// No mic was opened (an engine built without one, and every test engine).
    #[default]
    Off,
    /// Running.
    Live,
    /// The device went away or failed; asked for again every second.
    Lost,
    /// A dropped stream never gave the ring back; no further reopening.
    Stuck,
}

/// A device name held in a fixed array, so the status is `Copy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MicName { bytes: [u8; NAME_BYTES], len: u8 }
impl MicName { pub fn new(name: &str) -> Self; pub fn as_str(&self) -> &str; }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MicStatus {
    pub state: MicState,
    pub device: MicName,
    pub device_rate: u32,
    pub device_channels: u16,
    pub channel: Option<u16>,
    pub engine_rate: u32,
    /// Times the mic was opened again after the first.
    pub reopens: u32,
    /// `BufferUnderrun` reports from the host, not losses.
    pub glitches: u32,
    /// Resampler calls that returned an error (the chunk was dropped and the
    /// resampler reset); the design makes this 0.
    pub resample_errors: u32,
}

/// Where the supervisor publishes the status: written by the supervisor
/// thread at most once a tick, read by the UI thread through
/// `EngineHandle::stats`. No audio callback touches it, so a `Mutex` is
/// allowed here (CLAUDE.md's lock-free rule is for the real-time threads).
#[derive(Debug, Default)]
pub struct MicShared { /* Mutex<MicStatus>, AtomicU32 glitches, AtomicU32 resample_errors */ }
impl MicShared { pub fn status(&self) -> MicStatus; }

/// Called on the input callback after each push into the ring with
/// (offered, accepted) samples; the engine counts the difference as
/// `mic_samples_dropped`, as `make_input_callback` does today.
pub type PushCount = Arc<dyn Fn(usize, usize) + Send + Sync>;

/// The running mic: its supervisor thread. Dropping it stops the thread,
/// drops the stream and joins.
pub struct Mic { /* stop flag, thread handle, Arc<MicShared> */ }
impl Mic {
    pub fn shared(&self) -> Arc<MicShared>;
    pub fn status(&self) -> MicStatus;
}

/// Open the mic, report the first open's error here, then hand the stream
/// to the `kazoo-mic` thread.
pub fn open_mic(request: MicRequest, target: MicTarget, sink: HeapProd<f32>, counted: PushCount) -> Result<Mic>;
```

**The backend seam, so tests need no device:**

```rust
pub(crate) struct MicFacts { pub name: String, pub rate: u32, pub channels: u16 }
pub(crate) enum MicFault { Lost(String), Glitch }
pub(crate) trait MicBackend: Send + 'static {
    type Stream;
    fn facts(&mut self, device: &DeviceChoice) -> Result<MicFacts>;
    /// Open `device` at `facts.rate` and `facts.channels`, asking for `frames`
    /// per callback, moving `capture` into its callback.
    fn open(&mut self, device: &DeviceChoice, facts: &MicFacts, frames: usize, capture: MicCapture) -> Result<Self::Stream>;
    fn fault(&mut self, stream: &Self::Stream) -> Option<MicFault>;
}
```

`CpalMic` is the real one: `facts` resolves the device with `io::device::resolve_input_device` (`device.rs:312-329`, which names a missing device) and reads `device_name` and `default_input_config()` (rate and channels); `open` prepares `OutputRequest { device, channels: facts.channels, rate: RateChoice::Native }.prepare_input_with_frames(frames)` (`host/audio.rs:76-83`), refuses a prepared rate that differs from `facts.rate` (the default moved between the two calls; the next tick catches it), and opens it with `PreparedInput::open` (`host/audio_open.rs:125-132`), the callback calling `capture.process(data, info.channels)`; `fault` drains the stream's errors with `OpenedOutput::check_error_with_handler` (`host/audio.rs`, beside `check_error`; `OpenedInput` is the same type), mapping `cpal::StreamError::BufferUnderrun` to `Glitch` and every other error to `Lost` with its text.

**`MicCapture`** (moved into each stream's callback; built on the supervisor thread, where it may allocate):
- `sink: Option<HeapProd<f32>>`, `home: crossbeam_channel::Sender<HeapProd<f32>>`, `counted: PushCount`, `channel: Option<u16>`, `adapter: InputAdapter` (one configured channel, sized for 4,096 frames: the mean case), `pick: Vec<f32>` (4,096: the named-channel case), `resampler: Option<MicResampler>` (`None` when `facts.rate == target.rate`), `shared: Arc<MicShared>`.
- `process(&mut self, data: &[f32], device_channels: u16)`: reduce to mono (the mean through `adapter.process`, or channel `n − 1` of each frame into `pick`, in passes of at most 4,096 frames), `sanitize_sample` each sample, then either push straight into `sink` (equal rates: bit for bit) or feed the resampler, which pushes each output chunk; after each push call `counted(offered, accepted)`. A channel beyond `device_channels` is impossible here (checked at open).
- `impl Drop for MicCapture`: take `sink` and `home.try_send` it; a full or disconnected channel means the supervisor is gone or already holds a producer, and the producer is dropped with an explicit `drop` (no empty arm, no `let _ =`).
- `InputAdapter` is a `pub` struct with `pub` methods (`io/channel_adapt.rs:65`, `:73`, `:83`) in a module private to `io`, so `io::mic` reaches it as `super::channel_adapt::InputAdapter` with no change to `channel_adapt.rs`. It stays in use (the mean case), so removing `build_streams_with_output` leaves no dead code there.

**`MicResampler`**, built once per stream on the supervisor thread: the rubato `Async` above, `input: Vec<Vec<f32>>` (1 × chunk), `filled: usize`, `output: Vec<Vec<f32>>` (1 × `output_frames_max()`). `push(&mut self, mono: &[f32], out: impl FnMut(&[f32]))` copies into `input[0][filled..]`; each time `filled == chunk` it wraps both with `audioadapter_buffers::direct::SequentialSliceOfVecs::new` / `new_mut` (they borrow, `audioadapter-buffers-2.0.0/src/direct.rs:132-161`), calls `process_into_buffer(&input, &mut output, None)`, hands `&output[0][..written]` to `out`, and sets `filled = 0`. An `Err` adds one to `shared.resample_errors`, calls `reset()` (no allocation, `asynchro.rs:604-612`) and drops the chunk. `delay_frames()` is `output_delay()`.

**The supervisor** (`Supervisor<B: MicBackend>`, run by `open_mic` on the `kazoo-mic` thread; tests drive it directly):
- `start(&mut self) -> Result<()>`: facts, the rate check, the channel check, open. Errors, with the device name in each: "input device '{name}' runs at {rate} Hz, which kazoo cannot resample (8000 to 192000)"; "input device '{name}' has {n} channels; channel {c} was asked for"; any `facts` error gets " (kazoo mouth --list-inputs lists the inputs)" appended. `open_mic` calls `start` on the new thread and waits for its result on a `bounded(1)` channel, so the first error returns from `engine::start` as today.
- `tick(&mut self, now: Instant)`, every `HEALTH_EVERY`:
  - `Live`: `fault` → `Glitch` adds to `glitches`; `Lost` drops the stream, waits up to `RELEASE_WAIT` for the producer (`recv_timeout`), and goes `Lost` (or `Stuck`). Every `FACTS_EVERY`: `facts` failing → drop and `Lost`; a different `rate`, or (with `DeviceChoice::Default`) a different `name` → drop, take the producer back, open again with the new facts (`reopens += 1`); a failed reopen → `Lost`.
  - `Lost`: every `FACTS_EVERY`, try `facts` and open; success → `Live`, `reopens += 1`.
  - `Stuck`, `Off`: nothing.
- The thread loop: `tick`, then `park_timeout(HEALTH_EVERY)`, until the stop flag; on stop it drops the stream. `Mic`'s `Drop` sets the flag, unparks and joins.

### `io/device.rs` and `io/mod.rs`

- `StreamConfig`: `input_channels: u16` → `input_channel: Option<u16>` (default `None`), its doc as above.
- `build_streams`, `build_streams_with_output` and `AudioStreams` are removed (`device.rs:54-77`, `:244-301`), with their re-exports in `io/mod.rs:15-16`. Their only caller is `engine::start`; a pair builder that ties the input to the output's rate is the fault this lane removes. In their place: `pub(crate) fn open_engine_output(prepared: PreparedOutput, output_callback: impl FnMut(&mut [f32]) + Send + 'static) -> Result<OpenedOutput>`, which keeps the stereo `OutputAdapter` exactly as `build_streams_with_output` did (`:287`, `:291-293`).
- `stream_config_default_values` asserts `input_channel == None`.

### `engine/mod.rs`

- `start`: after `prepare_output`, build the mic ring at `RingBufferCapacities::new(buffer_size, sample_rate).mic`, then `io::mic::open_mic(MicRequest { device: stream.input_device → DeviceChoice, channel: stream.input_channel }, MicTarget { rate: sample_rate, frames: buffer_size }, mic_prod, Arc::new(move |offered, accepted| stats.record_mic_push(offered, accepted)))`, before the output opens; then `stats.set_mic(mic.shared())`; then `open_engine_output(prepared, ...)`.
- `make_input_callback` (`:413-449`) is removed: the capture does its work.
- `hold_streams(output: OpenedOutput, mic: Mic, stats)`: `stats.set_audio_health(output.health())`; the holder thread keeps both; on shutdown it drops the output first, then the mic (which joins `kazoo-mic`).
- `RingBufferCapacities::from_buffer_size(bs)` becomes `new(bs, rate)` with `mic: max(bs × 4, rate / 5)`; its three tests (`:195-220`) take a rate and assert the new mic size at 48 kHz (9,600 for 128 frames).

### `engine/stats.rs`

- `audio_health: OnceLock<[StreamHealth; 2]>` → `OnceLock<StreamHealth>` (the output's only); `set_audio_health` and `check_audio` follow. An input fault is never fatal again.
- `mic: OnceLock<Arc<MicShared>>`, `pub(super) fn set_mic(&self, shared: Arc<MicShared>) -> Result<()>` (as `set_audio_health` does it), and `EngineStatsSnapshot.mic: MicStatus` (`MicStatus::default()` when unset). `is_clean` ignores it (it is not a loss counter), and kazoo-tui's `COUNTERS` (`kazoo-tui/src/status.rs`) are untouched.

### kazoo-tui and kazoo-cli

- `kazoo-tui/src/run.rs`: `pub struct Args { pub input: Option<String>, pub input_channel: Option<u16>, pub list_inputs: bool }` with `Default`. `parse_args` accepts `--input <name>`, `--input-channel <n>` (1 to 64; anything else is an error naming the argument) and `--list-inputs`; `-h`/`--help` still wins anywhere; an unknown argument is still refused by name. `USAGE` gains three lines for them. `--list-inputs` prints, for each of `io::enumerate_input_devices()` (`device.rs:106-131`), one line `name  (default)  up to N channels  rates: 24000, 48000` and exits 0 without starting the engine. Otherwise `main_loop` sets `stream: StreamConfig { input_device: args.input, input_channel: args.input_channel, ..StreamConfig::default() }` in the `EngineConfig` at `:85-88`. The `Args: Args` pattern at `:76` and the test helpers follow.
- `kazoo-tui/src/ui/header.rs`: row 1 (`draw_row_branding`, `:81-153`) gets `mic_spans(&app.engine.stats().mic)` after `desk_spans`:
  - `Off`: nothing.
  - `Live`, equal rates: `  mic: {name} {k} kHz` (dimmed text style); resampled: `  mic: {name} {mic k} → {engine k} kHz`; a named channel adds ` ch {n}`; `glitches > 0` adds `  {n} glitches` in amber. kHz is the rate / 1000 with trailing zeros dropped: 48, 44.1, 22.05, 11.025.
  - `Lost`: `  mic lost: {name}, waiting for it` in `ACCENT_PAUSE`.
  - `Stuck`: `  mic stuck: restart kazoo mouth` in `METER_RED`.
- `kazoo-cli/src/cli.rs`: `Mouth` → `Mouth(MouthArgs)`, `MouthArgs { #[arg(long)] input: Option<String>, #[arg(long, value_parser = clap::value_parser!(u16).range(1..=64))] input_channel: Option<u16>, #[arg(long)] list_inputs: bool }`, and `impl From<MouthArgs> for kazoo_tui::Args`. `main.rs:44` passes `args.into()`. `cli/tests.rs:34` and `studio.rs:112` match `Command::Mouth(_)`. (V08a will grow this same `MouthArgs` with `--output`, `--quantise`, `--silent`, `--score`; the flag is `--input <device>`, as V08a already names it.)

## Not this lane

- The output side: the engine still runs at the output's native rate, and an output that is lost or changes rate still ends the program as today. (When the AirPods' mic is opened, macOS moves the AirPods to their headset profile, which can change their output rate too; whether CoreAudio converts that for the open output stream is not known from reading. Cassio checks it with Tom after landing: AirPods in and out together.)
- Counting the engine's mic underruns. `read_mic_input` zero-pads a short read silently (`processing.rs:792-809`), and `feed_voice` is given only the samples read (`processing.rs:503-509`), so a mic callback that arrives late is a splice in the tracker's signal. V12 measures what a splice does to high notes; a counter belongs in a lane that edits `processing.rs`.
- P01's elastic read-back ring (`host/live/ring.rs`) for the mouth's mic. Its `live_pair` accepts only 32 to 256 frames (`ring.rs:101-104`), and reading it would change `processing.rs`'s mic path; today's skip-to-newest ring is kept.
- Drift correction between the mic's clock and the output's clock (`set_resample_ratio_relative`): the ring's skip-to-newest policy handles drift as it does today for two devices at the same rate.
- Choosing an input from the screen (`kazoo-tui`'s Audio I/O view lists devices but cannot switch: `ui/audio_io.rs`).
- The desk's inputs (P02) and the dedicated mouth (V08a).

## Red tests first

All in `kazoo-core/src/io/mic/tests.rs` unless named otherwise. A `FakeMic` implements `MicBackend`: its facts are set by the test; `open` keeps the `MicCapture` in an `Arc<Mutex<Option<MicCapture>>>` the test feeds with `capture.process(..)` (test-only, off any audio thread); dropping its stream drops the capture unless the test asked it to leak (for `Stuck`). The engine-side ring is a real `HeapRb<f32>`. Red for all: there is no `io::mic`. No test opens a real device.

1. `equal_rates_pass_bit_for_bit`: mic and engine at 48 kHz, one channel, 1 s of seeded noise in 128-frame callbacks: the ring's samples equal the input's `to_bits()` one for one.
2. `a_24k_mic_reaches_a_48k_engine_at_pitch`: 440 Hz at 0.5, 24 kHz mic, 64-frame callbacks, 1 s: the ring receives 48,000 samples ± (one output chunk + `delay_frames()`); over 0.2 to 0.9 s the zero-crossing pairs give 440 Hz within 3 cents, and the RMS is within 0.5 dB of 0.5/√2.
3. `every_rate_from_8k_to_192k_reaches_the_engine`: mic rates 8,000, 11,025, 16,000, 22,050, 24,000, 32,000, 44,100, 48,000, 88,200, 96,000, 176,400, 192,000 into engines at 48,000 and at 96,000: a 1,000 Hz sine (300 Hz at 8 kHz) arrives within 3 cents and at the expected sample count ± (one chunk's output + delay). 96 kHz into 48 kHz and 48 kHz into 96 kHz are both in it.
4. `a_96k_mic_is_anti_aliased_into_48k`: a 30 kHz sine at 0.5 at 96 kHz: the engine-side RMS after the delay is at least 60 dB under the input's.
5. `rates_outside_8k_to_192k_are_refused_in_words`: facts at 4,000 and at 384,000 Hz: `start` fails, and the text names the device and contains "cannot resample (8000 to 192000)".
6. `the_named_channel_is_the_one_heard`: a two-channel fake, channel 1 silent, channel 2 a 440 Hz sine at 0.5: with `channel: Some(2)` the ring carries the sine at 0.5 peak; with `None` it carries the mean (0.25 peak); `Some(3)` fails `start` with "has 2 channels; channel 3 was asked for".
7. `a_lost_device_is_reopened_when_it_returns`: a named fake reports `Lost`: the status goes `Lost`; ticks while `facts` errors open nothing; when `facts` succeeds the next facts tick opens it, the status is `Live` with `reopens == 1`, and samples fed to the new capture arrive in the same ring the test kept the consumer of.
8. `a_rate_change_reopens_at_the_new_rate`: facts move from 48,000 to 24,000: one facts tick later the stream is reopened with a resampler and the status says `device_rate == 24_000`.
9. `the_default_moving_to_another_device_is_followed`: with `DeviceChoice::Default`, the facts' name changes: reopened. With `DeviceChoice::Named`, the default changing touches nothing (the named facts do not change).
10. `a_glitch_is_not_a_loss`: `Glitch` twice: still `Live`, `glitches == 2`, no reopen.
11. `a_stream_that_never_lets_go_is_said`: the fake leaks the capture on drop and reports `Lost`: after `RELEASE_WAIT` the status is `Stuck` and later ticks open nothing (the test shortens the wait through a `Supervisor` field set only in tests, never by sleeping a real second).
12. `capture_never_allocates`: 10 s of a 24 kHz sine through `MicCapture::process` in 64-frame callbacks into a 48 kHz ring, inside `assert_no_alloc::assert_no_alloc`; then the same at equal rates and with `channel: Some(2)` on a two-channel fake.
13. `nonsense_samples_reach_the_engine_as_silence`: NaN, ±Inf and ±1e30 at 24 kHz: no panic, every ring sample finite, and a clean sine after them arrives at pitch.
14. `the_status_names_the_mic`: `MicName::new` of a 70-byte name with a multi-byte character at byte 47 cuts at a char boundary at or under 48 bytes; `as_str` round-trips a short name.
15. `kazoo-tui/src/run.rs` tests: `--input "AirPods Pro" --input-channel 2` parses to those values; `--input` with no value, `--input-channel 0` and `--input-channel x` are errors naming the argument; `--list-inputs` sets the flag; `[]` parses to `Args::default()`; the existing help tests still pass.
16. `kazoo-tui/src/ui/header.rs` tests: `header_shows_the_mic_and_its_rates` (a `Live` status at 24,000 into 48,000 renders `mic: AirPods Pro 24 → 48 kHz`; at 44,100 into 48,000 renders `44.1 → 48 kHz`); `header_shows_a_lost_mic` (`mic lost: AirPods Pro, waiting for it`); `header_without_a_mic_says_nothing_about_it` (`Off`). Build the status into whatever the header reads; if `app.engine.stats()` cannot be steered in a test, the header reads the status through a small `fn mic_spans(status: &MicStatus)` and the tests call it directly, as `desk_spans` is tested.
17. `kazoo-cli/src/cli/tests.rs`: `kazoo mouth --input X --input-channel 2 --list-inputs` parses to `Command::Mouth(MouthArgs { .. })` with those values; `kazoo mouth` alone gives the defaults; `--input-channel 65` is refused.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -g /` and `sysctl -n vm.loadavg` before each and put them in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-core -p kazoo-tui -p kazoo-cli --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-core -p kazoo-tui -p kazoo-cli --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-core/src/io kazoo-core/src/engine kazoo-tui/src kazoo-cli/src
```

Doc tests, since doc comments are written: `nice lockf -k /Users/tom/.kazoo-cargo.lock cargo test --doc -p kazoo-core -j 4`. While iterating, filter nextest to `-E 'test(mic)'`; run the three crates whole once at the end.

## Definition of done

- [ ] Every red test fails first (paste under RED), then passes; every existing test in the three crates passes.
- [ ] `git grep -n "RateChoice::Hz(sample_rate)" kazoo-core/src` finds nothing; the mic opens with `RateChoice::Native` only.
- [ ] `processing.rs` is not in the diff (`git diff --stat`), and nothing that runs on an audio callback allocates (tests 12 and the existing `processing_never_allocates` tests).
- [ ] Smoke without opening a device: `kazoo mouth --help` and `kazoo-tui --help` exit 0 and show the three flags (paste). `kazoo mouth --list-inputs` only lists devices and opens no stream; the builder may run it and pastes its output.
- [ ] Not the builder's, listed under NOT CONFIRMED: after landing, Cassio installs and asks Tom to start `kazoo mouth` with the AirPods mic as the default (it starts, the header says `24 → 48 kHz` or the AirPods' real rate), to pull the AirPods out while it runs (the header says `mic lost`, the engine keeps running, and the mic comes back when they reconnect), and, once the interface is plugged in, `kazoo mouth --input "<its name>" --input-channel 1` at 96 kHz. The builder never opens Tom's mic or speakers: the wall plays on them.

## Code quality

As PLAN.md's STACK RULES. `MicCapture::process` runs on the input callback: no allocation, lock, I/O, format or panic; every buffer is sized when the capture is built on the supervisor thread; indexes are bounded by construction (passes of at most 4,096 frames into 4,096-sample buffers). The capture's `Drop` runs on whichever thread drops the stream (the supervisor's or cpal's disconnect thread), never on the output callback. The supervisor may allocate. `io/mic.rs` stays under 1000 lines: if it nears that, the resampler goes to `io/mic/resample.rs` and the supervisor to `io/mic/supervisor.rs` (say so under CHANGED). No `as` casts clippy pedantic flags: rates and frame counts convert through `f64::from` and checked helpers written once in `mic.rs`.

## For Cassio

- Amend KZ-V08a before it opens: its "Audio (`engine.rs`)" bullet opens the input at `RateChoice::Hz(<output rate>)` and fails otherwise. It should call `kazoo_core::io::mic::open_mic` with the mouth's own ring (`MicTarget { rate: output rate, frames: OutputInfo::frames }`) and drop the "the mic must run at the speakers' rate" error; its `MouthArgs` grows from this lane's (`--input`, `--input-channel`, `--list-inputs`), and its new `Command::Tui` passes `kazoo_tui::Args::default()`.

## END

```
START KZ-V11 base=<sha> branch=lane/kz-V11 df=<GB> load=<1m> at <time>
END KZ-V11 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
