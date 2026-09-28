# Rack view: plan (approved by Tom 2026-09-27)

Tom's decisions: all recommendations accepted. Shared rows in the patch (A2), moves out of the change log (z undoes your own last move), first arrangement one row per picker group, seat-added modules land at the end of their group's row, density and hidden log per session only, compact shows every knob's value.

I've mapped today's rack view, worked out a design for each of Tom's three asks, and put together a six-step build plan. Five questions need his answer. Everything below is based on the code as it stands, and I changed nothing.

## 1. What the rack view does today

**Layout.** Plates are never stored anywhere: not in the patch, not in the daemon. The layout is rebuilt from `snapshot.modules` on every frame, every 50 ms (`draw/rack.rs:116-119`, `tui/mod.rs:58`).
- **Order:** plates follow the order modules were added (`protocol/mod.rs:688`, `patch.rs:549`).
- **Undoing a removal:** `Patch::restore` pushes the module to the end (`patch.rs:642`), so its plate jumps to the end of the rack.
- **Plate size:** `plate_size` (`rack/geometry.rs:212-216`) sets the width from knob columns (8 cells each), three knobs to a column. The height is 2 (border) + 2 (title) + 3×5 (knobs) + 2 per line of jacks. A typical plate is 23 to 27 rows tall and 14 to 34 columns wide.
- **Rows:** the number of rows is whatever fits the screen height (`geometry.rs:227`). The plates are split in order into rows of about the same width (`:234-248`), and every plate is stretched to its row's tallest (`:250-253`).
- **Result:** on a normal terminal the rack area is about 40 rows, so there is exactly one row. 60 modules make a strip about 1,200 columns long, and only about a sixth of it is on screen. The layout also reflows on every resize and every add, so plates move around.

**Panning.** The `Window` (`draw/rack.rs:36-80`) maps sheet cells to screen cells with an offset, `pan`.
- **After each frame:** `settle_pan` (`:165-194`) clamps the pan to the sheet (`geometry.rs:443`). If `follow` is set, it scrolls just far enough to show the selected plate, or the selected knob if the plate is taller than the view (`geometry.rs:455`).
- **Keys:** every key sets `follow` (`app/keys.rs:34`). There are no keys that pan directly: h/l move the selection by snapshot index (`keys.rs:228-241`).
- **Mouse:** left-dragging a plate body or bare rack pans (`app/mouse.rs:239-254`, `:370-376`). The wheel pans 3 rows, or 8 columns sideways with shift or when the sheet is no taller than the screen (`:508-526`). Horizontal scroll pans too (`:181-182`).
- **Edges:** arrows at the edges show where the rack runs on (`draw/rack.rs:617-638`).

**Bug found.** In the rack view, j/k past the last knob jumps `index ± self.columns` (`keys.rs:261-273`). But `columns` is only ever set by the list view (`draw/wall.rs:71`), so it moves by a stale value.

**Zoom and space.** There is no zoom or density setting; every size is a `const` (`geometry.rs:20-48`, `dial.rs:10-13`). The rack view already uses the full width with no cable pane (`draw/mod.rs:72`), but the log still takes 4 to 8 rows (`:50-53`), plus 2 rows of header and 1 of status.

**Adding a module.** This is the same from either view.
- `a` opens the picker only if the catalogue has arrived (`keys.rs:450-460`).
- The picker is a list in group order (`app/mod.rs:572-580`, `group_of` at `:1066-1088`, drawn by `overlay.rs:156`). There are about 64 kinds and no filtering.
- j/k move and Enter sends `Request::Add { kind, name: None }` (`keys.rs:462-498`). `n` asks for a name first (`:500-533`).
- The daemon appends the module (`daemon/wall.rs:861-868`), the reply selects it (`app/mod.rs:797-799`, `:691-699`), and the empty rack says "press a" (`draw/rack.rs:125`).
- **Bug found:** the new plate is not scrolled into view. `follow` was used up by the Enter key's frame before the new snapshot arrives.

**Removing a module.** `x` counts its cables and asks, and y or Enter sends `Request::Remove` (`keys.rs:535-565`, `overlay.rs:220`). The mouse cannot remove.

