#!/usr/bin/env bash
# Makes the icon `qrsend send` shows in the macOS Dock
# (crates/qrsend/assets/dock-icon.png) from the artwork: a square picture that
# fills its canvas (assets/qrsendIcon-1024@1x.png).
#
# The system shows the picture it is handed as it is, so the margin its own
# icons keep (the shape takes 824 of 1024) is added here, and the picture is
# made small enough to sit inside the program: 512 pixels, 8 bits, sRGB.
#
# Needs Google Chrome (it does the scaling and the colors).
#
# usage: scripts/dock-icon.sh [ARTWORK.png]
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
art="${1:-$root/assets/qrsendIcon-1024@1x.png}"
out="$root/crates/qrsend/assets/dock-icon.png"
chrome="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cp "$art" "$work/art.png"
cat > "$work/icon.html" <<HTML
<!doctype html>
<body style="margin:0;background:transparent;overflow:hidden">
<img src="art.png" style="position:absolute;left:50px;top:50px;width:412px;height:412px">
HTML
"$chrome" --headless --disable-gpu --hide-scrollbars --force-device-scale-factor=1 \
  --window-size=512,512 --default-background-color=00000000 \
  --user-data-dir="$work/profile" --screenshot="$work/out.png" "file://$work/icon.html" 2>/dev/null
mkdir -p "$(dirname "$out")"
mv "$work/out.png" "$out"
echo "wrote $out ($(wc -c < "$out" | tr -d ' ') bytes)"
