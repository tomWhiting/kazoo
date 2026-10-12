# KZ-A02: one audio host opens the device for every instrument

Sweep: e0de0c0. Lines may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md with `-p kazoo-core` plus every crate touched.

## Outcome

Every instrument, the desk and the mouth synth open their audio device through one function in kazoo-core. The ten private copies of cpal device setup are gone.

## The design (decided; build exactly this)

- New module `kazoo-core/src/host/mod.rs` with `pub mod audio;`. `kazoo-core/src/host/audio.rs`:
  - `pub struct OutputRequest { pub device: DeviceChoice, pub channels: u16, pub rate: RateChoice }` with `DeviceChoice::{Default, Named(String)}` and `RateChoice::{Native, Hz(u32)}`.
  - `pub fn open_output<F>(req: &OutputRequest, callback: F) -> Result<OpenedOutput, Error> where F: FnMut(&mut [f32], &OutputInfo) + Send + 'static`. `OutputInfo { rate: u32, channels: u16, frames: usize }` is pre-built once. `OpenedOutput` owns the `cpal::Stream` and reports `rate()`, `channels()`, `buffer_frames()`; dropping it stops the stream.
  - Device enumeration and native-rate selection come from the helpers already in `kazoo-core/src/io/device.rs` (the survey found no instrument uses them); the f32 vs i16 sample-format fallback is implemented once here.
  - `pub fn open_input` with the same shape for the mouth synth (kazoo-tui) and the desk.
- Replace, call site by call site, the private device code in: kazoo-808, kazoo-303, kazoo-arp, kazoo-cs80, kazoo-mini, kazoo-juno, kazoo-prophet, kazoo-dx, kazoo-string, kazoo-glass (each has its own `audio.rs` or a block in `main.rs` building a `StreamConfig`; juno and prophet `audio.rs` differ by 8 lines), kazoo-mix's device open in `kazoo-mix/src/engine.rs`, and kazoo-tui's in its engine start. Delete each private copy once its crate compiles against the host.
- Error text: the host returns `Error::Device(String)` naming the device and the rate asked for; the caller prints it and exits non-zero. No silent default.

## Not this lane

Hub link setup (KZ-A03), terminal host (KZ-A04), the `kazoo` binary (KZ-A06).

## Red tests first

- `host::audio::tests::request_defaults_to_native_rate_and_stereo` (pure: builds the `StreamConfig` from a fake device description without opening a device).
- `host::audio::tests::named_device_that_does_not_exist_is_an_error_naming_it`.
- `host::audio::tests::info_is_built_once_and_matches_the_stream` (callback receives the same `OutputInfo` on every call; counted with an `AtomicUsize`).
- Each converted crate keeps its existing tests green.

## Definition of done

- [ ] `grep -rn "StreamConfig" --include=*.rs kazoo-808 kazoo-303 kazoo-arp kazoo-cs80 kazoo-mini kazoo-juno kazoo-prophet kazoo-dx kazoo-string kazoo-glass kazoo-mix kazoo-tui` returns nothing: only kazoo-core builds stream configs.
- [ ] Three red tests then green; every converted crate's nextest and clippy green.
- [ ] Each instrument still starts and plays (run `kazoo-808` headless for one second under `timeout` and confirm no error on stderr).
