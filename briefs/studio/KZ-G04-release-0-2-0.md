# KZ-G04: kzoo 0.2.0, the first release a plain `cargo install kzoo` takes

Written by Cassio, 17 Oct 2026, about 11:20, and brought up to date about 11:40 and 11:55 after the first two Opus reviews. Decided by Cassio; the reasons are given with each decision.

Place: Track G, after KZ-G03. Files: every crate's `Cargo.toml` and `Cargo.lock`; `release/crates.sh` (new); `npm/`: `publish.sh`, `notices.py`, `vendored-notices.txt`, `licences/` (with `upstream/` and `upstream.json`), `README.md`, `.gitignore`, the three `package.json`s and the two platform `.gitignore`s.

## Why

kzoo 0.1.0 went to crates.io on 17 Oct 2026, but a plain `cargo install kzoo` could not resolve it, because pyin's `zip` 0.5 is yanked. KZ-V16 removes pyin, so a plain install resolves. Tom asked on 17 Oct for a release to crates.io and npm under the name kzoo. On 15 Oct he said nobody waits for his approval to publish: gated and green means published and tagged, and he is told afterwards.

## Outcome

- **Version 0.2.0.** Every crate moves to 0.2.0 together, and every internal dependency is pinned `=0.2.0`. Eleven crates had a plain `"0.1.0"`, a caret range, on kazoo-core: the ten instruments and kazoo-tui. Those are pinned as well. It is 0.2.0, not 0.1.1, because breaking changes have landed since 0.1.0:
  - kazoo-host's `Options` gains a public field (KZ-G03);
  - kazoo-core's `JudgeStatsSnapshot` and `EngineStatsSnapshot` gain public fields (KZ-V16, KZ-V17), and neither struct is `#[non_exhaustive]`.
- **The tag, then crates.io.** `release/crates.sh` releases from a clean main that is origin/main.
  - Any crate already on crates.io at this version must have come from this commit; it reads each crate's `.cargo_vcs_info.json` to check.
  - It packages every crate and builds each from its package (`cargo package --workspace`) before anything else, so a crate that cannot be published is found while the version is still free.
  - It tags the commit `v<version>` and pushes the tag before anything is published, so a rerun from another commit is refused. A tag naming another commit is refused.
  - It then publishes each crate in dependency order (`cargo publish --workspace --no-verify`, already verified), passing over any already out at this version.
  - A crate published from a checkout with changes (`"dirty": true`) counts as from no commit, here and in npm/publish.sh.
  - When crates.io's limit is hit, it waits as long as crates.io says and goes on.
  - When the index lags a publish, it waits and looks again, up to ten times.
- **npm, from the tag.** `npm/publish.sh` publishes the commit tagged `v<version>`: main at the tag, or the tag checked out.
  - The checkout must be clean and on origin/main.
  - crates.io's record of where kzoo `<version>` came from (`.cargo_vcs_info.json`) must name that same commit.
  - So npm can follow crates.io at any time, and it never ships a binary from another commit.
  - A dry run reports the checks it cannot pass instead of stopping on them.
  - Each build is checked to link only the system's libraries, and a sanitizer's runtime is kept out. A failure of `otool` itself stops the release.
  - If crates.io can't be reached, or its record doesn't say a commit, it says so rather than taking that as "not published".
- **Third-party notices.** Each npm platform package ships `THIRD-PARTY-NOTICES`, written by `npm/notices.py` before any build, whole or not at all.
  - **What it covers:** the crates `cargo tree -p kzoo -e normal,no-proc-macro --target <triple>` builds into the program: 203 on Apple silicon and 204 on Intel. It leaves out the workspace's own crates, and proc-macros and the crates only they use, which run while building and are not in the program.
  - **How a crate's licence is checked:** its expression is parsed (OR, AND, WITH, parentheses), and one way to meet it must be covered by recognised texts. Those are texts the crate ships, plus the full Apache-2.0 and MPL-2.0 texts appended once.
    - A text is recognised by wording it must have and wording it must lack, so BSD-3, BSD-2 and BSD-1, ISC and 0BSD, and MIT and MIT-0 are told apart.
    - A cargo failure reports cargo's own words, and a failed write leaves no file behind.
  - **Crates that ship no text:** 34 crates, from eight projects. They are covered by the upstream texts kept in `npm/licences/upstream/`, fetched at the commits the crates were published from, and listed by exact version in `upstream.json`, so an upgrade must be checked again.
    - realfft and block have no licence text anywhere, so they are noticed under MIT with their authors from Cargo.toml, and the notice says so.
    - The objc2 crates' MIT text exists only on objc2's main branch, fetched 17 Oct 2026.
  - **MPL-2.0:** every MPL-2.0 crate is noted with where its source is published, and must come from crates.io.
  - **Vendored code:** the pYIN port's MIT and librosa ISC notices.
  - **Failing closed:** anything not covered fails the run, naming each crate.

## Done when

- The workspace gate is green, apart from two known failures, which are excluded with this reason: `early_meets_its_targets` and `a_sweep_is_one_onset_per_note` are KZ-R04's open measurement targets, parked as a lane of their own. They fail on main, they failed identically when 0.1.0 was published, and none of this release's changes touch them.
- All 23 crates are on crates.io at 0.2.0, and tag v0.2.0 is pushed.
- A plain `cargo install kzoo` resolves from crates.io.
- npm follows as soon as Tom's `npm login` works.
