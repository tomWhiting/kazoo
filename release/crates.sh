#!/bin/sh
# Publish every kazoo crate to crates.io at the workspace's version, then
# tag the commit they came from as v<version> and push the tag. npm follows
# from that tag (npm/publish.sh).
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

log=$(mktemp)
trap 'rm -f "$log"' EXIT
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
    cargo publish --workspace --locked $skip -j 4 \
    --target-dir "$root/target" --manifest-path "$root/Cargo.toml" > "$log" 2>&1; then
    continue
  fi
  grep -E "Published|^error" "$log" | head -5
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

if ! git -C "$root" rev-parse -q --verify "refs/tags/v$version" >/dev/null; then
  git -C "$root" tag -a "v$version" -m "kzoo $version" "$head"
fi
git -C "$root" push origin "v$version"
echo "crates.sh: kzoo $version is on crates.io, tagged v$version at $head"
