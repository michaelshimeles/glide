# Screen Studio 3.7.1: behaviour analysis and parity checklist

This document describes how Screen Studio (macOS, build 3.7.1-4399) behaves, so that this project
can reimplement the same features independently. It contains behaviour descriptions, data shapes
and numeric parameters written in our own words. It contains none of Screen Studio's code, assets,
sounds, wallpapers, cursors or fonts.

## Method

- Target: `Screen Studio.app` (Electron) with `app.asar` (265 MB) and `app.asar.unpacked` (native helpers).
- REA `analyze_javascript_application` built the application graph of the asar: 6,638 entries,
  2,926 parsed JavaScript files, 1 Electron main entry (`dist-electron/index.js`), 1 app renderer
  (`dist/index.html`, ~75 Vite chunks), 2 native addons (keychain access, macOS permissions),
  13 main-process IPC handlers, 4 context-bridge APIs and tRPC between renderer and main.
- REA `inspect_macho` on the Swift recorder helper (`polyrecorder`): universal x86_64/arm64,
  macOS 11+, SDK 26, links ScreenCaptureKit, AVFoundation, CoreMediaIO, CoreImage, Metal, IOKit,
  ApplicationServices (event taps) and Combine.
- The renderer bundles use a rotated-string-array obfuscation. We decoded the string table to read
  constants and data shapes, then documented the behaviour here.
- REA's deep native decompilation providers (Ghidra, Hopper, IDA) were not installed on the build
  machine, so native helpers were characterised from REA's Mach-O inventory, their strings, and how
  the main process drives them.

## Architecture

