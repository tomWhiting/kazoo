# KZ-A02b: the shared audio host opens an output with any channel count, as the instruments did before A02

Crate: kazoo-core (`src/host/audio.rs`), plus any instrument whose callback cannot take a count other than 2. Depends: A02. This lane continues branch lane/kz-A02b, which starts at A02's reviewed head e2de947: everything A02 and its review did is already there and is not yours to change. Line numbers move; find each item by what it does.

## Outcome

A02 moved every instrument onto one shared host, and its `configure` now asks for exactly `request.channels` (2 by default) and fails if the device lists no range with that count. On CoreAudio a device lists only its full channel count, so an 8-out interface now fails with "no supported f32, i16 or u16 configuration for 2 channels". Before A02 each instrument opened the device's default channel count and wrote stereo into it. After this lane an instrument opens on a mono device, a stereo device and an 8-channel interface, and plays on the first two channels (or both mixed to the one channel of a mono device), with silence on the rest.

## The design (decided; build exactly this)

- `configure` picks a range in this order, keeping the existing format order (F32, I16, U16) and rate and buffer checks inside each step:
  1. a range whose channel count equals `request.channels`;
  2. otherwise the range with the smallest channel count above `request.channels`;
  3. otherwise the range with the largest channel count below it (a mono device).
  The chosen `OutputInfo.channels` and `StreamConfig.channels` are the range's count, not the request's. The error text when nothing matches names the counts the device offers.
- `OutputRequest.channels` keeps its meaning as the count the instrument wants to write; document that the device may be opened wider or narrower and that `OutputInfo.channels` is what the callback gets.
- Every instrument already reads `output_info.channels`. Check each callback that writes frames (kazoo-303, 808, arp, cs80, dx, glass, juno, mini, string and the rest the host serves; grep for `output_info.channels`): with more than 2 channels it writes L and R to channels 0 and 1 and 0.0 to the rest; with 1 channel it writes (L + R) * 0.5. Fix any that do not, in the instrument's own callback code, without allocating on the callback path.
- No change to rate choice, buffer handling or the reviewer's chunked-callback fix from A02.

## Red tests first

In `kazoo-core/src/host/audio.rs` tests (next to the existing `configure` tests that build `SupportedStreamConfigRange` lists by hand):
- `an_eight_channel_device_opens_with_eight_channels` (ranges list only 8 channels; red today with the "no supported configuration" error).
- `a_mono_device_opens_with_one_channel` (ranges list only 1).
- `an_exact_match_is_preferred_over_a_wider_range` (ranges list 2 and 8; chooses 2).
For one instrument callback (the 808's, or a shared frame writer if one exists): `stereo_lands_on_the_first_two_of_eight_channels` and `mono_output_gets_the_mean_of_left_and_right`. Save each red's output to the logs.

## Checks

```
nice -n 10 env CARGO_BUILD_JOBS=4 cargo clippy --workspace --all-targets -- -D warnings
nice -n 10 env CARGO_BUILD_JOBS=4 cargo nextest run -p kazoo-core -p <each instrument crate you changed>
cargo fmt --all --check
ast-grep scan
```

## Definition of done

- [ ] The three configure tests and the two callback tests pass; every existing test in the crates you touched passes (kazoo-mix's `a_paced_source_with_a_jittery_timer...` fails on main under load too and is not this lane's).
- [ ] No instrument callback assumes 2 channels.
- [ ] No lint suppression; no file outside kazoo-core and the instrument run/callback files changed.
