# npm packages for kzoo

- `kzoo`: the launcher `npx kzoo` runs. It finds the build for this machine
  and runs it with the same arguments.
- `kzoo-darwin-arm64`, `kzoo-darwin-x64`: the program itself for each Mac
  (`bin/kzoo`), installed by `kzoo` as an optional dependency for the
  matching machine only. Each carries `LICENSE` and `THIRD-PARTY-NOTICES`.

A release goes to crates.io first, then npm, from one commit:

1. `release/crates.sh`, from a clean main that is origin/main, publishes
   every crate at the workspace's version, waiting out crates.io's limits.
   It then tags the commit `v<version>` and pushes the tag.
2. `npm/publish.sh`, from that tagged commit (main at the tag, or the tag
   checked out), does the rest:
   1. It checks that the checkout is clean and on origin/main. The tag
      `v<version>` must name it, and so must crates.io's record of where
      kzoo `<version>` came from.
   2. It writes each platform's `THIRD-PARTY-NOTICES` with `notices.py`.
      That run stops if any crate linked into that build has a licence it
      cannot cover.
   3. It builds `kzoo` in release for `aarch64-apple-darwin` and
      `x86_64-apple-darwin`. Each build must link only the system's
      libraries, and is signed ad hoc and verified.
   4. It sets every package's `version`, and the launcher's
      `optionalDependencies`, to the crate's version.
   5. It checks each tarball:
      - the program, executable, and the notices in each platform package;
      - the licence in all three;
      - the README in `kzoo`.
   6. It publishes the two platform packages, then `kzoo`. Any already out
      at this version are passed over.

`npm/publish.sh --dry-run` does all of it up to `npm publish --dry-run`. It
reports the release checks it could not pass instead of stopping on them.

The licence texts `notices.py` uses for crates that ship none are in
`licences/upstream/`, listed by exact version in `licences/upstream.json`.
Code vendored into kzoo's own crates is in `vendored-notices.txt`.
