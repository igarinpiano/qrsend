#!/usr/bin/env bash
# Publishes the npm packages of <version> that are not on the registry yet —
# for resuming a first publish that stopped half-way (e.g. a qrsend-bin-win32-*
# package rejected by npm's spam detection until support whitelists it).
#
# - Packages already on the registry at <version> are left alone.
# - --skip NAME leaves a package out (repeatable).
# - A failure does not stop the run; a summary is printed at the end.
# - The main package `qrsend-cli` is published last, and only when every platform
#   package exists afterwards (its optionalDependencies must all resolve), unless
#   --main is given.
#
# Binaries come from the GitHub Release v<version> (qrsend-<version>-<target>
# archives); README and LICENSE from the v<version> tag.
#
# Usage: scripts/npm-publish-remaining.sh <version> [--skip NAME]... [--main] [--dry-run]
set -uo pipefail

VERSION="${1:?usage: scripts/npm-publish-remaining.sh <version> [--skip NAME]... [--main] [--dry-run]}"
shift
DRY=""; FORCE_MAIN=0; SKIP=()
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) DRY="--dry-run" ;;
    --main) FORCE_MAIN=1 ;;
    --skip) SKIP+=("${2:?--skip needs a package name}"); shift ;;
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
publish_dir() {
  local dir="$1" name
  name="$(basename "$dir")"
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
    else
      REPORT+=("FAILED             $name@$VERSION (see the npm error above)"); return 1
    fi
  fi
  return 0
}

for dir in "$WORK/npm"/qrsend-bin-*; do
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
