#!/bin/sh
# Publish the launcher under the first of these names npm takes. npm
# refuses an unscoped name it finds too like a popular package (kzoo was
# "too similar to koa, zod"), and says so only when the name is published.
#
#   npm/launcher.sh kzka kzkz kaZz @kzoo/kzoo
#   npm/launcher.sh --dry-run kzka ...   everything up to npm publish --dry-run
#
# Each name is also a command the package installs, beside kzoo and kazoo:
# kaZz is published as the package kazz with the command kaZz, and
# @kzoo/kzoo as that scoped package with the command kzoo. A scope must
# exist first: an organisation is made on npmjs.com, never here.
#
# It stops at the first name published, at a name already out at this
# version, or at any refusal other than "too similar" (a sign-in, say).
# The platform packages must be out at this version first (npm/publish.sh):
# the launcher is never out before what it runs.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
dry=""
if [ "${1:-}" = "--dry-run" ]; then
  dry="--dry-run"
  shift
fi
if [ "$#" -eq 0 ]; then
  echo "usage: npm/launcher.sh [--dry-run] name..." >&2
  exit 2
fi

version=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/kazoo-cli/Cargo.toml" | head -n 1)
if [ -z "$version" ]; then
  echo "launcher.sh: kazoo-cli/Cargo.toml has no version" >&2
  exit 1
fi
for platform in kzoo-darwin-arm64 kzoo-darwin-x64; do
  if ! npm view "$platform@$version" version >/dev/null 2>&1; then
    echo "launcher.sh: $platform $version is not on npm; run npm/publish.sh first" >&2
    exit 1
  fi
done

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
mkdir "$stage/bin"
cp "$here/kzoo/bin/kzoo.js" "$stage/bin/kzoo.js"
cp "$root/LICENSE" "$stage/LICENSE"
cp "$root/README.md" "$stage/README.md"

for wanted in "$@"; do
  name=$(printf '%s' "$wanted" | tr '[:upper:]' '[:lower:]')
  command=${wanted##*/}
  case $name in
    @*/?*)
      scope=${name%%/*}
      scope=${scope#@}
      if ! npm org ls "$scope" >/dev/null 2>&1 &&
        ! npm access list packages "@$scope" >/dev/null 2>&1; then
        echo "launcher.sh: there is no npm scope @$scope; make the organisation at https://www.npmjs.com/org/create and run this again" >&2
        exit 1
      fi
      ;;
    @* | */*)
      echo "launcher.sh: $wanted is not a package name" >&2
      exit 2
      ;;
  esac
  if npm view "$name@$version" version >/dev/null 2>&1; then
    echo "launcher.sh: $name $version is out already"
    exit 0
  fi

  # A fresh manifest for each name, so no earlier name's command is left in it.
  cp "$here/kzoo/package.json" "$stage/package.json"
  (cd "$stage" &&
    npm pkg set name="$name" version="$version" "bin.$command=bin/kzoo.js" \
      "optionalDependencies.kzoo-darwin-arm64=$version" \
      "optionalDependencies.kzoo-darwin-x64=$version")

  echo "launcher.sh: trying $name (command $command)"
  log="$stage/publish.log"
  # npm signs in through the browser only on a terminal, so it gets one
  # (script); the output is kept as well, to read npm's reason.
  if script -q "$log" sh -c 'cd "$1" && shift && npm publish "$@"' sh "$stage" --access public $dry; then
    published=0
  else
    published=$?
  fi
  if [ "$published" = 0 ]; then
    if [ -n "$dry" ]; then
      echo "launcher.sh: (dry run) $name packs; npm judges the name only when it is published"
      continue
    fi
    echo "launcher.sh: published $name $version; install it with npm install -g $name"
    exit 0
  fi
  if grep -q "too similar" "$log"; then
    echo "launcher.sh: npm refused $name as too like another package; trying the next name"
    continue
  fi
  echo "launcher.sh: publishing $name failed for another reason (above); stopping" >&2
  exit 1
done
if [ -n "$dry" ]; then
  exit 0
fi
echo "launcher.sh: npm refused every name given" >&2
exit 1
