# Progress

Product name **Glide** (crate and binary `glide`, bundle `com.heyfabrika.glide`).
Milestones and parity track the checklist in `analysis/SCREEN_STUDIO.md`.

| # | Milestone | Status |
|---|---|---|
| M0 | Reverse engineering with REA, parity checklist | ☑ `analysis/SCREEN_STUDIO.md` |
| M1 | Engine: springs, deterministic animator, timeline map, sizes, auto zoom + targeting, cursor model, click effects, camera layouts, shortcut badges, captions | ☑ unit tested |
| M2 | GPU renderer (wgpu/Metal): background, shadow, rounded window + inset, cursor, click FX, motion blur, camera, text, masks, device frames | ☑ |
| M3 | Media: AVAssetReader decode with seeking and prefetch | ☑ |
| M4 | Recording: ScreenCaptureKit display/window/area, system audio, mic, camera, input + cursor logging, iOS devices | ☑ (capture core from rec-rs) |
| M5 | Export: MP4 (VideoToolbox H.264/HEVC, zero-copy), audio mix with cuts/speed/voice-over ducking, GIF | ☑ |
| M6 | Editor UI (egui + Inter): one editor-first window (recording setup, recents popover), preview, inspector, timeline, recording widget, area/window pickers, menu bar, shortcuts | ☑ |
| M7 | Captions (Apple Speech, on device, editable), shortcut overlay | ☑ |
| M8 | App bundle, icon, signing (`scripts/build.sh`) | ☑ |
| M9 | Verification with real recordings; `PERFORMANCE.md` | ☑ `scripts/selftest.sh --screen`, `scripts/bench.sh` |

## Feature parity

☑ done and verified on this Mac · ◐ done with a stated gap · ☐ not done ·
✗ deliberately different (see "Deliberate differences" in the analysis).

