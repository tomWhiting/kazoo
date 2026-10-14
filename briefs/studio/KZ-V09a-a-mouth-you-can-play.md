# KZ-V09a: a mouth you can play: every control in reach, by key and by mouse

Line numbers are from `main` at the head this lane starts from; find each item by what it does, and never rename anything. Checks, code quality and the END block are as PLAN.md's STACK RULES, with `-p kazoo-mouth`.

Place: Track V, after V08a to V08d. Written by Cassio, 14 Oct 2026, 20:0x, from Tom's words after playing the build-round mouth: "it sounds really good... it's just the interface now"; "I can't change the waveform. I can't get to the different effects. I don't know that the effects are doing anything. The help menu is just a wall of text"; "I'd really want to be able to fuck around with stuff as I go really easily... having mouse and stuff in there would be helpful"; the gate "not the easiest thing to figure out". Decided by Cassio under Tom's rule that internal calls are not parked on him.

Time: about 45 minutes. Crates: kazoo-mouth (`ui.rs`, `ui/*`, `controls.rs`, `controls/*`, `run.rs`), kazoo-core only if a gate setting must be exposed (see Gate). Depends: KZ-V08a to V08d (on main).

## Why

Today's screen is a list: Up/Down walks twelve synth controls, Tab swaps to an effects list, `[ ]` walks slots, and nothing on screen says which of these exist or what they are doing. The waveform (`Control::Shape`) is the eighth row of a list Tom never saw past. The effects have no light showing whether they are on or touching the sound. The help is every control's paragraph in one scroll.

## The design (decided; build exactly this)

The screen becomes a panel board: every control visible at once, each a labelled dial or switch, reachable by a mouse click or by one or two keys, and every effect showing whether it is working.