**Cables.**
- `cable_ends` finds both sockets by splitting strings and searching modules linearly (`geometry.rs:418-438`).
- `cable_path` draws a curve that sags between the two sockets, in braille dots; longer cables hang lower (`:379-413`).
- Cables are drawn in id order, only on blank cells or on top of other cables, merging dots, in the fingerprint dye colour or a colour by id (`draw/rack.rs:543-614`, `:92-99`). A map of which cable is in each cell is kept for right-click.
- Costs: every cable's path is rebuilt every frame, even off-screen, and every plate is drawn even off-screen (`:153-157`). `plugged()` scans all cables for each jack (`:322-336`).

**What the mouse does** (`app/mouse.rs:1-12`, `:141-190`):
- Clicking selects a plate or knob.
- Dragging a knob up or down turns it; shift makes it fine. Turns are sent at most every 100 ms with a 0.125-beat glide.
- The wheel over a knob turns it finely, and a double-click sends it back to its default.
- Dragging from a jack to a jack or knob patches.
- Right-clicking a cable, or the input it is plugged into, unplugs it.
- Dragging a plate body or the background pans.
- The middle button and bare movement do nothing. Outside the rack view, and while any prompt is open, the mouse is ignored.

## 2. Designs

### More on screen
- **Density levels, a console-only setting (`c` cycles them).** Replace the constants in `geometry.rs` with a `Scale` value passed to `plate_size` and `place`. Hits and sockets need no change, because they come from the placed plates.
  - **Full:** exactly today's look.
  - **Compact:** a 4×2-cell dial (8×8 braille dots, still round), name and value lines, 6-wide knobs, one-line jacks (`(●)out`), one title line. Plates are about 15 to 17 rows tall, so two rows fit where one does now, and about 30% narrower.
  - **Overview:** each module is a small cream block about 12×5 (name, kind, a strip of single-glyph sockets), with the cables still hanging between them. 60 modules fit on one 200×50 screen. Clicking a plate returns to the previous density, centred on it.
  - `dial.rs` needs the dial size made a parameter (const generics or sized arrays).
- **Hide the log in the rack view (`f`).** That gives back 4 to 8 rows. The status line stays.
- A separate minimap is not worth it; the overview level does that job and keeps the look.

### Panning
The mouse side is mostly there already. Add:
- **Keys:** Shift-arrows pan a quarter of the view; `.` centres on the selected plate; `g` opens a prompt to jump to a module by id, name or kind (`Mode::Jump { text }`).
- **Mouse:** middle-drag pans from anywhere, including over knobs.
- **Edge auto-pan:** carrying a cable or a plate near an edge scrolls the rack, driven from `App::tick` using `rack.area` and the pointer.
- **Visual navigation:** h/l follow the order on screen, and j/k past the last knob go to the nearest plate in the row below or above. This replaces the stale `columns` use.
- **Follow after add:** set `rack.follow = true` when the new module is selected, so it comes into view.
- Once rows are explicit (next section), the sheet can be taller than the screen, so vertical panning becomes real.

### Rearranging: where positions live
The candidates were (A1) a logged, undoable `What::Move` change; (A2) rows kept in the patch and shared, but outside the change log; and (B) a layout kept only by the console.

| | A1: logged `Move` | **A2: shared, not logged** | B: console only |
|---|---|---|---|
| Every console sees the same rack | yes | yes | no |
| Undo and log | `z` works, but the log fills with "moved" lines | not in the log; undo handled by the console (see Q2) | local only |
| Claude seats | every move notifies them; older kazoo-mcp builds cannot filter it out | nothing sent; old clients ignore it | nothing sent |
| Fingerprints | needs a no-op case (`fingerprints.rs:180`) | not touched | not touched |
| Protocol risk | new `What` variant (old clients read it as `Unknown`) | new request, event and optional field | none |

**Recommendation: A2.** It works like a physical Eurorack case: everyone sees the same one, but moving a module is not musical history.
- **Stored state:** `PatchFile.rack: Vec<Vec<String>>` (rows of module ids) and `Snapshot.rack: Option<Vec<Vec<String>>>`, both optional with defaults. `None` means an older daemon, so the console falls back to today's derived layout.
- **Request:** `Request::Arrange { module, row, before: Option<String> }`. Using `before` rather than an index holds up when two seats move things at once.
  - Add it to `is_change`, so it is flood-guarded and refused to watchers (`control.rs:~488`).
  - It returns no `Change` and does not bump the revision.
  - It is broadcast as `Event::Rack { rows }` through the existing announce path (`control.rs:~519`, `Outcome.fingerprints` renamed to a general event) and marks the patch dirty so it is saved.
