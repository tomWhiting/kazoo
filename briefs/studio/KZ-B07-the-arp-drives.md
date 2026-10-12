# KZ-B07: the arp sends its notes to the hub, so it can play any synth

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md with `-p kazoo-arp`.

## Outcome

The arpeggiator drives the other instruments through the desk, the way `kazoo-play --jam --drive` already does. Today it never sends a note (survey: instruments.md).

## The design (decided; build exactly this)

- Read `kazoo-play`'s `--drive` path (find the code that sends `NoteEvent` frames to the hub with stream-frame timestamps) and the arp's note scheduler (the struct that produces the next note on each step).
- The arp's scheduler output becomes a `NoteEvent { note, velocity, on: bool, at: StreamFrame }` sent through the link (`kazoo_core::host::link::Linked::tx` events side, or the existing note send if KZ-A03 has not landed: say which under FOUND) at the step's stream frame, computed from the desk transport via `TransportFollower` so the arp stays on the desk's grid.
- A `--target <instrument>` flag (default: all) names the receiving instrument; the hub already routes notes between instruments (survey: hub.md, item 7).
- The arp keeps its own internal sound if it has one; a `--silent` flag mutes it when it is only driving.

## Not this lane

Note recording (KZ-C05). Hub routing changes (none needed).

## Red tests first

- `arp::tests::each_step_emits_a_note_on_and_off_at_stream_frames` (fake transport at 120 bpm; eighth notes land 24,000 frames apart at 48 kHz).
- `arp::tests::latch_holds_notes_while_keys_are_up`.
- `arp::tests::target_flag_sets_the_destination`.

## Definition of done

- [ ] Three red tests then green.
- [ ] By hand: desk up, `kazoo mini`, `kazoo arp --target mini`, hold three keys, the mini plays the pattern (say what you heard).
