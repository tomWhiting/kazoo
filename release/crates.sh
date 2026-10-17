#!/bin/sh
# Tag this commit v<version> and push the tag, then publish every kazoo
# crate to crates.io at the workspace's version from it. npm follows from
# that tag (npm/publish.sh).
#
#   release/crates.sh
#
# Releases only from a clean main that is origin/main. A crate already on
# crates.io at this version is passed over, so a release stopped part way
# (crates.io limits how fast crates go up) is finished by running this
# again; when a limit is hit it waits as long as crates.io says and goes on.
# Builds with the repository's lock, four jobs, niced.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
agent="kazoo-release (https://github.com/tomWhiting/kazoo)"

if [ "$(git -C "$root" branch --show-current)" != main ] || [ -n "$(git -C "$root" status --porcelain)" ]; then
  echo "crates.sh: release from a clean checkout of main" >&2
  exit 1
fi
if ! git -C "$root" fetch -q --tags origin; then
  echo "crates.sh: could not fetch from origin; a release needs its tags and main" >&2
  exit 1
fi
head=$(git -C "$root" rev-parse HEAD)
if [ "$head" != "$(git -C "$root" rev-parse origin/main)" ]; then
  echo "crates.sh: main is not origin/main; push or pull first" >&2
  exit 1
fi

version=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/kazoo-cli/Cargo.toml" | head -n 1)
if [ -z "$version" ]; then
  echo "crates.sh: kazoo-cli/Cargo.toml has no version" >&2
  exit 1
fi
if tagged=$(git -C "$root" rev-parse -q --verify "refs/tags/v$version^{commit}"); then
  if [ "$tagged" != "$head" ]; then
    echo "crates.sh: v$version was released from $tagged, not this commit; bump the version" >&2
    exit 1
  fi
fi

# The published crates, from cargo itself; the unpublished (publish = false)
# are left out.
meta=$(nice -n 10 lockf -k "$HOME/.kazoo-cargo.lock" \
  cargo metadata --locked --no-deps --format-version 1 --manifest-path "$root/Cargo.toml")
crates=$(printf %s "$meta" |
  python3 -I -c 'import json, sys
for p in json.load(sys.stdin)["packages"]:
    if p.get("publish") != []:
        print(p["name"])')
unpublished=$(printf %s "$meta" |
  python3 -I -c 'import json, sys
for p in json.load(sys.stdin)["packages"]:
    if p.get("publish") == []:
        print(p["name"])')

# The commit crates.io records crate $1 at this version came from, from its
# .cargo_vcs_info.json; empty if it says none.
source_of() {
  file=$(mktemp)
  if ! curl -fsSL -A "$agent" -o "$file" "https://static.crates.io/crates/$1/$1-$version.crate"; then
    rm -f "$file"
    echo "crates.sh: could not fetch $1 $version from crates.io to check its commit" >&2
    exit 1
  fi
  # A crate published from a checkout with changes is from no commit.
  tar -xzOf "$file" "$1-$version/.cargo_vcs_info.json" 2>/dev/null |
    python3 -I -c 'import json, sys
git = json.load(sys.stdin)["git"]
print("" if git.get("dirty") else git["sha1"])' 2>/dev/null || true
  rm -f "$file"
}

# Whether crates.io has crate $1 at this version, from its sparse index.
out() {
  name=$1
  case ${#name} in
    1) path="1/$name" ;;
    2) path="2/$name" ;;
    3) path="3/$(printf %s "$name" | cut -c1)/$name" ;;
    *) path="$(printf %s "$name" | cut -c1-2)/$(printf %s "$name" | cut -c3-4)/$name" ;;
  esac
  curl -fsS -A "$agent" "https://index.crates.io/$path" 2>/dev/null |
    python3 -I -c 'import json, sys
version = sys.argv[1]
sys.exit(0 if any(json.loads(line)["vers"] == version for line in sys.stdin if line.strip()) else 1)' "$version"
}

# One version, one commit. Any crate already out at this version must have
# come from this commit; and the tag is made and pushed before anything
# else goes out, so a rerun from another commit is refused above.
for name in $crates; do
  if out "$name"; then
    from=$(source_of "$name")
    if [ "$from" != "$head" ]; then
      echo "crates.sh: $name $version is on crates.io from ${from:-an unknown commit}, not $head; bump the version" >&2
      exit 1
    fi
  fi
done
# Every crate packaged and built from its package before the tag, so a
# crate that cannot be published is found while the version is still free.
skip_unpublished=""
for name in $unpublished; do
  skip_unpublished="$skip_unpublished --exclude $name"
done
if ! nice -n 10 lockf -k "$HOME/.kazoo-cargo.lock" \
  cargo package --workspace --locked $skip_unpublished -j 4 \
  --target-dir "$root/target" --manifest-path "$root/Cargo.toml"; then
  echo "crates.sh: a crate does not package and build; nothing is tagged or published" >&2
  exit 1
fi

if ! git -C "$root" rev-parse -q --verify "refs/tags/v$version" >/dev/null; then
  git -C "$root" tag -a "v$version" -m "kzoo $version" "$head"
fi
git -C "$root" push -q origin "v$version"

log=$(mktemp)
trap 'rm -f "$log"' EXIT
lagged=0
while :; do
  skip=""
  for name in $unpublished; do
    skip="$skip --exclude $name"
  done
  left=0
  for name in $crates; do
    if out "$name"; then
      skip="$skip --exclude $name"
    else
      left=$((left + 1))
    fi
  done
  echo "crates.sh: $(date '+%H:%M:%S') $left of $(echo "$crates" | wc -w | tr -d ' ') to publish"
  if [ "$left" = 0 ]; then
    break
  fi
  # $skip unquoted: each exclusion is its own word.
  if nice -n 10 lockf -k "$HOME/.kazoo-cargo.lock" \
    cargo publish --workspace --locked --no-verify $skip -j 4 \
    --target-dir "$root/target" --manifest-path "$root/Cargo.toml" > "$log" 2>&1; then
    continue
  fi
  grep -E "Published|^error" "$log" | head -5
  # The index can lag a publish: a crate it does not show yet is tried
  # again and is refused as already there. Give the index time and look.
  if grep -q "already exists" "$log"; then
    lagged=$((lagged + 1))
    if [ "$lagged" -gt 10 ]; then
      echo "crates.sh: crates.io still says a crate exists that its index does not show:" >&2
      cat "$log" >&2
      exit 1
    fi
    echo "crates.sh: crates.io has a crate the index did not show yet; waiting 30s"
    sleep 30
    continue
  fi
  after=$(sed -n 's/.*try again after \(.* GMT\).*/\1/p' "$log" | head -n 1)
  if [ -z "$after" ]; then
    echo "crates.sh: cargo publish failed, not on a rate limit:" >&2
    cat "$log" >&2
    exit 1
  fi
  until=$(date -j -u -f "%a, %d %b %Y %H:%M:%S GMT" "$after" +%s)
  wait=$((until - $(date +%s) + 10))
  if [ "$wait" -gt 0 ]; then
    echo "crates.sh: crates.io's limit; waiting ${wait}s"
    sleep "$wait"
  fi
done

echo "crates.sh: kzoo $version is on crates.io, tagged v$version at $head"