- **Adding with a place:** `Request::Add` gets `place: Option<Place>`. An older daemon silently ignores the field and appends, which is safe.
- **Undoing removals:** `What::Remove` and `What::Restore` get an optional `place`, so undoing a removal puts the plate back where it was. Old log entries fall back to the default rule.
- **Old patch files:** no version bump. `migrate::read` must take the new `rack` field, because it rebuilds files field by field and would otherwise drop it. `Patch::load` fills in default rows without adding a note, drops unknown ids and appends missing ones. Older daemons ignore the field.
- **Gestures:**
  - Dragging a plate's title bar (a new `Hit::Title`) picks the plate up. The rack reflows live around the pointer, with an insertion mark, and the cables re-hang as it moves. Dropping sends one `Arrange`.
  - Dragging the plate body still pans, as today.
  - Keys: `H J K L` shove the selected module left, down, up or right. Moves are gathered like turns, so key repeat does not use up the flood guard.
  - The move is shown right away and held until `Event::Rack` confirms it, with a 2 s timeout, like the existing `SentTurn`.

### Adding from the rack
- In the rack view, `a` places the new module right after the selected one (`Add { place }`).
- Double-clicking bare rack, or clicking a "blank panel" drawn at the end of each row and as a spare row below the last, opens the picker with the place set to where you clicked.
- The picker gets type-to-filter, which matters with about 64 kinds.
- The status line says where the module will go, for example "adding lfo to row 2 after vcf1". The new plate is selected and scrolled into view.

## 3. Build plan
Each step ships and is tested on its own. Steps 1 to 3 are console-only changes.

