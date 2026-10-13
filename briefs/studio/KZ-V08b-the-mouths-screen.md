# KZ-V08b: the mouth's screen: the note you are singing, where it is being pulled, a trail of the last four seconds, and every quantiser and synth control on keys

Line numbers are from the sweep that wrote this brief, at `fa3c8d8`, and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md's STACK RULES, with the `-p` list below.

Crates: kazoo-mouth. Time: about 30 minutes. Depends: KZ-V08a.

Read by Cassio, 13 Oct evening. Changes: the checks take the lane form, the listen is Cassio's and Tom's after landing, and the trail's height is stated plainly. The 4 s trail lives in the UI's own deque; V08a's 1.4 s display ring only has to cover one redraw (33 ms), so it is enough. Opens only after V08a has landed.

Why: Tom gets frustrated when a program does not show what it is doing (his standing note: "always show what's happening"). Singing into a quantiser is only playable if you can see the note you are on, the note it is pulling you to, and how far off you are, as you sing. And every quantiser setting Tom asked for (scale, root, snap speed, keep vibrato, hysteresis, blend between direct and snapped) needs a key, so nothing has to be set by a flag.

## Outcome

The mouth's screen shows, live at 30 Hz: the sung note and the snapped note in large text with cents and Hz; a 4-second pitch trail with the direct pitch dim and the snapped pitch bright over the scale's note lines; gate, loudness, breath and confidence; a controls panel; the link state; and a footer of keys. Every control changes the sound at once.

## The design (decided; build exactly this)

### Layout (ratatui; new `src/ui.rs` and `src/controls.rs`; `run.rs` keeps the loop)

Top to bottom, full width, minimum terminal 80×24 (smaller: one line "make the terminal at least 80×24", nothing else drawn):

1. **Header (1 line):** `kazoo mouth` · link state (V08a's three texts) · `quantise on: major D, snap 30 ms` or `quantise off: hertz for hertz` · `silent` when silent.
2. **Pitch (5 lines):** left half: the sung note as big text (ratatui `BigText` is not in the tree; draw the note name in a 3×5 block font defined in `ui.rs` for `A`–`G`, `#`, digits and `-`), then `+12¢  443.1 Hz` beneath (cents from the nearest equal-tempered note, Hz to one decimal); right half: the snapped note the same way, then `→ A4  440.0 Hz`. Unvoiced: both dim, showing the held notes. Gate open: the header's left mark is a filled dot; closed: hollow.
3. **Trail (every line the other panels leave, which is 9 at 80×24):** a braille `Canvas` of the last 4 s from the display ring (V08a), x = time (newest at the right), y = pitch in semitones over a window of ±7 semitones around the median of the visible voiced frames (recentred when the median moves by more than 3 semitones, so the view does not swim); direct pitch dim, snapped pitch bright; when quantise is on, a faint horizontal line at each scale note in the window, the root's line labelled with its name.
4. **Meters (2 lines):** `loud ████░░  breath ██░░░░  sure █████░` (each 0..1 across 16 cells) and the gate as `gate ●`/`gate ○`.
5. **Controls (6 lines, two columns):** the selected control highlighted; each shows its name and value:

| Control | Range | Step (Left/Right) | Shift step | Default |
|---|---|---|---|---|
| quantise | off / on (blend 0 ↔ last non-zero blend, 1 at start) | toggle | toggle | off |
| scale | `SCALE_NAMES` | 1 | 1 | chromatic |
| root | `NOTE_NAMES` | 1 | 1 | C |
| snap | 0–400 ms | 5 ms | 50 ms | 30 ms |
| keep vibrato | off / on | toggle | toggle | on |
| hysteresis | 0–50 cents | 5 | 25 | 30 |
| blend | 0–1 | 0.05 | 0.25 | 1 (when on) |
| shape | sine, triangle, saw, square | 1 | 1 | saw |
| cutoff | 100–12,000 Hz | ×2^(1/12) | ×2 | 2400 |
| air | 0–1 | 0.05 | 0.25 | 0.25 |
| volume | 0–1 | 0.05 | 0.25 | 0.7 |
| silent | off / on | toggle | toggle | off (or `--silent`) |

   Why these steps: snap in 5 ms is finer than anyone hears; cutoff a semitone per press is how every synth on the desk moves a filter; the shift steps cross the range in a handful of presses.
6. **Footer (1 line):** `↑↓ choose  ←→ change (shift: big)  space quantise  v vibrato  s silent  ? help  esc quit`.

### Keys

`Up`/`Down` select (wrapping); `Left`/`Right` change, `Shift+Left/Right` by the shift step; `Space` quantise on/off; `v` keep vibrato; `s` silent; `?` a help overlay listing every key and what each control does (one sentence each, from the table's meaning); `Esc` closes the help, else quits; `Ctrl-C` quits. Every change builds the whole `QuantiseSettings` or `SynthSettings` and sends it with `try_send` on V08a's command channel; a full channel (64 waiting) shows `busy: change not sent` in the footer for 1 s and the screen's value is put back to what the engine has (the UI keeps the last value the engine accepted, so screen and sound never disagree).

### Display data

The UI drains the display ring (V08a) each redraw into a `VecDeque<VoiceFrame>` capped at 4 s of frames (`rate · 4 / VOICE_HOP`, 3000 at 48 kHz), allocated once at start with that capacity.

## Not this lane

Saving settings (V08a decided against it). Showing who follows (the desk shows it, V07b).

## Red tests first

`TestBackend` renders of `ui::draw` with a hand-built state; key handling as pure functions on an `App` with a fake command sender. Red for all: the screen does not exist (V08a's status screen has none of these).

- `the_pitch_panel_shows_sung_and_snapped`: a frame at A4 + 12 cents snapped to A4: the buffer contains `+12¢`, `443.1 Hz` (A4 + 12 cents is 443.06 Hz; the test computes it with `hz_from_volts` and formats it as the screen does), `→ A4` and `440.0 Hz`.
- `unvoiced_is_dim_and_held`: the last voiced note's name is drawn in the dim style.
- `the_trail_draws_four_seconds`: 3000 frames of a slow slide: the canvas has points across its full width; with quantise on, the root's line label appears.
- `every_control_moves_within_its_range`: for each control, Left at its minimum stays at the minimum, Right at its maximum stays at the maximum, Shift steps as the table; each change sends exactly one command with the whole settings struct.
- `space_remembers_the_blend`: blend 0.6, Space (off: sends blend 0), Space (on: sends 0.6).
- `a_full_channel_says_busy_and_keeps_the_engines_value`.
- `a_small_terminal_says_so`: 60×20 renders only the size message.
- `the_footer_and_help_name_every_key`: the help overlay lists every key in the Keys section.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -h /System/Volumes/Data` before each and put it in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-mouth --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-mouth --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-mouth
```

## Definition of done

- [ ] All red tests red first (paste), then green.
- [ ] Not the builder's: after landing, Cassio installs and asks Tom to sing a scale into `kazoo mouth` with quantise off, then on in major, and to say whether the trail shows the voice wandering and the snapped line stepping. The builder never opens Tom's mic or speakers, lists this under NOT CONFIRMED and does not claim it.

## Code quality

As PLAN.md's STACK RULES. The UI thread may allocate; it never blocks the audio side (`try_send` only, display ring read only). The block font is a `const` table. Files under 1000 lines (`ui.rs` splits into `ui/pitch.rs` and `ui/trail.rs` if it nears that).

## END

```
START KZ-V08b base=<sha> branch=lane/kz-V08b df=<GB> at <time>
END KZ-V08b head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
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
