#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

BROWSERSHOT="${BROWSERSHOT:-$HOME/repos/_tools/browsershot/browsershot}"
MAGICK="${MAGICK:-magick}"
MANUAL_DIR="$ROOT_DIR/demo/filesystem/screenshots/manual"
OUT_DIR="${OUT_DIR:-$ROOT_DIR/docs/assets/screenshots}"
ADDRESS="cloud.example.com"
IMASK=(--imask top:720px left:1280px)

if [[ ! -x "$BROWSERSHOT" ]]; then
  echo "browsershot executable not found: $BROWSERSHOT" >&2
  exit 1
fi

if ! command -v "$MAGICK" >/dev/null 2>&1; then
  echo "ImageMagick not found. Install it or set MAGICK=/path/to/magick." >&2
  exit 1
fi

mkdir -p "$OUT_DIR"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

drag_overlay="$TMP_DIR/dragged-file-overlay.png"

"$MAGICK" \
  "$MANUAL_DIR/drag-source.png" \
  -crop 200x180+250+115 +repage \
  \( -size 200x180 xc:none -fill white -draw 'roundrectangle 0,0 199,179 12,12' \) \
  -alpha set -compose CopyOpacity -composite \
  "$drag_overlay"

"$BROWSERSHOT" \
  "${IMASK[@]}" \
  "$MANUAL_DIR/context-menu.png" \
  "$MANUAL_DIR/thumb-mp3.png" \
  "$MANUAL_DIR/table-view.png" \
  --scale 10% \
  --top 0 \
  --left 420 \
  --cursor 490,490 \
  --address "$ADDRESS" \
  -o "$OUT_DIR/main-app.png"

"$BROWSERSHOT" \
  "${IMASK[@]}" \
  "$MANUAL_DIR/drop-target-table.png" \
  "$MANUAL_DIR/drag-source.png" \
  --overlay "$drag_overlay,200,360,90%" \
  --top 0 \
  --right 700 \
  --scale 5% \
  --cursor 320,470 \
  --address "$ADDRESS" \
  -o "$OUT_DIR/drag-n-drop-support.png"

"$BROWSERSHOT" \
  "${IMASK[@]}" \
  "$MANUAL_DIR/column-view.png" \
  --address "$ADDRESS" \
  -o "$OUT_DIR/column-view.png"

"$BROWSERSHOT" \
  "${IMASK[@]}" \
  "$MANUAL_DIR/user-admin.png" \
  --address "$ADDRESS" \
  -o "$OUT_DIR/user-admin.png"

"$BROWSERSHOT" \
  "${IMASK[@]}" \
  "$MANUAL_DIR/share-menu.png" \
  "$MANUAL_DIR/preview-video.png" \
  --scale 10% \
  --top 0 \
  --right 880 \
  --address "$ADDRESS" \
  -o "$OUT_DIR/media-preview.png"

echo "Wrote screenshot assemblies to $OUT_DIR"