| ID | Feature | Status | Notes |
|---|---|---|---|
| R1 | Display recording | ☑ | Native Retina size, display refresh, cursor hidden (re-rendered later). |
| R2 | Window recording | ☑ | Window bounds logged over time; cursor stays aligned when the window moves. Picked on screen ("Pick on screen…", Record with no window chosen, or the menu bar's Record Window…): windows outlined front to back, the one under the pointer highlighted with name, size, Resize, Use/Record and Adjust area; right-click for Bring to front / Ignore / Reset ignored; Esc cancels. Like Screen Studio, the pickers cover every display at once (one overlay per display; each shows its own windows, one shared selection; an area moves to the display you drag on). The scripted GUI run opens it on both displays here, highlights a window, screenshots each display and cancels. |
| R3 | Area recording | ☑ | Drag-to-select picker (move/resize, Return/Esc), size presets, exact values. |
| R4 | System audio | ☑ | Own track (stems). Everything the Mac plays, whatever the source: window recordings take it from a companion audio-only display stream (a window filter alone only hears the window's own app; before the fix a window recording was silent while another app played). ScreenCaptureKit only hears apps, not command-line players such as `afplay`. The app leaves its own sounds out; `glide record` doesn't (ScreenCaptureKit treats every copy of Glide as one app, so excluding "itself" also muted a Glide window playing). Checked by the selftest's audio round trip (a helper app plays a 1234 Hz tone, Glide records it from an area and from another app's window). |
| R5 | Microphone | ☑ | Own track; "Enhance voice" applies ML voice isolation, levelling and automatic gain control (±9 dB, speech-gated) at export rather than at capture, so the raw track stays untouched. Live capture checked with Microphone granted to the terminal: both capture paths (ScreenCaptureKit and AVCaptureSession) write a 48 kHz mono track in sync with the video. On this Mac it was silent (−91 dB, same with ffmpeg) because the lid is closed and macOS disconnects the built-in mic; Glide now warns about that before recording and in the voice-over panel. |
| R6 | Camera | ◐ | Separate 720p file, bubble/layouts in the editor. Not exercised live: Camera permission not granted on this Mac. |
| R7 | iOS device recording | ◐ | CoreMediaIO screen-capture devices, native-size HEVC, phone/tablet frame. Everything after capture is tested with a synthetic portrait device recording (`glide synth --device`: phone frame chosen automatically, no cursor, MP4 export) in `scripts/selftest.sh`. Live capture untested: no iPhone/iPad was connected. |
| R8 | Input logging | ☑ | 120 Hz mouse + buttons, cursor images with hotspots, keystrokes (needs Input Monitoring). |
| R9 | Countdown, pause/resume, restart, cancel | ☑ | Floating control bar like Screen Studio's: a native borderless, non-activating panel (clicking it doesn't bring Glide forward or change the recorded menu bar) shaped as a pill with a vibrancy background, hairline border and system shadow; pulsing red dot (orange when paused), monospaced timer, SF Symbol cancel / restart / pause buttons with hover highlights and tooltips (with shortcuts), red stop button; dragged by its background, remembered for the session; centred above the Dock on the recorded display. Kept out of recordings (own-window exclusion plus `sharingType = none`): checked on a real 5K recording of the display it sat on. The scripted GUI run checks it's present and draggable. |
| R10 | Hide desktop icons / Dock | ☑ | Excluded from the capture by window layer; wallpaper kept. Verified on a real recording. |
| R11 | Global shortcuts | ☑ | Start/stop, pause, cancel; re-bindable; conflicts reported. |
| R12 | Permissions | ☑ | Status, request button, exact steps below. |
| R13 | Resize the recorded window | ☑ | Window source shows the window's size with a Resize menu: the same preset groups, saved sizes, custom size; sizes that don't fit its display are disabled. Accessibility API (needs Accessibility permission). Verified by resizing Glide's own window (`scripts/selftest.sh --screen`). |
| Z1–Z7 | Auto zoom, zoom model, targeting, manual target, edge snapping, spring zoom, motion blur | ☑ | Engine unit tests; manual target picked by clicking the preview. |
| C1–C9 | Cursor re-rendering, smoothing, tilt, press, idle hide, hidden slices, click circle/ripple, click sounds, loop/freeze | ☑ | System cursor images, plus generated vector and touch skins. |
| S1 | Backgrounds | ☑ | Generated wallpapers (soft colour fields crossed by crisp "glass" ribbons, so blur visibly softens them), gradients, colour, imported PNG/JPEG (checked on import), current desktop picture, blur. Every recording, full-screen ones included, starts with padding so the background is visible; at 0 % padding a background, blur or shadow edit brings the padding back (8 %) and says so. `glide check-background` and `tests/background.rs` render each option and check the pixels change, on the editor preview and on a real 5K recording; an export test checks the exported frame matches the preview. |
| S2–S7 | Padding, roundness, shadow, inset, aspect ratio, crop | ☑ | Roundness and shadow sizes are px in a 1080p-tall frame (like blur), whatever the recording's resolution, so they look the same on Retina and 1080p recordings. |
| S8 | Device frames | ☑ | Generated laptop, phone, tablet frames (no artwork reused). |
| K1–K5 | Camera bubble, shape, shrink while zoomed, layouts, auto-hide | ◐ | Bubble, circle/aspect shapes and camera-only layout verified with a generated camera clip on a real recording; live camera capture unverified (permission). |
| T1–T7 | Slices, trim, split/delete, speed, zoom track, layout track, undo/redo/scrub/loop | ☑ | |
| T8 | Typing speed-up | ☑ | Detects typing runs in the key log; one click to 2–4×. |
| T9 | Masks | ☑ | Blur, pixelate, highlight; drawn on the preview; follow zoom. "Data may be visible": blur/pixelate masks are re-checked in the background (SSIM + histogram on 64×64 patches every 100 ms, one shared decode pass) and ranges where the covered content moved are flagged on the timeline; one click cuts them. Verified on a real 5K recording (only the mask over a playing video flagged, three static masks not) and by the UI smoke test. |
| T10 | Voice-over | ◐ | Recorded with AVAudioRecorder, mixed with ducking (mixing verified with a test file). Live capture needs Microphone permission. |
| O1 | Shortcut overlay | ☑ | |
| O2 | Captions | ☑ | Whisper (whisper.cpp with Metal; base/small/medium, downloaded on first use) or Apple Speech, both on device, word timings, editable lines. Verified with speech synthesized by `say` attached as a mic track: Whisper base returned all 20 words exactly, 7 s of speech in 0.4 s (plus a test that repeats this whenever the model is present). Apple Speech needs Speech Recognition permission (run from Glide.app) and wasn't exercised live. |
| A1–A2 | Mix, background music | ☑ | Per-slice volumes, mutes, master volume, looped music with fade. |
| A4 | Editor playback audio | ☑ | Plays through CoreAudio (cpal) from the edit's mix, in step with the playhead (re-seeked past 60 ms of drift; ≤ 45 ms measured). The master volume applies live in the output callback; mutes, clip volumes, music and voice-overs re-mix in the background (40–180 ms for 74 s) and swap in while playing; playing before the mix is ready picks up in step. The output restarts itself if the device changes or stops (another app reconfiguring it). Replaces AVPlayer on a temporary WAV, which stayed silent for a whole playback if the mix wasn't ready yet and ignored edits until paused. The Audio tab says when a track is silent or the mic was off. Checked by `GLIDE_GUI_PLAYTEST` (output level, ×0.25 volume = −12.0 dB, mute = silence, seek in sync) while another recording hears the speakers' tone. |
| A3 | Enhance microphone | ☑ | Apple's on-device ML voice isolation (AUSoundIsolation) run offline, then levelled: noise floor −45 → −90 dB on a test clip, ~60× real time. DSP fallback. |
| E1–E3 | MP4, resolution, frame rate | ☑ | Hardware H.264/HEVC, quality presets by bits per pixel. |
| E4 | GIF | ☑ | Parallel NeuQuant palettes, delta frames (only changed tiles, transparent elsewhere), duplicate merging, loop option. |
| E5 | Targets | ✗ | File, clipboard, reveal in Finder. No upload/share links (no accounts). |
| E6 | Presets / size estimate | ☑ | |
| E7 | Progress, cancel, quick export | ☑ | "Export & copy" banner after recording. Quitting, going back, or opening another recording during an export asks before cancelling it. That includes a recording that finishes while an export runs. A recording that fails to open leaves the export running. |
| E8 | Size-targeted export | ☑ | Not in Screen Studio 3.7.1; Glide's "Fit to file size" (MP4) stays at or under the target, re-encoding once if the first pass overshoots. |
| P1 | Projects library | ☑ | `~/Movies/Glide`, autosave. Editor-first window (Micky's request): no separate library or New-recording screen; recent recordings (with rendered thumbnails) are a popover off the window title, plus Open… and the projects folder. |
| P5 | Live source preview | ☑ | Recording setup shows the selected display, area or window live, filling the canvas under the Record button (Micky's request; Screen Studio does the same). Separate ScreenCaptureKit stream at 10 fps, 1280 px, cursor shown, Glide's own windows excluded; restarts when the source changes (after 300 ms of no edits), stops while recording, when the window is hidden, minimised or covered, and when a recording is open. iPhone sources show no preview. The GUI selftest checks frames arrive and that it's off while recording. |
| P2 | Style presets | ☑ | Save/apply/delete, default preset for new recordings. |
| P3 | Editor sidebar + selection inspector | ☑ | |
| P4 | Menu bar | ☑ | Record display/window/area, pause, status with elapsed time. |

## Verification on this MacBook (M4 Pro, macOS 27)

`scripts/selftest.sh --screen` (2026-10-12): 13/13 pass.

- `cargo test` (251): engine, project, presets, renderer helpers, masks,
  device frames, typing detection, caption editing, transcription chunking,
  Whisper transcription of synthesized speech,
  hotkeys, timeline clamping and cuts, voice isolation, masked-content
  change detection, window picking, GIF quantisation, icon, plus a
  headless UI smoke test driving every editor panel and background tests
  that edit each Background setting through the editor, read the preview's
  pixels back and compare them (and the exported frame) before and after,
  and audio tests (track levels and the silence note, the playback mix
  following mutes while open, the exported MP4 carrying the tone, tone
  detection under louder sound), a snapshot render, timeline pointer drags
  and unreadable-project errors.
- Synthetic package → MP4 and GIF.
- Background check (`glide check-background`): 22 edits (each wallpaper,
  blur on and off, gradient, its angle and colours, two colours, an image,
  image blur, padding, roundness, shadow) each visibly change a rendered
  frame; run on the synthetic package and on the real recording.
- Synthetic iPhone recording (1180×2556, kind = device) → phone frame → MP4.
- Scripted GUI run (`GLIDE_GUI_SELFTEST=1`): launch (opens on recording
  setup) → area picker opens over the display (screenshotted) and is
  confirmed → window picker opens on every display, highlights a window
  (screenshotted) and is cancelled → record (test pattern) → pause/resume →
  stop → editor opens the new project → export → Back to recording setup →
  the recording heads the recents list → reopen it → quit.
- Real 5K display recording (Screen Recording granted to the terminal) →
  export, and the background check on it.
- Audio round trip (`--audio`, part of `--screen`): a helper app plays a
  1234 Hz tone → Glide records it (5.7 s of 6 heard) → the editor plays the
  recording while a second recording listens (5.8 s heard; ×0.25 volume
  measured at −12.0 dB, mute silent, seek in sync) → the export carries it
  → a recording of another app's window hears it too → the microphone
  hears it through the speakers (reported only: depends on lid/headphones).
- Window resize: Glide's own window resized to 1024×700 through the
  Accessibility API and read back. Masks, device frames and voice-over mixing checked on real
  footage with `glide frame` and ffmpeg.

UI layouts are checked with `glide snapshot`, which renders the real UI
headlessly to PNG (home, every inspector tab, zoom/mask selection, export
sheet, post-recording banner; light and dark).

## What Micky needs to do (permissions)

The app bundle has its own privacy identity (`com.heyfabrika.glide`), signed
with the Apple Development identity so grants survive rebuilds.

1. `scripts/build.sh`, then `open dist/Glide.app`.
2. Press **Request access** on the home screen (or start a recording). Then in
   **System Settings › Privacy & Security**:
   - **Screen & System Audio Recording**: enable **Glide** (needed for any
     screen, window or area recording and for system audio). Quit and reopen
     Glide afterwards; macOS only applies this on relaunch.
   - **Microphone**: enable **Glide** (prompted the first time a recording or
     voice-over uses the microphone).
   - **Camera**: enable **Glide** (prompted the first time a recording uses
     the camera).
   - **Input Monitoring**: add/enable **Glide** to record keyboard shortcuts
     for the shortcut overlay and typing speed-up (mouse and cursor logging
     need no permission).
   - **Speech Recognition**: allow when prompted by *Transcribe microphone*
     in the Captions tab.
   - **Accessibility**: enable **Glide** to use *Resize* on the Window source
     (macOS asks the first time).
3. To try iPhone/iPad recording: connect the device with a cable, unlock it,
   tap *Trust*, then pick **iPhone** as the source (`glide devices` lists it).
4. Optional end-to-end check from Terminal (uses the terminal's own grants):
   `scripts/selftest.sh --screen` and `scripts/bench.sh`.

Already verified without your input: Screen Recording, Microphone and
Accessibility work from the terminal (real 5K recordings and exports, a
live mic track, window resizing above). The MacBook's lid was closed during
testing, so the built-in microphone delivered silence; to hear real speech
in a test, open the lid or use an external microphone. Camera and Speech
Recognition have not been granted on this Mac, so live camera capture and
transcription were exercised only through their code paths and test media.

## Log

- 2026-10-12: recording setup redesigned after Rec (Micky's request): the
  selected source live fills the canvas, with the camera in a round bubble;
  a right sidebar with SCENE / SCREEN / CAMERA / AUDIO / OPTIONS and recent
  recordings (Open a recording…); a bottom bar with a small record button,
  timer and live System / Mic meters. Camera and iPhone previews
  (AVCaptureSession, 640 px); system level from the preview stream, mic
  level from CoreAudio. Window fix: a saved window id that went stale
  (app relaunched) is found again by app and title, else cleared so Record
  asks; untitled windows the picker offers now show in the window list.
- 2026-10-11: REA analysis of Screen Studio 3.7.1; engine core with tests.
- 2026-10-11: wgpu renderer, AVFoundation decode, VideoToolbox/GIF export,
  recorder with input logging; zero-copy export path.
- 2026-10-11: egui app (library, editor, recording widget, menu bar, hotkey),
  on-device captions, Glide.app bundle with generated icon, self-tests,
  performance measurements.
- 2026-10-11: style presets, desktop-wallpaper background, typing speed-up,
  loop playback, caption editing, masks, area picker, hide icons/Dock,
  configurable shortcuts, device frames, voice-over, iOS device capture,
  menu bar sources, quick export, Finder document opening, GIF delta frames.
- 2026-10-11: "Data may be visible" warnings for masks (`glide check-masks`);
  resize the recorded window (`glide resize-window`); on-screen window
  picker covering every display; warning when the built-in mic is off with
  the lid closed; Whisper captions.
- 2026-10-12: audio fix (Micky's report: recordings had no sound in the
  editor). Editor playback rebuilt on CoreAudio with a live mix; window
  recordings now hear every app; `glide record` no longer mutes a running
  Glide; the Audio tab explains silent tracks and a mic that was off (his
  recordings had the mic off and, mostly, nothing playing); exports no longer
  leave `.sb-…` temporaries next to the file; `glide audio-check` and the
  selftest's audio round trip.
- 2026-10-11: new app icon (Micky's request: no purple): a Big Sur
  superellipse tile in graphite fading to near-black, a white cursor
  gliding in on a tapered curved trail, and a small red record dot; a test
  checks it has no purple pixels.
- 2026-10-11: background settings fix (Micky's report: no visible effect).
  Full-screen recordings defaulted to 0 % padding, which hides the
  background entirely; blur had nothing to soften on the smooth generated
  wallpapers; roundness and shadow were in recording pixels, so a third of
  the intended size on Retina recordings. Pixel-checked in tests and the
  selftest.
- 2026-10-11: native floating recording control bar.
- 2026-10-11: live preview of the selected source on the recording canvas.
- 2026-10-11: editor-first window: recording setup in the editor layout,
  recents popover off the title, compact Record + source menu in the
  editor's title bar; no separate library screen.
- 2026-10-11: "Fit to file size", Enhance voice (ML isolation + AGC),
  fixes from a full code review (preview audio lifecycle, timeline clamps,
  area bounds, voice-over/editor lifecycle, quit and restart behaviour).

- Live preview is Retina-sharp, as in Rec: screens are captured at native
  pixel size (`SCCaptureResolutionType::Best`, sRGB, 30 fps), Lanczos-scaled
  on the GPU (MPS) to exactly the canvas's pixels, and drawn 1:1,
  pixel-aligned, from the frame's IOSurface (no CPU copy or upload). Camera
  bubble at 720 px / 30 fps.

- Full screen fix (Micky's report: the UI breaks in full screen). The
  traffic-light inset moved AppKit's full-screen titlebar views, leaving a
  stray band and line at the top; the toolbar kept 88 pt for traffic lights
  that full screen hides; and the preview excluded only the windows Glide had
  at start, so the full-screen titlebar window showed up in it. Now the inset
  skips full screen, the toolbar margin follows it, the preview excludes
  Glide by application, and when Glide covers the display it would record the
  preview says so instead of showing a black void. Checked at min size,
  maximised, full screen and back.

- Export quality (Micky's report: exports look soft). Exports default to
  Native: the recording's own pixels, unscaled (e.g. 4654×3200 for a
  4112×2658 Retina display with padding; up to 8192×4320), with 4K, 1440p,
  1080p and 720p presets. Above H.264's 4096×2304 limit the export uses HEVC
  (HEVC Main, auto level). Quality presets are VideoToolbox constant quality
  (Studio 0.95, Social 0.85, …) instead of bits-per-pixel, and ProRes 4444
  (.mov, 4:4:4) is offered for lossless-looking masters. Downscaled exports
  resample the screen with a Catmull-Rom filter stretched to the footprint
  instead of bilinear (which aliased text into moiré). Exports are tagged
  BT.709 primaries/matrix with the sRGB transfer function — what the pixels
  are — so QuickTime/Safari show them without a gamma shift. Recordings use
  constant-quality HEVC (0.95). Measured on a full-display fine-text
  recording (export frame vs the recording, Lanczos-scaled for presets):
  1080p 31.4 → 33.9 dB, 4K preset 37.2 → 39.3 dB (now at the 4:2:0 ceiling),
  default 1080p → native 48.3 dB / SSIM 0.992; ProRes 49.2 dB / 0.996.
  `tests/resample.rs` checks native is pixel exact and downscales don't alias.
