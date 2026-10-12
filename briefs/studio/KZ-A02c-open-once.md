# KZ-A02c: an instrument opens its output once, and the engine's render width is fixed at stereo

Crates: kazoo-core (`src/host/audio.rs`, `src/host/audio_open.rs`, `src/io/device.rs`, `src/io/channel_adapt.rs`, `src/engine/`), and the `run.rs` of every program that opens output through the host (kazoo-303, 808, arp, cs80, dx, glass, juno, mini, mix, prophet, string; grep `request.info()` to find them all). Depends: A02 and A02b (landed). Line numbers move; find each item by what it does.

## Outcome

The A02b review left two items open; this lane closes both.

1. **Two probes.** Every `run.rs` calls `request.info()` (which probes the device and picks a configuration), sizes its engine from that `OutputInfo`, then calls `open_output`, which probes again. If the default device changes between the two (headphones plugged in, an interface woken), the stream opens with a channel count or rate the engine was not built for and plays at the wrong stride or speed. After this lane there is one probe: the configuration the engine was sized from is the one opened, or the open fails with an error that says the device changed.
2. **Stereo-only render.** `kazoo-core/src/engine/processing.rs` renders stereo frames (`chunk_len = mic_block * 2`) whatever `DeviceConfig.output_channels` says, while the review's `OutputAdapter` in `io/device.rs` `build_streams` adapts from the configured count. A caller setting `output_channels = 1` would get stereo frames read as mono. After this lane the engine's render width is always 2 and the adapter always adapts from 2 to the opened device width; `output_channels` means only "how many device channels to ask for".

## The design (decided; build exactly this)

- In `host/audio.rs` add `pub struct PreparedOutput { device, chosen, context }` (the three things `prepare` already returns) with `pub fn info(&self) -> OutputInfo` and `OutputRequest::prepare_output(&self) -> Result<PreparedOutput>` (and the same for input if `open_input` has the same shape). `open_output_with_frames` becomes a thin wrapper: `request.prepare_output_with_frames(frames)?.open(callback)`. `PreparedOutput::open(self, callback)` builds the stream from the stored device and config without probing again, keeping the Bluetooth fallback (`fall_back_when_fixed_is_refused`) and the chunked callback from A02's review. If the fallback changes the buffer size, `OutputInfo.frames` follows it as today.
- `OutputRequest::info()` stays only if something other than the instruments needs a probe without opening; if nothing does after the change, remove it.
- Every `run.rs`: `let prepared = request.prepare_output()?; let output_info = prepared.info();` ... size the engine ... `let stream = prepared.open(callback)?;`. No second probe anywhere (grep proves it).
- The `RateChoice::Hz(output_info.rate)` re-assignment in each `run.rs` (which existed only to make the second probe pick the same rate) goes.
- `engine/processing.rs` keeps rendering stereo. `io/device.rs` `build_streams` always builds `OutputAdapter` from 2 channels to the opened device count, and `DeviceConfig.output_channels` documents that it is the requested device count. A `DeviceConfig` with `output_channels = 1` plays a mono mix of the stereo render.

## Red tests first

- `host::audio::tests::prepared_output_opens_the_configuration_it_reported`: with the existing test seam that builds `SupportedStreamConfigRange` lists by hand (or a fake device trait if one exists), `prepare_output` then `open` uses the same `StreamConfig` (channels, rate, buffer size) that `info()` reported; red because `prepare_output` does not exist.
- `io::device::tests::a_mono_output_config_gets_a_mono_mix_of_the_stereo_render` (or next to `channel_adapt` tests): a stereo render block through the adapter configured as `DeviceConfig { output_channels: 1, .. }` yields (L+R)/2 per frame; red today if the adapter is built from the configured count.
- A grep-backed check is not a test; the reviewer greps for a second probe.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy --workspace --all-targets -- -D warnings
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-core -p <every crate whose run.rs changed>
cargo fmt --all --check
ast-grep scan
```

## Definition of done

- [ ] No program probes the output device twice; each opens the configuration its engine was sized from.
- [ ] The engine's render width is fixed at 2 and the adapter maps it to any device width, mono included.
- [ ] Both red tests then green; every existing test in the crates touched passes (kazoo-mix's jittery-timer hub test is load-sensitive and was red on main at load 240).
- [ ] No lint suppression; every new file under 1000 lines; no existing file split or moved.
