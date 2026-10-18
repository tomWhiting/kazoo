---
name: kazoo
description: Play kazoo, the terminal music studio. Covers the wall (a shared modular synth that never stops, played through the kazoo MCP's wall_* tools), its tree of every moment, the desk, and the instruments (808, 303, CS-80, Minimoog, Juno, Prophet-5, DX, string, glass, arp, the mouth synth). Use when someone wants to make music with kazoo, join or play the wall, patch modules, write a phrase for an instrument, or learn modular synthesis.
---

# kazoo

Kazoo is a music studio that lives in the terminal. At its centre is
**the wall**: one modular synth, shared by everyone on it, that plays
forever. People and Claudes reach up at any time to turn a knob, move a
cable, or add or take away a module. Nothing is automated, and nothing about
how it should sound is decided in advance. Every change to the sound is logged
under the name of whoever made it, and can be undone.

## Joining

The kazoo MCP is the same for everyone, with no name in it. To play:

1. `wall_walls`: the walls you can reach, and who is on each.
2. `wall_join {as: "<a name you choose>"}`: join under a name of your own.
   Pick something with character.
   - A name is 1 to 24 ASCII letters, digits, spaces, `_`, `.` or `-`, and
     is compared without case.
   - The wall's own names (`desk`, `kazoo-wall`) are always refused, and so
     is a name someone on that wall has now.
   - Joining a wall you are on already is refused: `wall_leave` first.
   - A wall that is not playing can still be joined: you are on it as soon
     as it starts.
3. `wall_look`: everything on the wall now. **Always look before you
   change anything**, and look again whenever someone else has changed
   something. Their changes come to you as channel notes, but those are
   hints about the past.
4. `wall_leave` when you're done.

Every tool but `wall_walls` takes an optional `wall`. `wall_join` needs it
when `wall_walls` lists more than one wall; the others need it when you are
on more than one.

If no wall is running, say so. A person starts one with `kzoo wall`. It is
silent until someone at its console presses `m`, and only a console can
turn the sound on.

## Playing

- **Ports** are `module.port`: outputs like `lfo1.out`, inputs like
  `vcf1.in`. Every knob is a jack too, so `vcf1.cutoff` takes a cable.
- **`wall_turn`** glides a knob over `glide_beats` (2 by default, up to 64).
  `after_beats` waits before it starts, and `curve` shapes it (`linear`,
  `ease_in`, `ease_out`, `s_curve`). Glide: a change over 8 to 16 beats is
  weather, a jump is a crash.
- **`wall_patch` and `wall_unpatch`** move cables. A cable's amount runs
  from −1 to 1. `after_beats` (0–256) holds a new cable back, and
  `fade_beats` (0–64) fades it in from silence. Patching into an input that
  is already used replaces that cable.
- **`wall_add` and `wall_remove`** put modules up and take them down.
  `wall_catalogue` lists every kind, with exact knobs and ranges.
- **`wall_batch`** makes several changes (`add`, `remove`, `turn`, `patch`,
  `unpatch`, `tempo`) as one, in order, and every step succeeds or none do.
  - It needs a `note`: one line, 1 to 200 characters, like a commit message.
  - An `add` can carry `as: "label"`, and later steps can name it as
    `@label` (and its ports as `@label.out`).
  - A Markdown `description`, `attachments`, and a `comment` on each step
    are optional.
  - The tree shows the batch as one knot (❖) with its note. `wall_notes
    {seq}` reads back its note, description and attachments.
- **`wall_tempo`** sets the tempo, held to 20–300 BPM. `after_beats` waits
  first, and `glide_beats` moves it gradually. While the wall follows the
  desk, the desk is asked instead and the tempo changes for everyone at
  once: a `glide_beats` is then refused.
- **Grouping and reuse:** `wall_group` puts modules in a named group (or
  takes them out, with `leave: true`). Grouping is not logged and not
  undone. `wall_chain` saves a chain of modules, adds copies of it, and
  lists the saved chains.
- **Speech and loops:** `wall_speak` gives a `speak` module words, and
  `wall_loop` erases a `loop` module's tape.
- **`wall_record {on: true}`** records what the wall plays to
  `~/Music/kazoo-wall`, even while it is silent, and `{on: false}` stops.
- **Listening:** `wall_listen` says what the wall sounds like now. To
  measure something, `wall_add` a `scope` or `readout`, patch a signal
  through it, and `wall_probe` reads it.
- **History:** `wall_log` lists recent changes with their numbers.
  `wall_undo {change: N}` undoes change N, whoever made it, as a new change.
- **Signals:** pitch is 1.0 per octave, 0 = C4. Gates are high above 0.5.

## The tree

The wall's past is a tree, and nothing in the past ever changes.

- `wall_tree` shows its branches, marks and recent moments.
- `wall_mark` names the moment the wall is at. Mark the good bits.
- `wall_go` puts the whole wall at a moment: a mark, a branch, `#<change>`
  or a commit. Every module is new afterwards, so `wall_look` again before
  changing anything.
  - On a branch, the next change carries the branch on.
  - Off any line, the next change anyone makes grows a new branch (a bud)
    from there.
  - `wall_undo` of the go brings the wall back exactly.

A person walks the tree with `kzoo tree`.

## Saving

`wall_save` keeps the wall as it is, under a name. `wall_saves` lists the
saves, and `wall_load` puts one up while the wall plays. The wall as it was
is saved first, so `wall_undo` of the load's change brings it back. Once a
save is loaded, every module is new: call `wall_look` again before changing
anything.

## How to be a good player

Read [references/modular.md](references/modular.md) for the patches every
player should know, and [references/aphex.md](references/aphex.md) for ways
of working worth stealing. The short version:

- Look, change one thing, listen.
- Glide. Leave room. Take something away before you add something.
- Don't undo or fight someone else's gesture. Answer it.
- Mark what's good, and branch to try something wild.
- Say what you did and why in each change's `comment` (1 to 200
  characters): that is what the log is for.

## The rest of the studio

[references/instruments.md](references/instruments.md) covers the desk
(`kzoo mix`) and every instrument: what each is, how to play it, and how it
joins the desk. [references/wall.md](references/wall.md) covers the wall's
modules in detail, and its console.

Everything kazoo does happens in a terminal, and the commands are all
`kzoo <something>`. `kzoo --help` lists them, and `kzoo setup` sets it all
up.
