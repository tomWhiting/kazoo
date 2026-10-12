# KZ-B05: an instrument at another sample rate is taken and resampled on the hub thread

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename. Depends: KZ-B03b landed (streams on the hub; this lane resamples per stream).

## Outcome

Today a desk at 48 kHz refuses a 44.1 kHz instrument: `kazoo-mix/src/hub.rs:711–718` `RefuseReason::SampleRate`, "use the same device". When this lane is done, the instrument is seated and heard: the hub thread resamples each of its streams with rubato (already a dependency of kazoo-core, used offline at `kazoo-core/src/io/resample.rs:60–68`) into the strip ring, with buffers allocated once at registration and nothing on the desk's audio callback. Transport syncs and paces still name frames of the instrument's own stream, so it stays in step. A rate the desk cannot resample (below 8 kHz or above 192 kHz) is still refused, in words.

## The design (decided; build exactly this)

- Dependencies: `kazoo-mix/Cargo.toml` adds `rubato = "1.0.1"` and `audioadapter-buffers = "2.0.0"` (both in `Cargo.lock` through kazoo-core).
- New `kazoo-mix/src/hub/resample.rs`: `pub struct StreamResampler { fft: rubato::Fft<f32>, input: Vec<Vec<f32>>, output: Vec<Vec<f32>>, interleaved: Vec<f32>, filled: usize, chunk_start: u64, channels: usize }`. Built with `Fft::<f32>::new(instrument_rate, desk_rate, CHUNK_IN, 1, channels, FixedSync::Input)` as `resample.rs:60–68` does, `pub const CHUNK_IN: usize = 256` (about 6 ms at 44.1 kHz: the latency the chunk adds), `input` is `channels` vectors of `CHUNK_IN`, `output` is `channels` vectors of `fft.output_frames_max()`, `interleaved` is `output_frames_max() * channels`. `pub fn delay_frames(&self) -> u32` is `fft.output_delay()`. `pub fn feed(&mut self, stream_frame: u64, bytes: &[u8], mut out: impl FnMut(u64, &[f32]))`: decodes the LE samples (sanitising) into `input` planar from `filled`; when `filled` reaches `CHUNK_IN`, runs `process_into_buffer` with `SequentialSliceOfVecs` adapters built over the pre-sized vectors (`resample.rs:75–85`; adapters borrow, they do not allocate), interleaves `output[..nbr_out]` into `interleaved`, and calls `out(chunk_start, &interleaved[..nbr_out * channels])`; `chunk_start` is the instrument stream frame of the chunk's first input frame, recorded when `filled` was 0. A block longer than what remains of a chunk is fed in pieces. Allocations happen only in `new`; `process_into_buffer` on pre-sized buffers allocates nothing (rubato's contract for the fixed-input resampler).
- `pub struct RateMap { instrument: u32, desk: u32 }` in the same file: `to_desk(frame: u64) -> u64` is `(u128::from(frame) * desk / instrument) as u64`, `to_instrument` the inverse; both identity when the rates are equal. Every place that turns a stream frame into a desk-rate frame or back goes through it: the stamp's input, `song_sync` (`hub.rs:1082–1108`: `stream_at` is in desk-rate stream frames; wrap it in `to_instrument(.. + delay)` before it goes on the wire), `pace` (`instrument.rs:327`: likewise). Put the two conversions on `StreamSlot` as `desk_stream_frame(instrument_frame)` and `instrument_frame(desk_stream_frame)` so each call site calls one function; `StreamSlot` holds `rate: RateMap` and `resampler: Option<StreamResampler>` (`None` at the desk's rate).
- Stamping a resampled stream: `StreamSlot::push_audio` (B03b) with a resampler feeds it and, per output chunk, stamps `to_desk(chunk_start).wrapping_sub(delay)` with `nbr_out` frames and pushes the interleaved chunk with `push_block` (`audio_transport.rs:159`). That is one copy more than the same-rate path (planar in, interleaved out); write it in the module doc: the resampled path costs three user-space copies, the same-rate path two (B02).
- Lead: the chunk and the filter delay are real latency; `fit_lead` (`instrument.rs:346–349`) gives a resampled stream `asked + to_desk(CHUNK_IN) + delay_frames` of extra lead, held by `most_extra_lead` as now. The registered `block_frames` (B02) is converted with `to_desk` for the block-size check and `most_extra_lead`.
- `register` (`hub.rs:701–784`): the rate check becomes `(8_000..=192_000).contains(&register.sample_rate)`; otherwise `RefuseReason::SampleRate` reworded: "it runs at {instrument} Hz, which the desk cannot resample (8000 to 192000)". `Registration` carries `instrument_rate` and `desk_rate`. `RegisteredMsg.hub_sample_rate` keeps telling the desk's rate; the instrument keeps rendering at its own.
- Health: a resampled stream's placements and drops count as now (they happen per output chunk).
- The desk callback (`callback.rs`, `engine.rs`) is untouched: it pops blocks from the strip ring as it always has.

