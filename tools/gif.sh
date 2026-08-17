#!/usr/bin/env bash
# Turn a directory of `capture` frames into an annotated GIF.
#
# `examples/capture.rs` writes frame0000.png … ; this is the other half of that pipeline, kept as a
# script rather than as remembered ffmpeg flags so that two GIFs taken a week apart are encoded
# identically and therefore actually comparable. A palette or dither change between runs would show up
# as a visual difference that no code change caused.
#
#   tools/gif.sh <frames-dir> <out.gif> "<caption>"
#
# The caption is burned in with ImageMagick because this ffmpeg build has no `drawtext` filter.
#
# LEGEND=audit (default) burns in the colour key `capture.rs --tint audit` applies, so the two must
# be changed together. LEGEND=none omits it, which is what the demo-tinted clips want: a key naming
# colours that are not in the picture is worse than no key at all.
set -euo pipefail

FRAMES=${1:?usage: gif.sh <frames-dir> <out.gif> "<caption>"}
OUT=${2:?usage: gif.sh <frames-dir> <out.gif> "<caption>"}
CAPTION=${3:-}

LEGEND=${LEGEND:-audit}
FONT=${FONT:-/System/Library/Fonts/Supplemental/Arial.ttf}
BOLD=${BOLD:-/System/Library/Fonts/Supplemental/Arial Bold.ttf}
WIDTH=${WIDTH:-640}
FPS=${FPS:-30}

[ -d "$FRAMES" ] || { echo "gif.sh: no such directory: $FRAMES" >&2; exit 1; }
count=$(find "$FRAMES" -name 'frame*.png' | wc -l | tr -d ' ')
[ "$count" -gt 0 ] || { echo "gif.sh: no frame*.png in $FRAMES" >&2; exit 1; }

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# Two-pass palette: one palette for the whole clip, so colours do not shift frame to frame.
ffmpeg -y -loglevel error -framerate "$FPS" -i "$FRAMES/frame%04d.png" \
  -vf "scale=${WIDTH}:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=128[p];[b][p]paletteuse=dither=bayer:bayer_scale=3" \
  "$tmp/raw.gif"

# Caption top-left, and the audit legend bottom-left when it describes what is on screen.
case "$LEGEND" in
  audit)
    magick "$tmp/raw.gif" -coalesce \
      -font "$BOLD" -pointsize 15 -fill white \
      -gravity NorthWest -annotate +12+10 "$CAPTION" \
      -font "$FONT" -pointsize 13 \
      -gravity SouthWest \
      -fill '#3d9e5c' -annotate +12+56 '●  watertight + manifold' \
      -fill '#e6a61f' -annotate +12+36 '●  watertight, non-manifold' \
      -fill '#d92eb8' -annotate +12+16 '●  open cut edges' \
      -layers optimize "$OUT"
    ;;
  none)
    magick "$tmp/raw.gif" -coalesce \
      -font "$BOLD" -pointsize 15 -fill white \
      -gravity NorthWest -annotate +12+10 "$CAPTION" \
      -layers optimize "$OUT"
    ;;
  *)
    echo "gif.sh: unknown LEGEND=$LEGEND (use 'audit' or 'none')" >&2; exit 1
    ;;
esac

echo "gif.sh: $count frames -> $OUT ($(du -h "$OUT" | cut -f1))"
