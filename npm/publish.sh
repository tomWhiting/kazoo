#!/bin/sh
# Build kzoo for both Macs, put each in its npm package with its
# third-party notices, and publish the platform packages and then the
# launcher, all at the crate's version.
#
#   npm/publish.sh            build, fill and publish
#   npm/publish.sh --dry-run  build and fill, then npm publish --dry-run
#
# npm follows crates.io (release/crates.sh): a release publishes the commit
# tagged v<version>, which must be the commit crates.io has kzoo <version>
# from, and on origin/main. Check it out (main at the tag, or the tag itself)
# and run this. A dry run checks what it can and says what it could not.
# Builds with the repository's lock, four jobs, niced.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
dry=""
if [ "${1:-}" = "--dry-run" ]; then
  dry="--dry-run"
fi
agent="kazoo-release (https://github.com/tomWhiting/kazoo)"

# What a dry run may pass over; a release may not.
refuse() {
  if [ -n "$dry" ]; then
    echo "publish.sh: (dry run) $1" >&2
  else
    echo "publish.sh: $1" >&2
    exit 1
  fi
}

version=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/kazoo-cli/Cargo.toml" | head -n 1)
if [ -z "$version" ]; then
  echo "publish.sh: kazoo-cli/Cargo.toml has no version" >&2
  exit 1
fi
head=$(git -C "$root" rev-parse HEAD)

if [ -n "$(git -C "$root" status --porcelain)" ]; then
  refuse "the checkout has changes; release from a clean one"
fi
if git -C "$root" fetch -q --tags origin; then
  if ! git -C "$root" merge-base --is-ancestor "$head" origin/main; then
    refuse "this commit is not on origin/main"
  fi
else
  refuse "could not fetch from origin, so the tags and main are unchecked"
fi
# A version is one commit: the one tagged, which crates.io has too.
if tagged=$(git -C "$root" rev-parse -q --verify "refs/tags/v$version^{commit}"); then
  if [ "$tagged" != "$head" ]; then
    refuse "v$version was released from $tagged, not this commit; check out v$version, or bump the version"
  fi
else
  refuse "v$version is not tagged; release the crates first (release/crates.sh)"
fi
crate=$(mktemp)
trap 'rm -f "$crate"' EXIT
if curl -fsSL -A "$agent" -o "$crate" "https://static.crates.io/crates/kzoo/kzoo-$version.crate"; then
  published=$(tar -xzOf "$crate" "kzoo-$version/.cargo_vcs_info.json" |
    python3 -I -c 'import json, sys; print(json.load(sys.stdin)["git"]["sha1"])')
  if [ "$published" != "$head" ]; then
    refuse "crates.io has kzoo $version from $published, not this commit"
  fi
else
  refuse "crates.io has no kzoo $version yet; release the crates first (release/crates.sh)"
fi

# The notices first: quick, and a release with one missing stops before
# the long builds. Written whole or not at all.
for pair in aarch64-apple-darwin:arm64 x86_64-apple-darwin:x64; do
  target=${pair%%:*}
  arch=${pair##*:}
  nice -n 10 lockf -k "$HOME/.kazoo-cargo.lock" \
    python3 -I "$here/notices.py" "$root" "$target" "$here/kzoo-darwin-$arch/THIRD-PARTY-NOTICES"
done

# A sanitizer's runtime must never be linked into a release.
unset RTSAN_ENABLE
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
  # Only the system's own libraries: anything else is not on the
  # machines it is installed on.
  foreign=$(otool -L "$package/bin/kzoo" | tail -n +2 | awk '{print $1}' |
    grep -v -e '^/usr/lib/' -e '^/System/Library/' || true)
  if [ -n "$foreign" ]; then
    echo "publish.sh: $target links libraries outside the system: $foreign" >&2
    exit 1
  fi
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
# executable and the third-party notices in each platform package, the
# licence in all three, the README in the launcher.
for package in kzoo-darwin-arm64 kzoo-darwin-x64; do
  (cd "$here/$package" && npm pack --dry-run --json) | node -e '
    const files = JSON.parse(require("fs").readFileSync(0, "utf8"))[0].files;
    const program = files.find((f) => f.path === "bin/kzoo");
    if (!program || (program.mode & 0o111) === 0) throw new Error("bin/kzoo is missing or not executable");
    if (!files.some((f) => f.path === "LICENSE")) throw new Error("LICENSE is missing");
    if (!files.some((f) => f.path === "THIRD-PARTY-NOTICES")) throw new Error("THIRD-PARTY-NOTICES is missing");'
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