1. **Performance groundwork and the two bugs (about 1 day).**
   - Cache the layout, keyed on module shapes, rows, density and area.
   - Skip plates outside the view.
   - Cache cable paths per `(id, from, to)` and skip cables whose bounding box misses the view.
   - Build one jack-to-colour map per frame.
   - Fix follow-after-add and j/k in the rack view.
   - Files: `draw/rack.rs`, `rack/geometry.rs`, `app/mod.rs`, `app/keys.rs`.
   - Tests: a new stress test in `tests/render.rs` with 96 modules and 320 cables (the wall's limits, `lib.rs:50-53`) that checks cables half off-screen still draw and the cache is used. Also tests in `tests/keys.rs` for reveal-after-add and j/k.
2. **Space and panning (about 1 day).** `f`, Shift-arrows, `.`, `g`, middle-drag, edge auto-pan.
   - Files: `app/keys.rs`, `app/mouse.rs`, `app/mod.rs` (new `Mode::Jump`), `draw/mod.rs`, `draw/overlay.rs` (help text).
   - Tests: `tests/keys.rs`, `tests/mouse.rs`, `tests/render.rs`.
3. **Density levels (2 to 3 days, the biggest visual piece).** `Scale`, sized dials, compact and overview drawing, overview click-to-zoom.
   - Files: `rack/geometry.rs`, `rack/dial.rs`, `draw/rack.rs`, `app/mouse.rs`.
   - Tests: `tests/rack.rs` (sizes and hits at each level), `tests/mouse.rs` (hits in compact), `tests/render.rs`, and the dial tests.
   - The compact dial needs Tom to look at it.
4. **Shared rows in the protocol and daemon (about 2 days).** The console only reads `Snapshot.rack` at this step.
   - Files: `protocol/mod.rs` (`Arrange`, `Event::Rack`, `Snapshot.rack`, `Add.place`, Remove/Restore `place`), `patch.rs`, `migrate.rs`, `daemon/wall.rs`, `daemon/control.rs`, `change.rs`.
   - Tests: `protocol/tests.rs` (old JSON without `rack` or `place` still parses; an unknown event reads as `Unknown`), `patch/tests.rs`, `migrate/tests.rs`, `daemon/wall/tests.rs` (no log entry, the broadcast, watcher refused, flood), and `tests/daemon.rs`.
   - Also update the Snapshot and ModuleView literals in `tui/tests/mod.rs` and `kazoo-mcp/src/render.rs:591`, and the 26 `Request::Add {` sites, including `kazoo-mcp/src/tools.rs:356`.
5. **Rearranging in the console (about 2 days).** Draw rows from the snapshot, `Hit::Title`, the live-reflow drag, gathered `HJKL`, the held optimistic move, and the move undo (depending on Q2).
   - Files: `app/mouse.rs`, `app/keys.rs`, `app/mod.rs`, `rack/geometry.rs` (`slot_at`), `draw/rack.rs`.
   - Tests: `tests/mouse.rs` (one `Arrange` with the right row and `before`; body-drag still pans), `tests/keys.rs`, `tests/live.rs`.
6. **Adding from the rack (about 1 day).** Place-aware `a`, double-click or blank panel, picker filter.
   - Files: `app/keys.rs`, `app/mouse.rs`, `draw/overlay.rs`, `draw/rack.rs`.
   - Tests: `tests/keys.rs`, `tests/mouse.rs`, `tests/render.rs`.

Later and optional: `wall_look` could list modules by row, and `wall_add` could take `next_to`, so Claude seats can say "the filter on row 2".

**Risks**
- **Performance:** at 96 modules and 320 cables in overview, rebuilding every path every frame would be about 250k dot inserts per frame at 20 fps. Step 1's cache is what makes the overview level workable; ratatui only redraws cells that changed, so terminal output is not the problem.
- **Mouse in terminals:**
  - tmux needs `mouse on`.
  - Many terminals keep shift-drag for text selection, which already affects fine knob drags.
  - Terminal.app often sends no horizontal scroll.
  - Option/Alt is unreliable on macOS.
  - Ctrl-wheel is usually the terminal's own zoom, so zoom stays on keys.
  - Every mouse gesture above has a key equivalent.
- **Protocol compatibility:**
  - No protocol type rejects unknown fields, and `Event` and `What` both fall back to `Unknown`, so old consoles and already-running kazoo-mcp builds keep working and receive nothing noisy.
  - A new console on an old daemon sees `rack: None` and keeps today's derived layout, and says why moving is unavailable instead of sending `Arrange`.
  - The kazoo-mcp changes are only the struct literals.
- **Lint rules:** none of this needs `unsafe`, lint allows, `let _ =` or `.ok()`. The optional serde fields use `#[serde(default, skip_serializing_if = …)]`, as the protocol already does.

## 4. Questions for Tom
1. **Should the arrangement be shared by every console?** I recommend yes: rows are kept in the patch (A2), like a real case.
2. **Should moves be in the change log, and undone with `z`?** I recommend keeping them out of the log so Claude seats aren't notified. `z` would still undo your own last move if it was the last thing you did.
3. **How should today's wall be arranged the first time?** I recommend one row per picker group (sound sources, modulation, filters & dynamics, effects, utilities) rather than the order modules were added. Either way, rows stop reflowing to fit the screen, and you pan down instead.
4. **Where should a module added by a Claude seat, or without a place, land?** I recommend at the end of the row holding the newest module of the same group, or a new bottom row if there is none.
5. **Is this gesture split right: drag a plate's title to move it, drag its body or bare rack to pan, as now?** I recommend yes. I'd also like his answer on two smaller points:
   - Should the chosen density and hidden log be remembered between sessions? I recommend session-only at first.
   - In compact mode, should every knob show its value? I recommend yes.

### Critical files for implementation
- /Users/tom/Developer/projects/deno_rust/kazoo/kazoo-wall/src/tui/rack/geometry.rs
- /Users/tom/Developer/projects/deno_rust/kazoo/kazoo-wall/src/tui/draw/rack.rs
- /Users/tom/Developer/projects/deno_rust/kazoo/kazoo-wall/src/tui/app/mouse.rs
- /Users/tom/Developer/projects/deno_rust/kazoo/kazoo-wall/src/protocol/mod.rs
- /Users/tom/Developer/projects/deno_rust/kazoo/kazoo-wall/src/patch.rs