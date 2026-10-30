#!/bin/sh
# Measure recording overhead and export speed on this Mac; prints a Markdown
# table body for PERFORMANCE.md. Usage: scripts/bench.sh [--no-screen]
# Outputs go to $GIMBAL_TEST_DIR (default ~/Movies/daisycap-tests)/bench.
set -eu
cd "$(dirname "$0")/.."
. scripts/env.sh
cargo build --release -q
G="$CARGO_TARGET_DIR/release/daisycap"
OUT="${GIMBAL_TEST_DIR:-$HOME/Movies/daisycap-tests}/bench"
rm -rf "$OUT"; mkdir -p "$OUT"
echo "# $(sysctl -n machdep.cpu.brand_string), $(sysctl -n hw.memsize | awk '{print $1/1073741824}') GB, macOS $(sw_vers -productVersion), $(date '+%Y-%m-%d %H:%M')"

# Recording: wall time, CPU time and peak memory of the whole recorder process.
rec() {
  name=$1; shift
  /usr/bin/time -l "$G" record -t 10 --dir "$OUT" --name "$name" "$@" >"$OUT/$name.out" 2>"$OUT/$name.time"
  awk -v n="$name" '
    / real /{real=$1; user=$3; sys=$5}
    /maximum resident/{rss=$1/1048576}
    END {printf "| record %s 10 s | %.1f s wall | %.2f s CPU (%.1f%% of one core) | %.0f MB peak |\n", n, real, user+sys, (user+sys)/real*100, rss}' "$OUT/$name.time"
}
rec pattern --pattern
if [ "${1:-}" != "--no-screen" ]; then
  rec display
fi

# Export: video seconds per wall-clock second.
exp() {
  src=$1; name=$2; shift 2
  line=$("$G" export "$OUT/$src.daisycap" -o "$OUT/$name" "$@" 2>/dev/null)
  echo "| export $src → $name $* | $(echo "$line" | sed 's/^[^:]*: //') |"
}
"$G" synth "$OUT/synth.daisycap" --duration 12 >/dev/null
exp synth 1080p60-social.mp4 --height 1080 --fps 60 --quality social
exp synth 1080p60-hevc.mp4 --height 1080 --fps 60 --codec hevc
exp synth 720p30-web.mp4 --height 720 --fps 30 --quality web
exp synth 4k60-studio.mp4 --height 2160 --fps 60 --quality studio
exp synth 720p.gif --format gif --height 720
if [ "${1:-}" != "--no-screen" ]; then
  exp display display-1080p60.mp4 --height 1080 --fps 60
  exp display display-4k60.mp4 --height 2160 --fps 60 --quality studio
  exp display display-720p.gif --format gif --height 720
fi
