# KZ-T10b: the console sends a batch with a note: `:batch <note>` holds Tom's changes instead of sending them, shows them held, and Enter sends them as one batch (KZ-T10a)

Line numbers are from main at `e905d15` and may have moved; find each item by what it does, never rename. Names from KZ-T10a (`Request::Batch`, `as`, `@label`) are as that brief names them; use what landed. Checks, code quality and the END block are as PLAN.md's STACK RULES, with `-p kazoo-wall`.

Written by Cassio, 15 Oct 2026, 00:3x. Decided by Cassio; reasons beside each decision.

Place: Track T, T10. Time: about 45 minutes. Crates: kazoo-wall (`tui/app/command.rs`, `tui/app/mod.rs`, `tui/app/keys.rs`, `tui/draw/panes.rs`, `tui/draw/rack.rs`, `tui/log.rs`, their tests). Depends: KZ-T10a has landed.

## Why

Tom, 14 Oct 2026, about 22:50: batches are "several changes sent at once with a note (like a commit message)", from the seats or from him. KZ-T10a gives the wall the batch and the seats `wall_batch`; this lane gives it to Tom's console.

## Outcome

1. **`:batch <note>` starts a batch.** The note is checked as the wall checks it (1 to 200 characters, one line); a bad note is refused in the command line with the reason. `:batch` with no note while a batch is held shows what is held.
2. **While a batch is held, the console's changes are held, not sent.** Every request the console would push as a change (`Turn`, `Patch`, `Unpatch`, `Add`, `Remove`, `Tempo`; `mod.rs:1178` lists them) goes into the held batch in order instead of `jobs`. A turn of a knob already turned in the held batch replaces that step's value and glide in place (a drag sends many turns; the batch keeps the last). Undo, loads, goes, saves and chains are refused while a batch is held, with `send or drop the batch first (Enter or Esc)`. Moves and groups (not changes) go at once as today.
3. **Held is visible.** The header shows `BATCH "<note>" · <n> held · Enter sends · Esc drops` in the warning colour. A knob turned in the batch shows its held value with a `•` beside it; a held add shows as a module card marked `held`, with its id shown as its label (`@lfo1`, the kind plus a count within the batch, sent as T10a's `as`), and the console's completions and patching offer it, so a held patch can name `@lfo1.out`. A held remove dims the module. The log pane lists the held steps under the note, marked held.
4. **Enter sends, Esc drops.** Enter (when the command line is not open) sends one `Request::Batch { note, changes }`. On success the held marks clear and the log shows the batch as the wall logged it. On a refusal the batch stays held and the header shows the wall's reason with its step, so Tom can fix the step or drop the batch. Esc drops the held batch (the header says `dropped "<note>": <n> changes`). Quitting with a batch held asks once (`a batch is held: Enter sends, q again drops it`). Decision: held is git's staging; the note is the commit message, and nothing reaches the wall until it is sent whole.
5. **Help** lists `:batch`, Enter and Esc with a batch held.

## Tests (write them first; each fails before the change)

- `:batch brass breathes`, two turns of different knobs, Enter: one `Request::Batch` with both turns in order; nothing was pushed before Enter.
- Twenty turns of one knob while held leave one step with the last value.
- An add then a patch from its `.out` while held: the batch carries `as: "lfo1"` on the add and `@lfo1.out` on the patch.
- Esc sends nothing and clears the marks; an undo while held is refused with the message; a move while held is sent at once.
- A refused batch stays held and the header names the step and reason.
- The header, the `•` mark, the held card and the log's held lines draw at 80 columns (buffer tests as the console's other draw tests do).
