#!/bin/sh
# End-to-end checks on this Mac: unit + UI smoke tests, synthetic exports,
# the background-settings pixel check, the scripted GUI run (record test
# pattern -> pause/resume -> stop -> editor -> export -> quit) and, with
# --screen, a real 3 s display recording + export + background check, and
# the audio round trip (also alone with --audio): a tone from
# another app is recorded, played back in the editor (and heard), and
# exported. The audio checks play a quiet 1234 Hz tone through the speakers.
# Outputs go to $GIMBAL_TEST_DIR (default ~/Movies/daisycap-tests), never the repo.
set -eu
cd "$(dirname "$0")/.."
. scripts/env.sh
G="$CARGO_TARGET_DIR/release/daisycap"
OUT="${GIMBAL_TEST_DIR:-$HOME/Movies/daisycap-tests}/selftest"
rm -rf "$OUT"; mkdir -p "$OUT"
pass=0; fail=0
ok() { echo "PASS  $1"; pass=$((pass+1)); }
ko() { echo "FAIL  $1 (see $OUT)"; fail=$((fail+1)); }

cargo build --release -q
# --audio: only the audio round trip below.
if [ "${1:-}" != "--audio" ]; then
if cargo test --release -q >"$OUT/tests.log" 2>&1; then ok "cargo test ($(grep -o '[0-9]* passed' "$OUT/tests.log" | awk '{s+=$1} END {print s}') tests)"; else ko "cargo test"; fi

if "$G" synth "$OUT/synth.daisycap" --duration 6 >"$OUT/synth.log" 2>&1 \
   && "$G" export "$OUT/synth.daisycap" -o "$OUT/synth.mp4" >>"$OUT/synth.log" 2>&1 \
   && "$G" export "$OUT/synth.daisycap" --format gif -o "$OUT/synth.gif" >>"$OUT/synth.log" 2>&1 \
   && [ -s "$OUT/synth.mp4" ] && [ -s "$OUT/synth.gif" ]; then
  ok "synthetic package -> mp4 + gif"
else
  ko "synthetic export"
fi

# Every Background setting (wallpapers, blur, gradient, colour, image) and the
# padding, roundness and shadow around it must visibly change a rendered frame.
if "$G" check-background "$OUT/synth.daisycap" >"$OUT/background.log" 2>&1; then
  ok "background settings change the frame ($(grep -c '^ok' "$OUT/background.log") checks)"
else
  ko "background settings"
fi

# iPhone/iPad recordings (no device needed): portrait → phone frame → MP4.
if "$G" synth "$OUT/phone.daisycap" --device --duration 3 --width 1180 --height 2556 >"$OUT/phone.log" 2>&1 \
   && grep -q '"device": "phone"' "$OUT/phone.daisycap/project.json" \
   && "$G" export "$OUT/phone.daisycap" -o "$OUT/phone.mp4" >>"$OUT/phone.log" 2>&1 && [ -s "$OUT/phone.mp4" ]; then
  ok "synthetic iPhone recording -> phone frame + mp4"
else
  ko "synthetic iPhone recording"
fi

code=0
GIMBAL_GUI_SELFTEST=1 GIMBAL_OUTPUT_DIR="$OUT/gui" "$G" gui >"$OUT/gui.log" 2>&1 || code=$?
echo "exit status $code" >>"$OUT/gui.log"
if [ "$code" -eq 0 ] && grep -q 'selftest: PASS' "$OUT/gui.log"; then
  ok "gui app ($(grep -o 'elapsed=[0-9.]*s' "$OUT/gui.log"), $(grep -o 'frames: [0-9]*' "$OUT/gui.log"))"
else
  ko "gui app"
fi
fi

