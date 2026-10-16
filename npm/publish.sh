#!/bin/sh
# Build kzoo for both Macs, put each in its npm package, and publish the
# platform packages and then the launcher, all at the crate's version.
#
#   npm/publish.sh            build, fill and publish
#   npm/publish.sh --dry-run  build and fill, then npm publish --dry-run
#
# Run from anywhere; builds with the repository's lock, four jobs, niced.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
dry=""
if [ "${1:-}" = "--dry-run" ]; then
  dry="--dry-run"
fi

version=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/kazoo-cli/Cargo.toml" | head -n 1)
if [ -z "$version" ]; then
  echo "publish.sh: kazoo-cli/Cargo.toml has no version" >&2
  exit 1
fi

for pair in aarch64-apple-darwin:arm64 x86_64-apple-darwin:x64; do
  target=${pair%%:*}
  arch=${pair##*:}
  nice -n 10 lockf -k "$HOME/.kazoo-cargo.lock" \
    cargo build -j 4 --release --locked -p kzoo --bin kzoo \
    --target "$target" --target-dir "$root/target" --manifest-path "$root/Cargo.toml"
  package="$here/kzoo-darwin-$arch"
  mkdir -p "$package/bin"
  cp "$root/target/$target/release/kzoo" "$package/bin/kzoo"
  chmod 755 "$package/bin/kzoo"
  # Signed ad hoc here: the linker signs only Apple silicon builds, and
  # macOS kills an unsigned program as it starts.
  codesign --force --sign - "$package/bin/kzoo"
  codesign --verify "$package/bin/kzoo"
  cp "$root/LICENSE" "$package/LICENSE"
done
cp "$root/LICENSE" "$here/kzoo/LICENSE"
cp "$root/README.md" "$here/kzoo/README.md"

# Every package at the crate's version, and the launcher asking for them.
for package in kzoo kzoo-darwin-arm64 kzoo-darwin-x64; do
  (cd "$here/$package" && npm pkg set version="$version")
done
(cd "$here/kzoo" &&
  npm pkg set "optionalDependencies.kzoo-darwin-arm64=$version" \
    "optionalDependencies.kzoo-darwin-x64=$version")

# What each tarball holds, checked before anything goes out: the program
# executable in each platform package, the licence in all three.
for package in kzoo-darwin-arm64 kzoo-darwin-x64; do
  (cd "$here/$package" && npm pack --dry-run --json) | node -e '
    const files = JSON.parse(require("fs").readFileSync(0, "utf8"))[0].files;
    const program = files.find((f) => f.path === "bin/kzoo");
    if (!program || (program.mode & 0o111) === 0) throw new Error("bin/kzoo is missing or not executable");
    if (!files.some((f) => f.path === "LICENSE")) throw new Error("LICENSE is missing");'
done
(cd "$here/kzoo" && npm pack --dry-run --json) | node -e '
  const files = JSON.parse(require("fs").readFileSync(0, "utf8"))[0].files.map((f) => f.path);
  for (const want of ["bin/kzoo.js", "LICENSE", "README.md"]) {
    if (!files.includes(want)) throw new Error(want + " is missing");
  }'

# The programs first: the launcher must never be out before what it runs.
# A package already out at this version is passed over, so a release that
# stopped part way is finished by running this again.
for package in kzoo-darwin-arm64 kzoo-darwin-x64 kzoo; do
  if [ -z "$dry" ] && npm view "$package@$version" version >/dev/null 2>&1; then
    echo "publish.sh: $package $version is out already"
    continue
  fi
  (cd "$here/$package" && npm publish --access public $dry)
done

# Tagged once all of it is out (the crates.io release tags the same).
if [ -z "$dry" ] && ! git -C "$root" rev-parse -q --verify "refs/tags/v$version" >/dev/null; then
  git -C "$root" tag -a "v$version" -m "kzoo $version"
fi
