# KZ-C10: parameter changes recorded against the timeline and played back

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md with `-p kazoo-mix -p kazoo-core`. Depends: KZ-C05 (signal recording), KZ-C08 (sessions), KZ-B06 (ParameterChange).

## Outcome

Move a fader, a pan or an instrument's knob while recording, and the move is kept on that strip's lane and played back on the same frames next time, so a mix can be performed and then repeated.

## The design (decided; build exactly this)

- Record: when a strip is armed and the transport is recording, every `ParameterChange` that touches that strip (desk params: fader, pan, mute, send, trim, EQ; instrument params through KZ-B06) is appended to the strip's signal file from KZ-C05 as an event of kind `param` with the stream frame, param index and value; the desk's own control moves are turned into the same events at the point they are applied in the engine (read where fader glides start in `engine.rs`).
- Play: the take player (KZ-C06) reads param events with the same cursor it reads notes, and on each frame applies them: desk params directly (through the same 5 ms glide the desk already uses), instrument params by sending `ParameterChange` down the link with `glide_ms = 0`.
- Modes per strip: `off`, `read`, `write` (shown as a letter on the strip; keys follow the strip's existing key conventions). `write` records; `read` plays; a manual move during `read` wins until the next recorded event (no latch, no touch: decided for scope).
- Session JSON (KZ-C08) gains the per-strip mode; the events live in the signal file already.

## Not this lane

Drawing or editing automation curves in the timeline view (a later lane); note recording (KZ-C05).

## Red tests first

- `automation::tests::a_fader_move_while_writing_is_recorded_with_its_frame`.
- `automation::tests::read_applies_the_recorded_value_on_the_same_frame` (fake clock; value asserted at frame N and unchanged at N-1).
- `automation::tests::manual_move_during_read_wins_until_the_next_event`.
- `automation::tests::instrument_param_is_sent_down_the_link` (test link end receives the ParameterChange at the frame).

## Definition of done

- [ ] Four red tests then green.
- [ ] By hand: write a fader ride on one strip, play it back, watch the fader move (say what you saw).