# Audio round trip. ScreenCaptureKit only hears apps, and treats every copy
# of DaisyCap as DaisyCap itself, so the tone comes from a small helper app
# (scripts/test-window.js under osascript). DaisyCap records it, plays the
# recording back in the editor (GIMBAL_GUI_PLAYTEST, which also checks the
# output follows the volume, mute and playhead) while a second recording
# listens, and exports it. `daisycap audio-check` looks for the tone each time.
helper() { osascript -l JavaScript scripts/test-window.js "$@" >/dev/null 2>&1; }
capturing() { # name pid — wait until the recording is running
  i=0
  until grep -q 'recording to' "$OUT/$1-record.log" 2>/dev/null; do
    i=$((i+1)); [ "$i" -gt 300 ] || ! kill -0 "$2" 2>/dev/null && break
    sleep 0.1
  done
}
record_with_tone() { # name [record options...] — 7 s; the helper plays the tone from 1 s in
  name=$1; shift
  "$G" record -t 7 --dir "$OUT" --name "$name" "$@" >"$OUT/$name-record.log" 2>&1 &
  rpid=$!
  capturing "$name" "$rpid"
  helper 8 "$TONE" 1 &
  hpid=$!
  wait "$rpid"; r=$?
  wait "$hpid" 2>/dev/null
  return $r
}
tone() { grep "^$1" "$2" | sed 's/.*Hz tone/tone/'; }
if [ "${1:-}" = "--screen" ] || [ "${1:-}" = "--audio" ]; then
  if "$G" permissions 2>/dev/null | grep -q 'Screen Recording: *Granted'; then
    "$G" synth "$OUT/tone-source.daisycap" --duration 8 --width 640 --height 400 --tone 1234 >/dev/null 2>&1
    TONE="$OUT/tone-source.daisycap/recording/system.wav"
    if record_with_tone tone --area 0,0,320,200 \
       && "$G" audio-check --freq 1234 "$OUT/tone.daisycap" --require system >"$OUT/tone-check.log" 2>&1; then
      ok "records another app's tone (system audio): $(tone system "$OUT/tone-check.log")"
    else
      ko "recording a tone"
    fi
    # DaisyCap plays the recording; a second recording (started first) listens.
    "$G" record -t 10 --dir "$OUT" --name heard --area 0,0,320,200 >"$OUT/heard-record.log" 2>&1 &
    rpid=$!
    capturing heard "$rpid"
    GIMBAL_GUI_PLAYTEST="$OUT/tone.daisycap" GIMBAL_PLAYTEST_SECONDS=6 GIMBAL_OUTPUT_DIR="$OUT/gui" "$G" gui >"$OUT/playtest.log" 2>&1 || true
    wait "$rpid"
    if grep -q 'playtest: PASS' "$OUT/playtest.log" \
       && "$G" audio-check --freq 1234 "$OUT/heard.daisycap" --require system >"$OUT/heard-check.log" 2>&1; then
      ok "editor playback is audible: $(tone system "$OUT/heard-check.log"); $(grep -o 'volume x0.25.*soundtrack' "$OUT/playtest.log")"
    else
      ko "editor playback of the recorded tone"
    fi
    if "$G" export "$OUT/tone.daisycap" -o "$OUT/tone.mp4" >"$OUT/tone-export.log" 2>&1 \
       && "$G" audio-check --freq 1234 "$OUT/tone.mp4" --require audio0 >>"$OUT/tone-export.log" 2>&1; then
      ok "export carries the tone: $(tone audio0 "$OUT/tone-export.log")"
    else
      ko "export of the recorded tone"
    fi
    # A window recording hears the whole Mac, not just the window's app:
    # record a silent helper window while another helper plays the tone.
    helper 40 &
    wpid=$!
    sleep 1.5
    win=$("$G" devices 2>/dev/null | grep 'DaisyCap audio test (' | grep -v '(sound)' | awk 'NR==1 {print $1}')
    if [ -z "$win" ]; then
      echo "SKIP  window recording audio (helper window didn't appear)"
    elif record_with_tone window-tone --window "$win" \
       && "$G" audio-check --freq 1234 "$OUT/window-tone.daisycap" --require system >"$OUT/window-check.log" 2>&1; then
      ok "window recording hears other apps: $(tone system "$OUT/window-check.log")"
    else
      ko "window recording audio"
    fi
    kill "$wpid" 2>/dev/null; wait "$wpid" 2>/dev/null || true
    # The microphone hears the tone through the speakers unless the lid is
    # closed or headphones are in, so that part is reported, not required.
    if "$G" permissions 2>/dev/null | grep -q 'Microphone: *Granted'; then
      if record_with_tone mic-tone --area 0,0,320,200 --mic --no-system-audio \
         && "$G" audio-check --freq 1234 "$OUT/mic-tone.daisycap" >"$OUT/mic-check.log" 2>&1 && grep -q '^mic: .*dBFS RMS' "$OUT/mic-check.log"; then
        if "$G" audio-check --freq 1234 "$OUT/mic-tone.daisycap" --require mic >/dev/null 2>&1; then
          ok "microphone recorded the tone from the speakers: $(tone mic "$OUT/mic-check.log")"
        else
          ok "microphone recorded, without hearing the speakers (lid closed or headphones?): $(grep '^mic' "$OUT/mic-check.log" | cut -d';' -f1)"
        fi
      else
        ko "microphone recording"
      fi
    else
      echo "SKIP  microphone (no Microphone permission)"
    fi
  else
    echo "SKIP  audio round trip (no Screen Recording permission)"
  fi
fi

if [ "${1:-}" = "--screen" ]; then
  if "$G" record -t 3 --dir "$OUT" --name screen >"$OUT/screen.log" 2>&1 \
     && "$G" export "$OUT/screen.daisycap" -o "$OUT/screen.mp4" >>"$OUT/screen.log" 2>&1 \
     && [ -s "$OUT/screen.mp4" ]; then
    ok "real display recording -> export"
  else
    ko "real display recording (Screen Recording permission?)"
  fi
  if [ -d "$OUT/screen.daisycap" ] && "$G" check-background "$OUT/screen.daisycap" >"$OUT/screen-background.log" 2>&1; then
    ok "background settings on the real recording ($(grep -c '^ok' "$OUT/screen-background.log") checks)"
  else
    ko "background settings on the real recording"
  fi
  # Resize DaisyCap's own window (only that exact window, by its process) through the
  # Accessibility API, as the Window source's Resize menu does.
  if "$G" permissions 2>/dev/null | grep -q 'Accessibility: *Authorized'; then
    GIMBAL_OUTPUT_DIR="$OUT/gui" "$G" gui >"$OUT/resize.log" 2>&1 &
    gpid=$!
    # Find its window by process id (CoreGraphics; no ScreenCaptureKit).
    resized=""
    for _ in 1 2 3 4 5 6 7 8 9 10; do
      sleep 1
      if "$G" resize-window "pid:$gpid" 1024x700 >>"$OUT/resize.log" 2>&1; then resized=1; break; fi
    done
    if [ -n "$resized" ] && grep -q '→ 1024×700' "$OUT/resize.log"; then
      ok "window resize (Accessibility)"
    else
      ko "window resize"
    fi
    kill "$gpid" 2>/dev/null; wait "$gpid" 2>/dev/null || true
  else
    echo "SKIP  window resize (no Accessibility permission)"
  fi
fi
echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]
