# KZ-P02: the desk takes named inputs from the audio interface, each a strip with gain, a converter clip light, a profile and its latency, at 64-frame buffers

Line numbers are from the sweep that wrote this brief, at `efef1b6`, and may have moved. Find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Place in the track: Track P (audio inputs), lane 2. P01 gave the input path; this lane puts it on the desk. P05 adds each input's gate and its place in the insert chain; P08 lets an input drive the voice.

Written by the Track P drafting seat, 13 Oct 2026 (night). Read by Cassio, 13 Oct night, line by line. Checked: the 1 ms crossfade the help names is P01's drift and underrun jump, and `NAME_BYTES` is 12. Changes: the device parse rule now says how an optional profile is told from a channel, and test 1's colon example has its name. The 64-frame default with an input is accepted (two buffers are 2.67 ms).

Time: about 30 to 40 minutes. Crates: kazoo-mix, kazoo-cli.

Depends:
- KZ-P01 (`host::live`) must have landed before this lane opens.
- KZ-C01 (strips without limit: `StripState`, the seat table, `MAX_STRIPS`) must have landed before this lane opens.
- If KZ-C02 has landed, the strip renders in blocks (`pre: StereoBlock`); write the live source into that path. If not, into the per-frame path. Find it by what it does.

Files (8): `kazoo-mix/src/inputs.rs` (new), `kazoo-mix/src/inputs/tests.rs` (new), the strip's file (`engine.rs`, or `engine/strip.rs` if C01 split it), `run.rs`, `shared.rs`, `ui.rs` (strip head only), `hub.rs` (seats for inputs), `kazoo-cli/src/cli.rs`.

Why: Tom's main use is "to get the sound in there and then probably process it with effects", for voice, guitar, bass, synths, anything on his interface. The desk is where sound is mixed and processed, and today it opens no input (`run.rs:301-312` opens only the default output, at 128 frames).

## Outcome

`kazoo mix --input gtr=default:1:guitar --input bass="Fireface 400":2:bass` starts the desk with two input strips, `gtr` and `bass`, left of every instrument's strip. Each is heard through its strip (trim as input gain, EQ, fader, pan) at the interface's latency. Its head shows the profile, a converter clip light that latches when the raw samples reach full scale, the software latency (`2.7ms`), and `2clk` when the input device is not the output device. With any input, the desk runs at 64-frame buffers unless `--frames` says otherwise. `kazoo mix --round-trip default:1` measures the true round trip through a loopback cable, prints it and exits.

## The design (decided; build exactly this)

### The command line (`kazoo-cli/src/cli.rs:65-69` `MixArgs`, `:231-235`; `kazoo_mix::Args`)

