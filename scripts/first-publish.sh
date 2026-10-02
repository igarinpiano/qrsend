#!/usr/bin/env bash
# First (manual) publish of a version to crates.io and npm. Later releases go
# through .github/workflows/publish-all.yml with Trusted Publishing; trusted
# publishers can only be configured for packages that already exist, hence
# this script.
#
# Prerequisites:
#   - The GitHub Release v<VERSION> exists with binaries named
#     qrsend-<VERSION>-<target> (run publish-all with only "GitHub Releases"
#     selected, and `ref` = the tag for an existing release) — npm packages
#     are built from them.
#   - `cargo login` (crates.io token) and `npm login` done on this machine.
#   - gh, python3, npm, cargo on PATH.
#
# Usage: scripts/first-publish.sh <version> [--dry-run] [--crates-only | --npm-only]
set -euo pipefail

VERSION="${1:?usage: scripts/first-publish.sh <version> [--dry-run] [--crates-only|--npm-only]}"
shift
DRY=""; DO_CRATES=1; DO_NPM=1
for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY="--dry-run" ;;
    --crates-only) DO_NPM=0 ;;
    --npm-only) DO_CRATES=0 ;;
    *) echo "unknown option $arg" >&2; exit 2 ;;
  esac
done

REPO=igarinpiano/qrsend
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'git -C "$ROOT" worktree remove --force "$WORK/src" 2>/dev/null || true; rm -rf "$WORK"' EXIT

git -C "$ROOT" fetch --tags --quiet
git -C "$ROOT" rev-parse --verify --quiet "refs/tags/v$VERSION" > /dev/null \
  || { echo "tag v$VERSION not found — create the GitHub Release first" >&2; exit 1; }
# Publish exactly what the tag contains, not the working tree.
git -C "$ROOT" worktree add --quiet --detach "$WORK/src" "v$VERSION"
SRC="$WORK/src"
have="$(grep -m1 '^version' "$SRC/Cargo.toml" | sed -E 's/version = "(.*)"/\1/')"
[ "$have" = "$VERSION" ] || { echo "Cargo.toml at v$VERSION says $have" >&2; exit 1; }

if [ "$DO_CRATES" = 1 ]; then
  echo "== crates.io: qrsend-core $VERSION"
  (cd "$SRC" && cargo publish -p qrsend-core --locked $DRY)
  if [ -z "$DRY" ]; then
    echo "== waiting for qrsend-core $VERSION in the index"
    for _ in $(seq 1 60); do
      code="$(curl -s -o /dev/null -w '%{http_code}' -H 'User-Agent: qrsend-first-publish' \
        "https://crates.io/api/v1/crates/qrsend-core/$VERSION")"
      [ "$code" = 200 ] && break
      sleep 5
    done
    echo "== crates.io: qrsend $VERSION"
    (cd "$SRC" && cargo publish -p qrsend --locked)
  else
    echo "(dry run: qrsend itself is checked by the real publish once qrsend-core $VERSION exists)"
    (cd "$SRC" && cargo package -p qrsend --locked --no-verify --list > /dev/null)
  fi
fi

if [ "$DO_NPM" = 1 ]; then
  echo "== npm: downloading release binaries"
  mkdir -p "$WORK/assets" "$WORK/binaries"
  gh release download "v$VERSION" -R "$REPO" -D "$WORK/assets" -p "qrsend-$VERSION-*"
  for a in "$WORK/assets"/*; do
    base="$(basename "$a")"
    target="${base#qrsend-$VERSION-}"; target="${target%.tar.gz}"; target="${target%.zip}"
    mkdir -p "$WORK/x" "$WORK/binaries/$target"
    case "$a" in
      *.zip) unzip -q -o "$a" -d "$WORK/x" ;;
      *.tar.gz) tar -xzf "$a" -C "$WORK/x" ;;
    esac
    cp "$WORK/x/qrsend-$VERSION-$target"/qrsend* "$WORK/binaries/$target/"
  done
  # Packaging tooling from this checkout (older tags may predate it); the
  # README and LICENSE shipped in the package come from the tag.
  python3 "$ROOT/npm/assemble.py" --version "$VERSION" --binaries "$WORK/binaries" --out "$WORK/npm" --docs "$SRC"
  # Platform packages first so the main package's optionalDependencies resolve.
  for d in "$WORK/npm"/qrsend-bin-* "$WORK/npm/qrsend"; do
    echo "== npm publish $(basename "$d")"
    (cd "$d" && npm publish --access public $DRY)
  done
fi

echo
echo "Done. Now add Trusted Publishing (see the header of .github/workflows/publish-all.yml):"
echo "  crates.io: qrsend-core, qrsend   — workflow publish-all.yml, environment crates-io"
echo "  npmjs.com: qrsend and every qrsend-bin-* package — workflow publish-all.yml"