| Part | Role |
|---|---|
| Electron main process | Window management, tRPC router, project I/O, spawning helpers, ffmpeg/gifsicle for GIF, whisper for captions, uploads. |
| Renderer (React + MobX + PixiJS/WebGL) | Recording UI, editor, preview and export rendering. Export renders frames with PixiJS and encodes with WebCodecs `VideoEncoder` (H.264 `avc1.640033`, hardware preferred, constant bitrate, `latencyMode: quality`). |
| `polyrecorder` (Swift) | Capture of display, window, area, camera, microphone, system audio and iOS devices. Also logs mouse moves, clicks and keystrokes, cursor images and window bounds. Started with a JSON config on stdin; reports newline-delimited JSON status on stdout (`recordingPrepared`, `recordingResumed`, `recordingStoppedWithError`, …). Start, pause and resume are signals. A `discover` subcommand streams the available displays, windows and devices. |
| Other helpers | `list-windows`, `manage-window`, `hide-icons` (desktop icons), `macos-wallpaper`, `face-detector`, `dynamic-mask-tracker`, `audio-composer`, `deep-filter` (speech denoise), `transcribe-audio` (Apple Speech), `whisper` (whisper.cpp), `ffmpeg`, `gifsicle`, `sf-icon` (SF Symbols). |
| Unused helper | `face-detector <video> <interval_ms>` runs Vision face detection over the camera video and is exposed as the `faceBounds` query (1000 ms default, cached under the project's `cache/face/`), but no renderer bundle in 3.7.1 calls it (API names the UI uses, such as `detectMaskedContentShift`, appear in the renderer bundles; `faceBounds` appears in none), so it has no user-visible behaviour to match. |

## Project format

A project is a package directory `Name.screenstudio/`:

```
project.json            project data (below)
meta.json               app version metadata
recording-markers.json  markers added while recording
assets/                 user-added images and audio
transcripts/            microphone-session-N.json, voice-over-file-*.json
recording/
  metadata.json         sessions and recorders (display/window/webcam/mic/system audio/input meta)
  metadata-raw.json
  cursors.json          cursor images seen while recording (id, image, hotspot, size)
  channel-1-display-0.mp4     screen video for session 0 (cursor NOT drawn into it)
  channel-1-system-audio-0.m4a
  …webcam / microphone files per session
  mousemoves-0.json  mouseclicks-0.json  keystrokes-0.json
```

`project.json`:

```
{ id, name, createdAt, updatedAt, lastSavedAt,
  config: { …style config, see "Project config" … , recordingRange:[0,durationMs], recordingCrop:{x,y,width,height} (0..1) },
  meta: { recordingFlags: [] },
  scenes: [ { id, name:"Default", type:"recording", sessionIndex:0,
              zoomRanges:[…], slices:[…], layouts:[…], masks:[…],
              voiceOvers:[…], resolvedTypingSpeedIncreaseSuggestions:[] } ] }
```

- Pausing creates a new recording *session*. Each session has its own files, and the timeline concatenates them.
- Mouse moves carry `{x, y, cursorId, processTimeMs}` in screen points. On load, a move is dropped if the next
  move arrives within one display frame (1000/refresh-rate ms) or is less than 1 px from the last kept move,
  unless the cursor image changes. Clicks carry `mouseDown`/`mouseUp` with position and time. Drags are marked `mouseDragged`.
- Keystrokes carry `{character, activeModifiers[], type keyDown/keyUp, recordingTimeMs}`.
- Window recordings store `windowBoundChanges` over time, so a moving or resizing window keeps its framing.
- Recording frame rate = the display refresh rate (60 or 120 Hz). iOS device recordings use 60 fps.

## Project config (defaults)

| key | default | notes |
|---|---|---|
| backgroundType | `system` wallpaper | also `gradient`, `color`, `image` |
| backgroundGradient | 2-stop linear, (0,0)→(1,1) | stops {color, at} |
| backgroundBlur | 0 | |
| backgroundPaddingRatio | 10 | % of mean(screen w,h). Set to 0 for full-display recordings, kept for window/area/device recordings |
| windowBorderRadius | 12 | 0 for full display |
| shadowIntensity / Angle / Distance / Blur / IsDirectional | 0.75 / 90° / 25 / 20 / false | |
| insetPadding / insetColor / insetAlpha | 0 / #000 / 0.5 | inner border between the window edge and the content |
| defaultOutputAspectRatio | null (Auto) | Wide 16:9, Square 1:1, Classic 4:3, Vertical 9:16, Tall 3:4, Portrait 4:5 |
| cursorSize | 1.5 | |
| cursorSet | `macos` | |
| cursorRotateOnXMovementRatio | 0.5 | tilt |
| cursorBaseRotation | 0 | |
| hideNotMovingCursorAfterMs | null | |
| loopCursorPositionBeforeEndMs | null | |
| stopCursorMovementInLastPartMs | 0 | |
| removeCursorShakeThreshold | 500 | ms |
| hideCursor | false | |
| clickEffect | null | `circle`, `ripple{rippleStrength:2}` |
| clickSoundEffect | null | volume 0.25 |
| mouseMovementSpring | {470, 70, 3} | stiffness, damping, mass |
| screenMovementSpring | {200, 40, 2.25} | |
| mouseClickSpring | {700, 30, 1} | |
| disableMouseMovementSpring | false | |
| motionBlurAmount / Cursor / ScreenMove / ScreenZoom | 1 / 1 / 1 / 1 | |
| alwaysKeepZoomedIn | false | |
| hideCamera / mirrorCamera | false / false | |
| cameraSize / cameraRoundness / cameraScaleDuringZoom | 0.35 / 0.25 / 0.7 | |
| cameraPosition / cameraPositionPoint | bottom-right / {1,1} | |
| cameraAspectRatio | original | square, vertical, horizontal |
| defaultLayout | {type both, cameraSize 0.35, point {1,1}} | |
| showShortcuts / showShortcutsWithSingleLetters / shortcutsSizeRatio | false / false / 1 | |
| showTranscript / transcriptSizeRatio | true / 1 | |
| audioVolume | 1 | |
| muteMicrophone / muteSystemAudio | false / false | |
| improveMicrophoneAudio | true | |
| backgroundAudioFileName / backgroundAudioVolume | null / 0.05 | |
| deviceFrameKey / enableDeviceMockup | auto / true | |

App settings (defaults): countdown off, create automatic zooms on, hide the dock icon while recording,
open the editor after recording ("create-project"), quick-export widget on, auto-save on, max webcam height 1080,
preview max height 1080, playback quality "high-quality".

## Animation engine

**Spring.** Every animated property is a damped spring with `{stiffness k, damping c, mass m}`, Newtonian and
integrated with semi-implicit Euler in 1 ms substeps:
`a = (−k·(x − target) − c·v) / m; v += a·dt; x += v·dt`. Defaults are k=90, c=9, m=1. The rest threshold is
relative: `precision = max(1, |from − to|) · 0.002`. The spring snaps to its target when both |v| and |x − target|
fall below it. `m = 0` means instant. If the elapsed time exceeds the settle time (simulated once at 60 Hz steps,
capped at 20 s), the spring also snaps. A style object (position, scale, opacity, blur…) animates as one spring per
numeric leaf.

**Deterministic seeking.** For time t, the engine restarts the simulation at t − 3500 ms from the target value at
that time, then steps one output frame at a time. On each frame it sets the target to the style at that instant and
applies any per-time spring override. Results are cached per frame, with up to 60 frames computed eagerly. So the
rendered frame is a pure function of t: preview and export match, and scrubbing is stable.

**Presets.** Screen: *Focused* {200,40,2.25} (default) and *Smooth* {170,50,3}. Cursor: *Smooth* {470,70,3}
(default), *Medium* {340,60,3}, *Rapid* {530,40,1} and *None*. *Customize* exposes Rigidity (stiffness 5–600),
Smoothness (damping 5–200) and Momentum (mass 0.1–15).

## Feature parity checklist

Status legend: ☐ not started · ◐ partial · ☑ done (verified). The *Status* column is updated in `PROGRESS.md`.

### Recording
| # | Feature | Screen Studio behaviour |
|---|---|---|
| R1 | Display recording | ScreenCaptureKit display capture at native (Retina) resolution and the display refresh rate; system cursor hidden from frames. |
| R2 | Window recording | Single-window capture. Bounds changes are tracked over time so a moving or resizing window stays framed. Windows are picked on screen: a cover over the display (fed by the `list-windows` helper at a 200 ms interval) draws each capturable window at its position in z-order; hovering highlights one with its app icon, app name, size/Resize control and a record button; clicking selects it (clicking the cover deselects), Escape cancels. Buttons offer "Adjust recording area" (switch to an area starting from the window's bounds) and, for full-screen windows, "Ignore this window". The context menu has "Bring to front", "Ignore this window" and "Reset ignored windows" (ignored for the session). With no windows: "No windows to select" and "Record entire display". Screenshot tools' own windows are excluded. |
| R3 | Area recording | Drag-select a region. Recorded as a display capture plus a normalised crop. Saved size and crop presets. |
| R4 | System audio | Captured separately to its own track (m4a). |
| R5 | Microphone | Separate track. Auto gain control. Optional speech enhancement (denoise) in the editor. |
| R6 | Camera | Separate video (≤1080p) composited in the editor. Continuity Camera supported. |
| R7 | iOS device recording | USB iPhone/iPad screen via CoreMediaIO, with a device-frame mockup and 4:3 default output. |
| R8 | Input logging | Mouse moves, clicks and drags, keystrokes with modifiers, cursor image per move (with hotspot). |
| R9 | Countdown, pause/resume, restart, cancel | Optional countdown. Pause creates a new session. Recording widget/toolbar while recording. |
| R10 | Hide desktop icons / dock while recording | Options. |
| R11 | Global shortcuts | User-configurable global hotkeys to start, stop and pause. |
| R12 | Permissions | Screen Recording, Microphone, Camera, Accessibility (input monitoring). |
| R13 | Resize the recorded window | In window mode the highlighted window shows its size ("W × H") and a Resize button. The menu offers 16:9 sizes (1280×720, 1920×1080, 2560×1440) at the top and 4:3, 9:16, 16:10 and Square submenus, the user's saved sizes ("Save current size", "Delete saved sizes"), and "Custom..." (inline width × height fields, Enter confirms, Escape cancels). Sizes larger than the window's display, or equal to its current size, are disabled. Implemented by the `manage-window` helper (`<id> --resize=WxH`, also `--bring-to-front`, `--focus`) through the Accessibility API. |

### Automatic zoom and motion
| # | Feature | Behaviour |
|---|---|---|
| Z1 | Auto zoom generation | For every click at least 1 s before the end: range [t−300, t+2500] ms, clamped to duration−800. Touching ranges merge, then gaps ≤ 2500 ms are filled (repeated until stable). Each range: `follow-click-groups`, zoom 2×, snapToEdgesRatio 0.25. Off for iOS recordings. |
| Z2 | Zoom range model | `{id, type manual / follow-click-groups / follow-mouse, startTime, endTime, zoom, snapToEdgesRatio, manualTargetPoint, glideSpeed 0.5, isDisabled, hasInstantAnimation}`. Minimum length 1000 ms. Splits must be ≥ 100 ms from an edge. Gaps behave as zoom 1. |
| Z3 | Zoom target (auto) | Events in the range (or the nearest event before it) are clustered greedily while the cluster's bounding box fits within 50% width × 70% height of the zoomed viewport. The target is the centre of the latest cluster started at or before t. |
| Z4 | Zoom target (manual) | Fixed normalised point. |
| Z5 | Edge snapping and clamping | Target remapped per axis from [r, 1−r] to [0, 1] with r = min(snapRatio, visible/content). The zoomed body is placed by interpolating between left- and right-aligned positions, so the background never shows inside the frame. |
| Z6 | Smooth spring zoom | {scale, position} animated with screenMovementSpring. Instant-animation ranges snap within 100 ms of their edges. |
| Z7 | Motion blur | Per animated node, comparing on-screen bounds at frames f and f−1. Move: directional blur along velocity relative to the parent (21 taps, centred). Zoom: radial blur, strength |1 − diag ratio|. Strength × amount × fps/60. Separate amounts for cursor, screen move and zoom. |
| C1 | Cursor re-rendering | Cursor drawn from logged data at high resolution: height 40 px × cursorSize × image scale, anchored at the hotspot. macOS cursor types mapped to a cursor set, with a fallback when the app's cursor image is low-res. |
| C2 | Cursor smoothing | Target = last logged position, but if a click comes within 500 ms the target is the click point (removes shake). Spring per preset. Instant when disabled or inside a slice that disables smoothing. Rapid spring near clicks, stiff spring while dragging. |
| C3 | Cursor tilt | rotate = clamp((x(t) − x(t−400 ms)) × 0.03 × ratio, ±20°) + base rotation. |
| C4 | Press feedback | Cursor scales to 0.8 while the button is down, starting 130 ms before mouseDown. |
| C5 | Hide idle cursor | After N ms without movement → {alpha 0, scale 0.8, blur 5}. Reappears 250 ms before the next move. |
| C6 | Hide-cursor slices | Timeline slices can hide the cursor. |
| C7 | Click effects | Circle: grey disc radius 16 × cursorSize, 150 ms, scale 0.2→3.5, alpha peaks at 5%. Ripple: shockwave distortion of the screen at the click, 250 ms. |
| C8 | Click sounds | Optional, volume 0.25. |
| C9 | Loop and freeze cursor | Return to the start position in the last N ms (seamless loops), or freeze in the last N ms. |

### Styling
| # | Feature | Behaviour |
|---|---|---|
| S1 | Backgrounds | System wallpaper presets (categories), linear gradients, solid colour, custom image, macOS current wallpaper. Blur slider. |
| S2 | Padding | backgroundPaddingRatio % of the mean screen dimension on each side. |
| S3 | Rounded corners | windowBorderRadius px; inner radius = radius − mean inset (min 4). |
| S4 | Shadow | Intensity, angle, distance, blur, optional directional. Drawn under the rounded window. |
| S5 | Inset | Padding inside the window filled with insetColor at insetAlpha ("Inset balance" distributes it). |
| S6 | Output aspect ratio | Auto (recording + padding) or fixed ratio (content centred, extra background). Output capped at 3840×2160. Vertical outputs fill by default. |
| S7 | Crop | Normalised crop of the recording. |
| S8 | Device frames | iPhone/iPad/Mac mockup frames around the content. |

### Camera
| # | Feature | Behaviour |
|---|---|---|
| K1 | Camera bubble | Inside a safe frame inset 5% of min(W,H). Size = camera-aspect box fitted in the frame × cameraSize. Position = point (0..1) inside the frame. Roundness × min side as radius (0.5 = circle). Same shadow as the screen. Mirror option. |
| K2 | Shape / aspect | original, square, vertical 9:16, horizontal 16:9 (centre-cropped). |
| K3 | Shrink during zoom | Size × 0.7 while a zoom range is active, animated with the screen spring. |
| K4 | Layouts timeline | Ranges of `both` / `only-screen` (bubble hidden) / `only-camera` (camera fullscreen, screen blurred 15 px). Transitions: alpha, blur and scale with spring {530,40,1}. |
| K5 | Auto-hide | Bubble fades (alpha 0, blur 10) when hidden by a layout or voice-over. |

### Timeline editor
| # | Feature | Behaviour |
|---|---|---|
| T1 | Slices model | Ordered slices `{id, sourceStartMs, sourceEndMs, timeScale, volume, systemAudioVolume, externalDeviceAudioVolume, hideCursor, disableSmoothMouseMovement}`. Playback time concatenates slices; gaps are cuts. |
| T2 | Trim | Move the first slice's start or the last slice's end. |
| T3 | Cut / split / delete | Split a slice at the playhead; delete a slice (ripple). |
| T4 | Speed | Per slice speed = 1/timeScale; audio is time-scaled with it. |
| T5 | Zoom track | Add, move and resize zoom ranges; per-range zoom level, mode (auto/manual + target picker), instant toggle, disable. |
| T6 | Layout track | Camera layout ranges. |
| T7 | Undo/redo, timeline zoom, scrubbing with playhead, play/pause, loop. |
| T8 | Typing speed-up suggestions | Suggests speeding up typing sections. |
| T9 | Masks | Highlight/blur regions over time. The `dynamic-mask-tracker` helper does not move masks: it is a content-change detector ("detectMaskedContentShift"). Given the video and each mask's time range and bounds, it compares the masked region every 100 ms (`samplingRate`) against the frame at the mask's start, drawn into a 64×64 bitmap, using a similarity threshold of 0.88 and a histogram threshold of 0.85, and merges changes less than 200 ms apart into `contentChanges` ranges. The editor intersects them with the kept slices and shows "Data may be visible" bars (shield icon, label hidden under 128 px) above the masks; clicking one cuts that fragment (split 100 ms before and after, remove the middle) and selects the mask. |
| T10 | Voice-overs | Record narration over the timeline. Can mute the original. |

### Overlays and audio
| # | Feature | Behaviour |
|---|---|---|
| O1 | Keyboard shortcut overlay | Combos with modifiers (plus special keys; single letters optional). Identical combos within 2 s grouped. Bottom-centred dark pill, glyph keys, text size min(H×5%, W×7%) × ratio, springs in from below and out upward with blur. |
| O2 | Captions | Word-timed transcript (whisper.cpp base/small/medium, word-level, or Apple Speech). Lines fit the width, shown at the bottom. Size ratio. Editable. |
| A1 | Mix | Mic + system + device audio, per-slice volumes, mute toggles, master volume. |
| A2 | Background music | Looped track, volume 0.05 default. |
| A3 | Enhance microphone | Speech denoise. |

### Export
| # | Feature | Behaviour |
|---|---|---|
| E1 | MP4 | H.264 High (avc1.640033) via hardware encoder, CBR. Bitrate = W × H × fps × k, k = 0.3 Studio, 0.05 Social media, 0.0175 Web, 0.0075 Web (Low). AAC audio. |
| E2 | Resolution | MP4 720p / 1080p / 4K (Web qualities ≤ 1080p). Output keeps the project aspect ratio at that height. |
| E3 | Frame rate | MP4 60/50/30/25/24/20/10. GIF 50/30/25/20/15/10. |
| E4 | GIF | Render → palette-generated GIF (Studio) → lossy optimise. Optional no-loop. Heights 480/720/1080. Default 480p 15 fps. |
| E5 | Targets | File, clipboard, shareable link (upload; public or private). |
| E6 | Presets | MP4 720p social 60 (default), 1080p social 60, 4K studio 60, GIF 480p studio 15. Estimated file size shown. |
| E7 | Progress and cancel | Progress UI, cancel, quick-export widget after recording. |
| E8 | Size-targeted export | Not present in 3.7.1: the export sheet only shows an estimated output size. (The `targetSize` string in the main bundle belongs to the bundled fluent-ffmpeg progress parser.) DaisyCap adds an optional "Fit to file size" for MP4. |

### App
| # | Feature | Behaviour |
|---|---|---|
| P1 | Projects library | Default projects folder, auto-save, thumbnails. |
| P2 | Presets | Save and apply style presets (`.screenstudiopreset`). |
| P3 | Editor sidebar | Background & Screen · Cursor · Camera · Captions · Audio · Keyboard Shortcuts · Animations · Device frame. Inspector for the selected zoom or slice. |
| P4 | Menu bar presence | Status-item recorder with source picker. |

## Deliberate differences

- No accounts, licences, uploads or analytics. Shareable links are replaced by "Copy to clipboard" and "Reveal in Finder".
- Original wallpapers, cursor images, device frames, sounds and music are not reused. We ship procedurally generated
  gradients, vector cursors drawn by our renderer, and synthesised click sounds, plus support for the user's own images.
- Native Rust + Metal (wgpu) rendering and VideoToolbox encoding replace Electron, PixiJS and WebCodecs.
