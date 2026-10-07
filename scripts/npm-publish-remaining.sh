#!/usr/bin/env bash
# Publishes the npm packages of <version> that are not on the registry yet —
# for resuming a first publish that stopped half-way (e.g. a qrsend-bin-win32-*
# package rejected by npm's spam detection until support whitelists it), and
# for the first publish of platform packages added in a later release
# (@qrsend/cli-bin-*), which have no trusted publisher yet.
#
# - Packages already on the registry at <version> are left alone.
# - --skip NAME leaves a package out (repeatable).
# - --try-name PKG=NAME first publishes a copy of platform package PKG under
#   NAME, to find out whether npm accepts that name (its spam detection tends
#   to reject new unscoped *-win32-* names). If npm rejects it, the run goes on
#   with the names as they are. If npm accepts it, the run stops there, before
#   anything else is published: switch npm/assemble.py and npm/launcher.js to
#   that name and run again.
# - --trust registers this repository's publish-all.yml (environment `npm`) as
#   the trusted publisher of every package this run publishes (`npm trust`,
#   npm 11.10+), so the next release can publish it from GitHub Actions.
# - A failure does not stop the run; a summary is printed at the end.
# - The main package `qrsend-cli` is published last, and only when every platform
#   package exists afterwards (its optionalDependencies must all resolve), unless
#   --main is given.
#
# Binaries come from the GitHub Release v<version> (qrsend-<version>-<target>
# archives); README and LICENSE from the v<version> tag.
#
# Usage: scripts/npm-publish-remaining.sh <version> [--skip NAME]... [--try-name PKG=NAME]... [--trust] [--main] [--dry-run]
set -uo pipefail

VERSION="${1:?usage: scripts/npm-publish-remaining.sh <version> [--skip NAME]... [--try-name PKG=NAME]... [--trust] [--main] [--dry-run]}"
shift
DRY=""; FORCE_MAIN=0; TRUST=0; SKIP=(); TRY=()
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) DRY="--dry-run" ;;
    --main) FORCE_MAIN=1 ;;
    --trust) TRUST=1 ;;
    --skip) SKIP+=("${2:?--skip needs a package name}"); shift ;;
    --try-name) TRY+=("${2:?--try-name needs PKG=NAME}"); shift ;;
    *) echo "unknown option $1" >&2; exit 2 ;;
  esac
  shift
done

REPO=igarinpiano/qrsend
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'git -C "$ROOT" worktree remove --force "$WORK/src" 2>/dev/null || true; rm -rf "$WORK"' EXIT

set -e
# Unauthenticated publishes of new packages come back as a misleading
# "404 Not Found", so check the login first (npm login sessions expire).
if [ -z "$DRY" ] && ! who="$(npm whoami 2>/dev/null)"; then
  echo "npm: not logged in (or the session expired). Run \`npm login\` and try again." >&2
  exit 1
fi
[ -n "$DRY" ] || echo "== npm user: $who"
git -C "$ROOT" fetch --tags --quiet
git -C "$ROOT" worktree add --quiet --detach "$WORK/src" "v$VERSION"
echo "== downloading release binaries of v$VERSION"
mkdir -p "$WORK/assets" "$WORK/binaries" "$WORK/x"
gh release download "v$VERSION" -R "$REPO" -D "$WORK/assets" -p "qrsend-$VERSION-*"
for a in "$WORK/assets"/*; do
  base="$(basename "$a")"
  target="${base#qrsend-$VERSION-}"; target="${target%.tar.gz}"; target="${target%.zip}"
  mkdir -p "$WORK/binaries/$target"
  case "$a" in
    *.zip) unzip -q -o "$a" -d "$WORK/x" ;;
    *.tar.gz) tar -xzf "$a" -C "$WORK/x" ;;
  esac
  cp "$WORK/x/qrsend-$VERSION-$target"/qrsend* "$WORK/binaries/$target/"
done
python3 "$ROOT/npm/assemble.py" --version "$VERSION" --binaries "$WORK/binaries" --out "$WORK/npm" --docs "$WORK/src"
set +e

published() { npm view "$1@$VERSION" version > /dev/null 2>&1; }
skipped() { local s; for s in "${SKIP[@]+"${SKIP[@]}"}"; do [ "$s" = "$1" ] && return 0; done; return 1; }

declare -a REPORT
missing=0
accepted=0

# Lets publish-all.yml publish later versions of a package through OIDC.
trust() {
  [ "$TRUST" = 1 ] && [ -z "$DRY" ] || return 0
  if npm trust github "$1" --file publish-all.yml --repo "$REPO" --env npm --allow-publish --yes; then
    REPORT+=("trusted publisher  $1")
  else
    REPORT+=("no trusted publisher added for $1 (see the npm error above; \`npm trust list $1\` shows what is registered)")
  fi
}

for t in "${TRY[@]+"${TRY[@]}"}"; do
  from="${t%%=*}"; to="${t#*=}"
  if [ ! -d "$WORK/npm/$from" ]; then
    REPORT+=("name not tried     $to — no package $from in this release")
  elif published "$to"; then
    REPORT+=("name accepted      $to@$VERSION is already published")
  else
    mkdir -p "$WORK/try/$to"
    cp -R "$WORK/npm/$from/." "$WORK/try/$to/"
    NAME="$to" node -e 'const fs = require("fs"), f = process.argv[1], p = JSON.parse(fs.readFileSync(f)); p.name = process.env.NAME; fs.writeFileSync(f, JSON.stringify(p, null, 2) + "\n")' "$WORK/try/$to/package.json"
    echo
    echo "== trying the name $to (a copy of $from@$VERSION)"
    if (cd "$WORK/try/$to" && npm publish --access public $DRY); then
      REPORT+=("name accepted      $to@$VERSION${DRY:+ (dry run)} — published as a copy of $from")
      trust "$to"
      [ -n "$DRY" ] || accepted=$((accepted + 1))
    else
      REPORT+=("name REJECTED      $to (see the npm error above)")
    fi
  fi
done
publish_dir() {
  local dir="$1" name
  name="$(node -p "require('$dir/package.json').name")"
  if published "$name"; then
    REPORT+=("already published  $name@$VERSION")
  elif skipped "$name"; then
    REPORT+=("skipped            $name"); return 1
  else
    echo
    echo "== npm publish $name@$VERSION"
    if (cd "$dir" && npm publish --access public $DRY); then
      REPORT+=("published          $name@$VERSION${DRY:+ (dry run)}")
      [ -n "$DRY" ] && return 1  # still not on the registry
      trust "$name"
    else
      REPORT+=("FAILED             $name@$VERSION (see the npm error above)"); return 1
    fi
  fi
  return 0
}

if [ "$accepted" -gt 0 ]; then
  echo
  echo "== summary"
  printf '  %s\n' "${REPORT[@]}"
  echo
  echo "npm accepted a tried name, so nothing else was published. Use that name in"
  echo "npm/assemble.py and npm/launcher.js, then run this script again without --try-name."
  exit 3
fi

for dir in "$WORK/npm"/qrsend-bin-* "$WORK/npm"/@qrsend/*; do
  [ -d "$dir" ] || continue
  publish_dir "$dir" || missing=$((missing + 1))
done

if [ "$missing" -eq 0 ] || [ "$FORCE_MAIN" = 1 ]; then
  publish_dir "$WORK/npm/qrsend-cli" || true
else
  REPORT+=("held back          qrsend-cli@$VERSION — $missing platform package(s) not on the registry yet (use --main to publish anyway)")
fi

echo
echo "== summary"
printf '  %s\n' "${REPORT[@]}"
