# npm packages for kzoo

- `kzoo`: the launcher `npx kzoo` runs. It finds the build for this machine
  and runs it with the same arguments.
- `kzoo-darwin-arm64`, `kzoo-darwin-x64`: the program itself for each Mac
  (`bin/kzoo`), installed by `kzoo` as an optional dependency for the
  matching machine only.

Each release, in order (`publish.sh` does it):

1. Build `kzoo` in release for `aarch64-apple-darwin` and
   `x86_64-apple-darwin`, and copy each into its package's `bin/kzoo`.
2. Set every package's `version`, and the launcher's
   `optionalDependencies`, to the crate's version.
3. Check each tarball: the program executable and signed in each platform
   package, the licence in all three, the README in `kzoo`.
4. Publish the two platform packages, then `kzoo`.