1. **Layout** (from top): a header line (input device and rate, desk or direct, tempo, latency placeholder `--` filled by V09b); the **voice strip** (the pitch trail as today, beside a level meter for the mic and one for the output); then four panels side by side, each boxed with its name: **SOURCE** (waveform, cutoff, air, volume, silent), **PITCH** (quantise on/off, scale, root, snap, vibrato, hysteresis, blend), **GATE** (open threshold, close threshold, hold; see Gate), **FX** (the five slots); then the looper strip (V08d's, unchanged); then one footer line of the eight most used keys. At under 100 columns or 30 rows the panels stack two by two; under 60 columns one per row, scrolling with the focus. Nothing is drawn outside the terminal.
2. **Waveform** is a row of named buttons, one per shape the synth has (sine, triangle, saw, square and any others `SynthSettings` holds), the current one lit; `w` steps to the next shape from anywhere; a click picks one.
3. **Dials**: each continuous control draws as `name ▕████░░░░▏ value unit`. Focus moves with arrows inside a panel and Tab / Shift-Tab between panels; Left/Right turn the focused dial (Shift for the big step, as today); `d` restores its default. Mouse: clicking a dial focuses it; clicking at a point along its bar sets the value there; the scroll wheel over a dial turns it one step per notch; dragging along the bar sets it continuously. Switches (quantise, vibrato, silent, an effect's on/off) toggle on click or Space.
4. **Effects panel**: the five slots in chain order, each a line `[●] megaphone   ▁▃▅▇ mix 0.40` with: an on/off light (lit when on), the effect's name (a click on it opens that slot's knobs below the list, in the same dial form), and an **activity meter**: the difference in level between that slot's input and its output over the last 100 ms, so a slot that is on but changing nothing shows a flat meter. The engine publishes per-slot input and output RMS through the existing display/gauge path (no allocation, no lock; atomics or the existing ring). `1`..`5` select a slot; `b` toggles it; `{ }` move it in the chain, as today.
5. **Help** (`?`) becomes a grouped, short card: one heading per panel, under each only `key: what it does` lines of at most 50 characters, no paragraphs; a line at the bottom `Enter on a control: what it does`. A control's long description appears in the footer line while that control has focus, not in the help card. `/` in the help card filters its lines by typed text (case-insensitive), Esc clears or closes.
6. **Mouse capture** is enabled on start and released on every exit path (normal, error and panic), alongside the existing terminal restore.

## Gate

The synth's voice gate (`synth.rs`'s `gate` and `gate_open`, from the tracker's voicing) gets three user controls with ranges and defaults decided here: **open** threshold in dBFS of input level, -70 to -10, default the level the current code effectively uses (measure it and say so in the END block); **close** threshold, -80 to -15, default open minus 6 dB, always held at least 3 dB below open; **hold**, 0 to 300 ms, default 40 ms. The GATE panel shows a small light that is lit while the gate is open, and the mic meter draws the open and close lines on its bar. If the gate's level test lives in kazoo-core's voice path, add these as settings there with the same ranges, defaults and clamping, and keep every existing kazoo-core test green.

## Not this lane

The scope, the latency readout (V09b), the vocoder blend (V09c), the looper's own controls (unchanged), any change to how the synth or effects sound.

## Red tests first

In `ui/tests.rs` and `controls/tests.rs` (create if absent), with ratatui's `TestBackend`:

1. `every_panel_is_on_screen`: at 120×40 the rendered buffer contains SOURCE, PITCH, GATE and FX panel titles and every control's name.
2. `the_waveform_is_one_key_away`: `w` from any focus changes the shape to the next one; the lit button moves.
3. `a_click_sets_a_dial`: a mouse-down event at 75% along the cutoff bar sets cutoff to within one step of 75% of its range.
4. `the_wheel_turns_a_dial`: two wheel-up notches over volume raise it by two steps.
5. `an_idle_effect_shows_flat`: an effect slot that is on with mix 0 shows an activity meter at its lowest cell; with mix 1 and a signal through it, a higher cell.
6. `help_is_short_and_grouped`: every help line is at most 50 characters; each panel name is a heading; `/` then `gate` leaves only lines containing "gate".
7. `the_gate_holds_its_order`: setting close above open holds close at open minus 3 dB; the panel shows the clamped value.
8. `small_terminals_stack`: at 80×30 and 50×20 nothing is drawn outside the area and the focused control is visible.
9. `mouse_is_released_on_exit`: the exit path (normal and error) disables mouse capture (assert through the terminal-guard abstraction already used for restore).
10. `the_meters_never_allocate`: per-slot RMS publication inside the engine's existing `assert_no_alloc` render test.

## Checks

Every cargo command runs under the kazoo cargo lock, niced, with four jobs. Read `df -g /` and `uptime` before each and put them in START and END.

```
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo nextest run -p kazoo-mouth --build-jobs 4 -j 4
nice lockf -k /Users/tom/.kazoo-cargo.lock cargo clippy -p kazoo-mouth --all-targets -j 4 -- -D warnings
cargo fmt --all --check
ast-grep scan kazoo-mouth
```

If kazoo-core changes (Gate), also run its nextest and clippy.

## Definition of done

- [ ] Each red test fails first (paste it), then passes.
- [ ] Every kazoo-mouth test passes; clippy, fmt and ast-grep exit 0.
- [ ] The gate's measured current threshold, stated.
- [ ] Cassio's and Tom's own try with a mouse, after landing (NOT CONFIRMED until then).

## END

```
START KZ-V09a base=<sha> branch=lane/kz-V09a df=<GB> load=<1m> at <time>
END KZ-V09a head=<sha> base=<sha> pushed=lane at <time> minutes=<n>
DONE: each definition-of-done line with its proof
RED: each test's name and its failing output before the fix
TESTS: nextest counts; clippy, fmt and ast-grep each with their own exit code
BENCH: none asked
CHANGED: files with +/-
FOUND: anything wrong you saw and did not fix, with file:line
NOT CONFIRMED: what was not proven
BLOCKED ON: step, what you tried, time spent
If this code were used in a hospital, the worst credible failure is ___, and it could harm about ___ people.
```
