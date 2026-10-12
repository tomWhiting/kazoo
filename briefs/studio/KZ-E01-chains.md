# KZ-E01: `:chain save` keeps a run of modules and cables; `:chain add` puts it down

Line numbers are from the sweep that wrote this brief and may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md with `-p kazoo-wall -p kazoo-mcp`.

## Outcome

Tom builds a chain he likes (say kick → klon → tape), types `:chain save beatchain`, and later `:chain add beatchain` puts a copy of it on any wall, cables included, named and placed like new modules.

## The design (decided; build exactly this)

- Model: a `Chain { name, modules: Vec<ChainModule { kind, label, knobs: Vec<f32> }>, cables: Vec<ChainCable { from: (usize, port), to: (usize, jack), amount }> }` stored as `~/.kazoo/wall/chains/<name>.json` through the same serde path as `PatchFile` (read `patch.rs` for the save format and the commit that added groups (find it with `git log -S":group" -- kazoo-wall`) for how a named set is selected). Indices are positions in `modules`, so a chain is wall-independent.
- `:chain save <name>` takes the current selection (the group under the cursor, or the tagged modules; read how `:group` selects) and keeps every cable whose both ends are inside it; a cable with one end outside is dropped and counted in the reply ("saved 3 modules, 2 cables, 1 cable to the outside left off").
- `:chain add <name>` adds each module through the existing `Request::Add` path (so the load guard and the log apply), then the cables through `Request::Patch`, as one undo step; the modules land in a fresh row via `usual_place`.
- `:chains` lists the saves. `Request::ChainSave`, `Request::ChainAdd`, `Event::Chains` added beside the group variants; `kazoo-mcp` gains `wall_chain {save|add|list}` the way `wall_group` was added.

## Not this lane

Folding (KZ-E02). Any engine change.

## Red tests first

- `patch::tests::chain_save_keeps_only_inside_cables`.
- `patch::tests::chain_add_is_one_undo_step_and_lands_in_a_new_row`.
- `patch::tests::chain_round_trips_through_json`.
- `mcp::tests::wall_chain_tool_lists_saves`.

## Definition of done

- [ ] Four red tests then green.
- [ ] By hand on the live wall: save the `beat` group as a chain, add it, hear two kicks (say what you heard).
