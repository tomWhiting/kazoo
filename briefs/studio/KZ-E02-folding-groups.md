# KZ-E02: a group folds to one tile on the rack and unfolds

Sweep: e0de0c0. Lines may have moved; find each item by what it does, never rename. Checks, code quality and the END block are as PLAN.md with `-p kazoo-wall`.

## Outcome

A big wall stops being a wall of noise: `:fold beat` turns the whole `beat` group into one tile showing its name, module count and its outside cables; `:unfold beat` opens it again. Sound is untouched.

## The design (decided; build exactly this)

- Fold is a console view state only (`Look.folded: BTreeSet<String>` persisted in `~/.kazoo/wall/console.json` beside the view prefs from ead6d8f); the engine and the patch model never know.
- Rack rendering (read the rack view's row layout in the console): a folded group renders as one tile at the place of its first module; the other modules of the group are skipped; cables from outside the group to any module inside are drawn to the tile; cables inside are not drawn.
- Cursor movement skips folded members; H/J/K/L on a tile moves the whole group (the existing group move from 4e02527, or blocked if that is still blocked: say which under FOUND).
- `:fold <group>`, `:unfold <group>`, `:fold` with no name folds the group under the cursor; `Tab` on a tile unfolds it.

## Not this lane

Chains (KZ-E01). Engine or MCP changes (none).

## Red tests first

- `console::tests::folded_group_renders_one_tile_with_count` (render to a test buffer, assert the tile text and that member names are absent).
- `console::tests::outside_cables_land_on_the_tile`.
- `console::tests::cursor_skips_folded_members`.
- `console::tests::fold_state_survives_a_restart` (round trip through console.json).

## Definition of done

- [ ] Four red tests then green.
- [ ] By hand: fold `scream` on the live wall, the rack shows one tile; unfold restores it; audio unchanged.
