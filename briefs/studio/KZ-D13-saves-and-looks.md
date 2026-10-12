# KZ-D13: the wall saves only on change, and `look` and the console paginate

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md with `-p kazoo-wall -p kazoo-mcp`. Depends: KZ-D05.

## Outcome

A big wall no longer writes a full JSON file every second whether or not anything changed, and a `look` at a thousand-module wall comes back as pages instead of one line that grows without bound.

## The design (decided; build exactly this)

- Saving (find the once-a-second autosave in `daemon/wall.rs` that serialises the whole `PatchFile`): a `dirty: bool` set by every applied `Request` that changes the patch, cleared on write; the writer runs on its timer but writes only when dirty, to `<path>.tmp` then `rename` (atomic on APFS), with the serialisation done on the daemon thread, never the render thread (already the case; keep it so). Knob glides mark dirty once at the end of the glide, not per step.
- `look` (`Request::Look` / `Event::Look` and the MCP `wall_look`): add `page: u16` and `per_page: u16` (default 64 modules) with `pages: u16` in the answer; cables are listed with the page of their source module. `wall_look` gains optional `page`. The console's module list and the picker use the same pager.
- The single-line `look` text gains a trailer "page 1 of 7" when there is more than one page.

## Not this lane

Incremental tables (KZ-D05), the engine.

## Red tests first

- `wall::tests::no_change_no_write` (a tempdir; the file's mtime unchanged after 5 ticks with nothing applied).
- `wall::tests::a_change_writes_once_atomically` (no `.tmp` left behind; contents parse).
- `wall::tests::look_pages_a_200_module_wall_into_four` and `look_page_out_of_range_is_an_error_in_words`.
- `mcp::tests::wall_look_passes_page_through`.

## Definition of done

- [ ] Five red tests then green.
- [ ] On the live wall: `ls -l ~/.kazoo/wall/` shows the save's mtime still while nobody touches it; `wall_look` of the 90-module save returns two pages.