- `--input <name>=<device>:<channel>[:<profile>]`, repeatable up to `MAX_INPUTS = 8` (P01's `MAX_LIVE_CHANNELS`).
  - `name` is 1 to 10 of `a-z 0-9 -`, unique. Ten leaves room in `NAME_BYTES` (12, `engine.rs:52`), and the strip key is `in/<name>` (C01's `StreamKey`).
  - `device` is `default` or a device name; a name containing `:` is quoted. The parse splits from the right: if the last `:`-separated field is all digits it is the channel and the profile is `line`; otherwise it is the profile and the field before it is the channel. Everything left of the channel is the device.
  - `channel` is 1 to 64.
  - `profile` is one of P01's `InputProfile` names, default `line`.
  - A bad spec prints what was wrong and the form, and exits 2, as clap does.
- `--frames <32|64|128|256>`. The default is 64 when there is any `--input`, else 128 as today. Why 64: two buffers are 2.67 ms at 48 kHz, inside guitar standard. 32 is allowed for anyone who wants 1.33 ms back and whose machine keeps up. The desk's callback runs inserts on every strip, and at 32 frames its fixed cost per callback doubles: the header's `cpu` (C02) will show it.
- `--round-trip <device>:<channel>` excludes every other flag. It opens the default output and that input at `--frames`, runs P01's `RoundTrip`, prints `round trip 5.4 ms (259 frames at 48000 Hz, 64-frame buffers); software share 2.7 ms`, and exits 0. On `NothingCameBack` it prints "no click came back: connect an output to that input with a cable, and turn monitoring off on the interface", and exits 1. On `TooUneven` it prints the spread, and exits 1.

### Opening (`kazoo-mix/src/inputs.rs`)

- `pub struct InputSpec { name, device: DeviceChoice, channel: u16, profile: InputProfile }`, `pub fn parse_input(text) -> Result<InputSpec, String>`.
- `pub struct DeskInputs { streams: Vec<LiveInputStream>, strips: Vec<InputStrip { spec, slot, device_name, two_clocks: bool }> }`.
- `open(specs, rate, frames, output_device_name)` groups specs by device and opens one `open_live_input` per device with that device's channels. A device that cannot run at the desk's rate fails startup with "input device '<name>' cannot run at <rate> Hz (the desk's output rate); set both to the same rate in Audio MIDI Setup".
- `two_clocks` is `device_name != output_device_name`.
- Why fail rather than resample: a resampler adds latency on the one path that must have none.

### The desk opens at the frames asked (`run.rs:301-312`)

`MixerAudio::start` takes `frames` and calls `request.prepare_output_with_frames(frames)` (public since P01) instead of `prepare_output()`. The output's device name is kept for `two_clocks`. If the output stream's `fixed_buffer()` is false, the footer says "the output device refused <n>-frame buffers; it runs at its own size". A device may also clamp the asked size to its minimum without refusing (P01's review: `configure` turns 32 into 64 and `fixed_buffer()` stays true), so the head's latency figure always comes from `LiveInput::latency_frames()` (the measured callback size), never from `--frames`; when the measured input callback is larger than asked, the footer says "the interface runs <m>-frame buffers, not <n>".

### Seats (`hub.rs`)

Inputs take the first strips, `0..k`, before the hub starts. C01's `StripSeat` gains a state `Input` that is never freed, reclaimed or forgotten, so the first instrument gets strip `k`. Each input strip is `Live { key: "in/<name>" }` from the first callback. Why first and fixed: Tom's instruments are always plugged in. A fixed place on the left is where a console's inputs are.

### The strip's source (the strip's file)

`ChannelStrip` gains `live: Option<LiveInput>`, set once at engine start by `MixerEngine::attach_live(slot, name, key, LiveInput)` before the stream starts, never detached. In the render path, a strip with `live` reads `frames` mono samples into a pre-allocated `mono: Vec<f32>` of `MAX_RENDER_FRAMES` (allocated in `new`, as C01 sizes every strip's buffers), and copies them to left and right. Pan then places it in the stereo field as for any mono source. From there it is an ordinary strip: trim (±20 dB, `controls.rs:20`, the input's gain), EQ, inserts (C02), fader, mute, solo, pan, meters and the existing post-trim clip light. A live strip consumes no `AudioBlockConsumer`, so the studio clock's lead (`studio_clock.rs:85-96`) never applies to it: that lead is why an instrument through a link is about 11 ms late, and an input is not.

### Readouts (`shared.rs`, `ui.rs` strip head)

- Per slot: `input: InputCell { raw_clip: AtomicBool (latched), latency_frames: AtomicU32, profile: AtomicU8, two_clocks: AtomicBool, error: AtomicBool }`. The callback updates `raw_clip` and `latency_frames` from `LiveInput::stats()` and `latency_frames()` once per buffer (atomic loads and stores only). The UI thread sets `error` from each `LiveInputStream::check_error()` once per frame, and the footer shows the message once.
- The existing clip-reset key (find the key that clears strip clip lights) also clears `raw_clip`.
- The strip head of an input shows, in its free space: the profile's short name (`vox`, `gtr`, `bass`, `line`); `ADC` in the clip colour when `raw_clip` is latched (in the dim colour otherwise); the latency as `2.7ms` (`latency_frames / rate`); and `2clk` in amber when `two_clocks`. The help gains: "ADC: the interface clipped before any gain here; turn the interface's gain down. 2clk: input and output are different devices; their clocks drift, and the desk pulls them back with a 1 ms crossfade now and then".
- Why a separate ADC light: the strip's own clip light (`engine.rs:49`) is after trim, and a converter clip cannot be fixed by turning trim down.

## Not this lane

The input's gate, its insert latency and its exemption from compensation (P05). Tracking (P08). Adding or removing inputs while the desk runs: inputs are given at start, and a session file (C08) can carry them later. Recording inputs (C04 records any armed strip, inputs included, with nothing more).

## Red tests first

`kazoo-mix/src/inputs/tests.rs`, plus engine and UI tests where the existing ones live. Red for all: none of this exists.

1. `input_specs_parse`: `gtr=default:1:guitar`; `amp="Fireface 400: Main":2` (the device name holds a colon, profile defaults to `line`); `amp="Fireface 400: Main":2:bass`; `x=default:0` refused (channel); `gtr=default:1:banjo` refused, naming the four profiles; a duplicate name refused; a ninth input refused; `Gtr` refused (case).
2. `frames_default_to_64_with_an_input_and_128_without`; `--frames 48` refused.
3. `round_trip_excludes_every_other_flag`.
4. `inputs_take_the_first_strips_and_instruments_follow`: with two inputs, a hub client joins and gets slot 2 (`Joined { slot: 2 }`); the input seats are never freed when D is pressed on them.
5. `a_live_strip_is_heard_with_its_trim`: `live_pair` feeds 0.25 into an input strip; with trim at +6 dB, centre pan and the fader at 0 dB, the output settles at `0.25 × db_to_gain(6) × centre pan gain` within 1e-3. With the fader down, silence.
6. `a_live_strip_has_no_studio_clock_lead`: an impulse written to the ring just before a callback appears in that callback's output (in-before-out schedule), whereas a linked source's impulse lands at least `device + block + margin` later.
7. `the_adc_light_latches_and_the_clip_reset_clears_it`: a raw 0.9995 sample latches `raw_clip`, trim at −20 dB does not stop it, and the reset key clears it.
8. `the_head_shows_profile_adc_latency_and_two_clocks`: a `TestBackend` draw at 160x40 with one `guitar` input, `raw_clip`, latency 128 frames at 48 kHz and `two_clocks`: the head text contains `gtr`, `ADC`, `2.7ms`, `2clk`.
9. `a_glide_and_a_vibrato_come_through_the_strip_without_steps`: the perfect fourth over 2 s and ±30 cents at 6 Hz through `live_pair` and an input strip (EQ flat, trim 0). The pitch of every 10 ms window of the left output is within 2 cents of the input's, and no window steps by more than the input's own change plus 1 cent.
10. `the_round_trip_command_reports_and_refuses`: the runner, fed through `live_pair` with a simulated 317-frame device, prints the line with `round trip`. Fed nothing, it returns exit 1 with the cable sentence.
11. `the_callback_never_allocates_with_inputs`: `AudioCallback::process` with two live strips for 1 s inside `assert_no_alloc`.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-mix -p kazoo-cli --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-mix -p kazoo-cli --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-mix/src kazoo-cli/src
```

## Definition of done

- [ ] All red tests fail first (paste the failures), then pass. Every existing kazoo-mix and kazoo-cli test passes.
- [ ] `kazoo mix --help` shows `--input`, `--frames` and `--round-trip` with the forms above.
- [ ] Not the builder's: after landing and reinstalling, Cassio with Tom:
  - runs `kazoo mix --round-trip <interface>:1` with a loopback cable, at 64 and at 32 frames, and writes both figures into BENCH.md under `## KZ-P01 round trip`;
  - starts `kazoo mix --input gtr=<interface>:1:guitar --input bass=<interface>:2:bass`, asks Tom to play each through his preamp, and checks the gain staging (ADC never lit at his loudest), the latency figure on each head, and that he hears no crackle in a five-minute play.

  The builder never opens a device, lists this under NOT CONFIRMED, and does not claim it.

## Code quality

As PLAN.md's STACK RULES. The render path with a live strip never allocates, locks, formats or does I/O. `LiveInput::read` is the only call into the input. Every readout is an atomic. `hub.rs` (1226 lines), `ui.rs` (1891) and `engine.rs` (2046) are already over 1000 lines: never split them here. Add only the calls named, put the logic in `inputs.rs`, and list those files under FOUND with their line counts.

## END

```
START KZ-P02 base=<sha> branch=lane/kz-P02 df=<GB> at <time>
END KZ-P02 head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
