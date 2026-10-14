# KZ-D18: a headset's mic never takes the desk down: the mouth does not open the mic of the device it plays through, every refusal says why where the player is looking, and the mouth stops re-creating its output

Line numbers are from main at `fe32b20` and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with `-p kazoo-core -p kazoo-mouth -p kazoo-mix -p kazoo-wall`.

Written by Cassio, 15 Oct 2026, 00:0x, from an Opus agent's diagnosis of Tom's report. Decided by Cassio; reasons beside each decision.

Place: Track D (the desk). Time: about 50 minutes. Crates: kazoo-core (`host/audio.rs`, the input choice), kazoo-mouth (`run.rs`, the status line, the output churn), kazoo-mix (`run.rs`, the refusal list), kazoo-wall (the desk link's log).

## Why

Tom, 14 Oct 2026, about 23:47: "the mixer doesn't seem to be working... when I turn the mixer on, it's not hooking up the synth or anything like that."

What happened (system audio log and process times):
- AirPods connected at 23:44:40 and became the default output and input.
- `kazoo mouth` (23:45:31) opened the AirPods' output at 48 kHz, then their mic a second later. Opening a Bluetooth headset's mic puts it in its two-way (HFP) mode, and its output drops to 24 kHz (log, 23:45:32 to :36).
- `kazoo mix` (23:45:57) opened the default output, now at 24 kHz. The desk refuses any instrument at another rate (`kazoo-mix/src/hub.rs:778-784`), so the mouth (48 kHz) and the wall (48 kHz, on the laptop speakers) were both turned away.
- Nobody could see why. The desk shows a refusal in its header for 8 seconds only (`kazoo-mix/src/run.rs:26`, `:225-239`). The mouth's status says "playing direct: no desk" whether or not the desk refused it (`kazoo-mouth/src/run.rs:587-600`), though the link keeps the reason (`LinkStatus::last_refusal`, `kazoo-core/src/ipc/link.rs:432`). The wall logs nothing.

Also seen: the mouth sets up new CoreAudio output units several times a second for as long as it runs (system log, pids 18354 and 13616): something re-opens or re-queries the device every frame.

## Outcome

1. **The mouth never opens the mic of the device it plays through, unless told to.** When no `--input` is given, and the default input device has the same name as the output device the mouth plays on (a headset: AirPods, any Bluetooth set), the mouth opens the built-in mic instead: the first input device whose name contains "MacBook" or "Built-in", else the first input device that is not the output's. Its header says so: `mic: MacBook Pro Microphone (not the AirPods' mic: it would drop them to phone quality)`. `--input` naming the headset opens it as asked, and the header then says `mic: AirPods (they now play at phone quality)`. No other input exists: open the headset's mic, and say so the same way. Decision: a sung note through a phone-quality mic is still a note, but a desk that goes silent is a broken instrument; the choice is always visible and always overridable.
   - The choice is a pure function in kazoo-core (`host/audio.rs`), `fn choose_input(default_input: Option<&str>, output: &str, inputs: &[String]) -> InputPick`, with `InputPick { name, why: Option<String> }`, so it is tested without devices.
2. **A refusal says why, where the player looks, for as long as it lasts.**
   - Mouth: while its desk link has a `last_refusal` newer than its last connection, the status line says `not on the desk: <the reason>` (e.g. `not on the desk: it runs at 48000 Hz, the desk at 24000 Hz (use the same device)`) in place of `playing direct: no desk`.
   - Desk: refused instruments stay listed under the strips, `refused: mouth (48000 Hz, the desk runs at 24000 Hz)`, one line each, newest first, until that instrument connects or it has not tried for 60 seconds. The 8-second header notice stays as it is.
   - Wall: the daemon logs the first refusal of each distinct reason to `daemon.log` (`kazoo-wall: the desk refused the wall: <reason>`), and again only if the reason changes or after a connection.
3. **The mouth's output is opened once.** Find what re-creates CoreAudio output units several times a second (the agent's evidence: the system log lines `AudioUnit ... created` under the mouth's pid, about every 200 ms) and stop it: the device is opened at start, and again only when it is lost or changed (V11's supervisor). Report what it was, with file and line.

## Tests (write them first; each fails before the change)

- kazoo-core: `choose_input` picks the built-in mic when the default input and the output share a name; keeps the default input when they differ; honours an explicit `--input` of the headset with the phone-quality note; falls back to the headset (with its note) when nothing else exists; never panics on empty lists.
- kazoo-mouth: the status line reads `not on the desk: <reason>` given a link status with a refusal newer than its last connection, and `playing direct: no desk` given none.
- kazoo-mix: a refused instrument is listed with its reason, still listed at 59 s without a retry, gone at 61 s, and gone at once when it connects.
- kazoo-wall: two refusals for the same reason log one line; a different reason logs another.
- kazoo-mouth, the churn: whatever was found in Outcome 3 has a test that would have caught it (e.g. the device is asked for once over 100 frames).

## Not in this lane

- The desk running at a rate other than its device's, or resampling a strip that arrives at another rate. That is the deeper fix (the desk never refuses for rate) and goes against CLAUDE.md task 10 ("native sample rate"); it is its own brief, written after this one lands.
