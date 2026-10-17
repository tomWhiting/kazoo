# KZ-G04: kzoo 0.2.0, the first release a plain `cargo install kzoo` takes

Written by Cassio, 17 Oct 2026, about 11:20. Decided by Cassio; the reasons are given with each decision.

Place: Track G, after KZ-G03. Files: every crate's `Cargo.toml` and `Cargo.lock`, `npm/` (`publish.sh`, `notices.py`, `vendored-notices.txt`, `licences/`, the three `package.json`s).

## Why

kzoo 0.1.0 went to crates.io on 17 Oct 2026, but a plain `cargo install kzoo` could not resolve it, because pyin's `zip` 0.5 is yanked. KZ-V16 removes pyin, so a plain install resolves. Tom asked on 17 Oct for a release to crates.io and npm under the name kzoo. On 15 Oct he said nobody waits for his approval to publish: gated and green means published and tagged, and he is told afterwards.

## Outcome

- **Version 0.2.0.** Every crate moves to 0.2.0 together, and every internal dependency is pinned `=0.2.0`. Ten instrument crates had a plain `"0.1.0"` (a caret range) on kazoo-core, and those are pinned as well. It is 0.2.0, not 0.1.1, because breaking changes have landed since 0.1.0:
  - kazoo-host's `Options` gains a public field (KZ-G03);
  - kazoo-core's `JudgeStatsSnapshot` and `EngineStatsSnapshot` gain public fields (KZ-V16, KZ-V17).
- **npm notices.** Each npm platform package ships `THIRD-PARTY-NOTICES`, written by `npm/notices.py` from `cargo metadata` for its target. It covers:
  - the licence texts of every crate linked into the program;
  - for a crate that ships no licence text, the licence it declares: MIT is written out with its authors, and Apache-2.0 and MPL-2.0 are appended once, with where each MPL crate's source is published;
  - the pYIN port's MIT and librosa ISC notices.

  A release with a notice missing is refused.
- **One version, one commit.** `npm/publish.sh` fetches the tags, then refuses a version whose tag names another commit. npm is published at 0.2.0, never at 0.1.0 from a main that no longer matches tag v0.1.0.

## Done when

- The workspace gate is green, apart from the known R04 pair.
- All 23 crates are on crates.io at 0.2.0, and tag v0.2.0 is pushed.
- A plain `cargo install kzoo` resolves from crates.io.
- npm follows as soon as Tom's `npm login` works.
