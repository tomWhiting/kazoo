#!/bin/sh
# Tag this commit v<version> and push the tag, then publish every kazoo
# crate to crates.io at the workspace's version from it, and then kzhz
# (its own workspace) once kzoo is out. npm follows from that tag
# (npm/publish.sh).
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

# Whether crates.io has crate $1 at this version, from its sparse index. An
# index that cannot be reached is no answer: the release stops.
out() {
  name=$1
  case ${#name} in
    1) path="1/$name" ;;
    2) path="2/$name" ;;
    3) path="3/$(printf %s "$name" | cut -c1)/$name" ;;
    *) path="$(printf %s "$name" | cut -c1-2)/$(printf %s "$name" | cut -c3-4)/$name" ;;
  esac
  listing=$(mktemp)
  # --retry repeats timeouts, 408, 429 and 5xx, never a 404.
  code=$(curl -sS -A "$agent" --connect-timeout 10 --max-time 60 --retry 3 \
    -o "$listing" -w '%{http_code}' "https://index.crates.io/$path" 2>/dev/null) || code=000
  case $code in
    200)
      found=0
      python3 -I -c 'import json, sys
version = sys.argv[1]
try:
    listed = any(json.loads(line)["vers"] == version for line in sys.stdin if line.strip())
except (ValueError, KeyError, TypeError):
    sys.exit(2)
sys.exit(0 if listed else 1)' "$version" < "$listing" || found=$?
      rm -f "$listing"
      case $found in
        0) return 0 ;;
        1) return 1 ;;
        *)
          echo "crates.sh: crates.io's index for $name could not be read; nothing more is done" >&2
          exit 1
          ;;
      esac
      ;;
    404)
      rm -f "$listing"
      return 1
      ;;
    *)
      rm -f "$listing"
      echo "crates.sh: could not reach crates.io's index for $name (HTTP $code); nothing more is done" >&2
      exit 1
      ;;
  esac
}

# kzhz is its own workspace (it owns the same commands as kzoo), built
# from its own manifest, and published once kzoo, which it runs, is out.
kzhz="$root/kazoo-kzhz/Cargo.toml"

# One version, one commit. Any crate already out at this version must have
# come from this commit; and the tag is made and pushed before anything
# else goes out, so a rerun from another commit is refused above.
for name in $crates kzhz; do
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
# kzhz cannot be packaged until kzoo is on crates.io. Its lock must hold
# only what the root's does (so it ships what kzoo ships), its lints must
# be the root's, and it goes through the same legs here: fmt, clippy and
# nextest (it has no library, so no doc tests).
if python3 -I - "$root" <<'PY'
import sys
if sys.version_info < (3, 11):
    print(f"crates.sh: these checks need python3 3.11 or later (tomllib); this is {sys.version.split()[0]}", file=sys.stderr)
    sys.exit(4)
import tomllib
root = sys.argv[1]
def load(path):
    with open(f"{root}/{path}", "rb") as f:
        return tomllib.load(f)
def packages(path):
    return {(p["name"], p["version"]) for p in load(path)["package"]}
extra = sorted(p for p in packages("kazoo-kzhz/Cargo.lock") - packages("Cargo.lock") if p[0] != "kzhz")
for name, version in extra:
    print(f"crates.sh: kazoo-kzhz/Cargo.lock has {name} {version}, which the root lock does not", file=sys.stderr)
if extra:
    sys.exit(3)
if load("kazoo-kzhz/Cargo.toml")["workspace"].get("lints") != load("Cargo.toml")["workspace"].get("lints"):
    print("crates.sh: kazoo-kzhz/Cargo.toml's [workspace.lints] are not the root's", file=sys.stderr)
    sys.exit(5)
PY
then
  :
else
  echo "crates.sh: kzhz is out of step with the root (above); nothing is tagged or published" >&2
  exit 1