## Not this lane

- Streams and strips: KZ-B03a/b. Copies on the same-rate path: KZ-B02.
- Choosing the desk's device or rate: not planned here (the desk opens the default device at its default config, `main.rs:258–265`).
- Latency compensation across strips: KZ-C02.

## Red tests first

1. `kazoo-mix/src/hub/resample.rs` tests: `a_sine_through_the_resampler_keeps_its_frequency_and_level` (440 Hz at 44.1 kHz fed in 256-frame blocks for one second; the output chunks concatenated at 48 kHz have 440 ± 2 zero-crossing pairs per second and a peak within 0.05 of the input's); `blocks_that_straddle_a_chunk_are_split_and_nothing_is_lost` (feed 100-frame blocks; total output frames equal `to_desk(total input) ± 2`); `rate_map_round_trips_and_is_identity_at_equal_rates`.
2. `kazoo-mix/src/hub/tests.rs` (`Rig`, `:27–97`): `an_instrument_at_44100_is_accepted_and_heard_at_48000` (connect at 44,100; `Joined`; stream a 440 Hz sine; after the lead, 0.5 s of `render()` shows a 440 ± 5 Hz tone on the left channel by zero-crossing count); `an_instrument_at_96000_is_heard_too` (same at 96,000); `a_resampled_instruments_sync_names_its_own_frames` (connect at 44,100, stream one second of blocks, `set_playing(true)`; the sync's `at_frame` is at most the instrument frames sent plus two seconds at 44,100, and at least the frames sent; and `DeskPaceMsg::playing_stream` for a paced instrument at 44,100 advances at 44,100 frames per second ± 2 % over two seconds); `a_rate_the_desk_cannot_resample_is_refused_in_words` (4,000 Hz; the text contains "cannot resample"); `an_instrument_at_the_desks_rate_takes_the_same_path_as_before` (the existing `an_instrument_plugs_into_the_first_free_strip_and_is_heard` still passes and `snapshot()` reports `resampled == 0`, a new `HubSnapshot` counter of streams being resampled).

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-mix
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy -p kazoo-mix --all-targets -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-mix/src
```

Doc tests if doc comments changed: `nice -n 10 env CARGO_BUILD_JOBS=4 cargo test --doc -p kazoo-mix`.

## Definition of done

- [ ] A 44.1 kHz and a 96 kHz instrument are seated and heard at pitch on a 48 kHz desk; the refusal remains only outside 8–192 kHz.
- [ ] `StreamResampler` allocates only in `new` (write in DONE which rubato call runs per chunk and why it allocates nothing); the desk callback is unchanged (`git diff --stat` shows no change to `callback.rs` or `engine.rs`).
- [ ] Every stream-frame conversion goes through `RateMap` on `StreamSlot`; syncs and paces are in the instrument's own frames (test 3).
- [ ] Eight red tests red first (paste output), then green; every existing hub test green.
- [ ] BENCH: hub thread CPU per second of audio for one resampled stereo stream (measure with `rustix::time::clock_gettime(ThreadCPUTime)` around 1,000 chunks in the unit test; the wall uses the same clock at `kazoo-wall/src/engine/mod.rs:730`), reported in microseconds per chunk.

## Code quality

The tree's rules: `unsafe_code` forbidden; no `#[allow]`/`#[expect]`; no `let _ =`; no `.ok()` discards; no empty `Err` arms (`process_into_buffer`'s error ends the stream with `LeaveReason::Protocol` and the error's text); no `if let Ok` without `else`; `resample.rs` under 1000 lines; no TODO, no stubs. On the desk's audio callback nothing changes. The hub thread is real-time: resampling is bounded work per chunk, no allocation after registration. Every sample is sanitised on decode, and the resampler's output is sanitised by `push_block`. Red first, then green, one behaviour per commit.

## END

```
START KZ-B05 base=<sha> branch=lane/kz-b05 df=<GB> at <time>
END KZ-B05 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