fi
for leg in fmt clippy nextest; do
  case $leg in
    fmt) set -- fmt --check --manifest-path "$kzhz" ;;
    clippy) set -- clippy --locked -j 4 --all-targets --target-dir "$root/target/kzhz" \
      --manifest-path "$kzhz" -- -D warnings ;;
    nextest) set -- nextest run --locked -j 4 --build-jobs 4 --target-dir "$root/target/kzhz" \
      --manifest-path "$kzhz" ;;
  esac
  if ! nice -n 10 lockf -k "$HOME/.kazoo-cargo.lock" cargo "$@"; then
    echo "crates.sh: kzhz fails cargo $leg; nothing is tagged or published" >&2
    exit 1
  fi
done

if ! git -C "$root" rev-parse -q --verify "refs/tags/v$version" >/dev/null; then
  git -C "$root" tag -a "v$version" -m "kzoo $version" "$head"
fi
git -C "$root" push -q origin "v$version"

log=$(mktemp)
trap 'rm -f "$log"' EXIT
lagged=0

# After `cargo publish` of $1 failed with $log: wait and return to try
# again (the index lagging a publish, or crates.io's rate limit), or stop.
after_refusal() {
  grep -E "Published|^error" "$log" | head -5
  # The index can lag a publish: a crate it does not show yet is tried
  # again and is refused as already there. Give the index time and look.
  if grep -qE "already (exists|uploaded)" "$log"; then
    lagged=$((lagged + 1))
    if [ "$lagged" -gt 10 ]; then
      echo "crates.sh: crates.io still says $1 exists where its index does not show it:" >&2
      cat "$log" >&2
      exit 1
    fi
    echo "crates.sh: crates.io has $1 where the index did not show it yet; waiting 30s"
    sleep 30
    return 0
  fi
  after=$(sed -n 's/.*try again after \(.* GMT\).*/\1/p' "$log" | head -n 1)
  if [ -z "$after" ]; then
    echo "crates.sh: cargo publish of $1 failed, not on a rate limit:" >&2
    cat "$log" >&2
    exit 1
  fi
  if ! until=$(LC_ALL=C date -j -u -f "%a, %d %b %Y %H:%M:%S GMT" "$after" +%s 2>/dev/null); then
    echo "crates.sh: crates.io asked to wait until \"$after\", which is not a time this can read:" >&2
    cat "$log" >&2
    exit 1
  fi
  wait=$((until - $(date +%s) + 10))
  if [ "$wait" -gt 0 ]; then
    echo "crates.sh: crates.io's limit; waiting ${wait}s"
    sleep "$wait"
  fi
}

before=""
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
  # Lags are counted only while nothing goes out.
  if [ -n "$before" ] && [ "$left" -lt "$before" ]; then
    lagged=0
  fi
  before=$left
  if [ "$left" = 0 ]; then
    break
  fi
  # $skip unquoted: each exclusion is its own word.
  if nice -n 10 lockf -k "$HOME/.kazoo-cargo.lock" \
    cargo publish --workspace --locked --no-verify $skip -j 4 \
    --target-dir "$root/target" --manifest-path "$root/Cargo.toml" > "$log" 2>&1; then
    continue
  fi
  after_refusal "a crate"
done

# kzhz last, once crates.io's index shows kzoo at this version.
tries=0
until out kzoo; do
  tries=$((tries + 1))
  if [ "$tries" -gt 60 ]; then
    echo "crates.sh: crates.io's index still does not show kzoo $version; run this again" >&2
    exit 1
  fi
  sleep 10
done
lagged=0
while ! out kzhz; do
  if nice -n 10 lockf -k "$HOME/.kazoo-cargo.lock" \
    cargo publish --locked -j 4 --target-dir "$root/target/kzhz" --manifest-path "$kzhz" > "$log" 2>&1; then
    break
  fi
  after_refusal kzhz
done

echo "crates.sh: kzoo and kzhz $version are on crates.io, tagged v$version at $head"
