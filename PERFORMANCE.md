# Performance

All numbers are real measurements on this MacBook Pro (Apple M4 Pro, 24 GB,
macOS 27.0.1), release build, from `scripts/bench.sh` on 2026-10-11. Two runs
are shown where they differ: 12:15 (first build with the editor) and 13:11
(after masks, device frames and GIF delta frames). During the 13:11 run a
browser video was playing on the recorded display, so the "real display"
recording had far more motion than at 12:15. Re-run `scripts/bench.sh`
after performance work and update this file.

## Recording overhead

`daisycap record -t 10`, measured with `/usr/bin/time -l` over the whole recorder
process (ScreenCaptureKit capture, hardware HEVC encode, 120 Hz mouse/cursor
logging, package write). CPU is user + system time divided by wall time.

| Recording | Run | Wall | CPU time | CPU load | Peak memory |
|---|---|---|---|---|---|
| 5K display (5120×2880 @ 60 fps, HEVC), 10 s | 12:15 | 10.7 s | 1.07 s | **10 % of one core** | **56 MB** |
| 5K display, 10 s, video playing on screen | 13:11 | 10.9 s | 0.96 s | 8.8 % of one core | 58 MB |
| Generated test pattern (1080p @ 60 fps), 10 s | 13:11 | 11.4 s | 1.52 s | 13 % of one core (pattern drawn on the CPU) | 91 MB |

A 10 s 5K60 recording of a mostly static screen is 16 MB on disk
(~13 Mbit/s HEVC). The encoder runs on the media engine, so recording leaves
the CPU and GPU almost idle for the app being recorded. The full GUI flow
(launch, area picker, record 3 s, pause/resume, stop, open editor, export,
quit) peaks at 172 MB resident.

## Export speed

"× real time" = seconds of output video per second of wall-clock export.
Export renders every frame on the GPU (wgpu/Metal) directly into the
encoder's pixel buffers (zero-copy via `CVMetalTextureCache`), with decode,
render and encode pipelined.

| Source | Output | 12:15 | 13:11 |
|---|---|---|---|
| Synthetic 2560×1600 HEVC, 12 s | 1594×1080 @ 60, H.264 "social" | 2.84 s, **4.2×** | 3.08 s, 3.9× |
| Synthetic 2560×1600 HEVC, 12 s | 1594×1080 @ 60, HEVC | 2.84 s, 4.2× | 3.77 s, 3.2× |
| Synthetic 2560×1600 HEVC, 12 s | 1062×720 @ 30, H.264 "web" | 1.45 s, 8.3× | 1.38 s, **8.7×** |
| Synthetic 2560×1600 HEVC, 12 s | 3188×2160 @ 60, H.264 "studio" | 9.89 s, 1.2× | 10.1–13.8 s, 0.9–1.2× |
| Synthetic 2560×1600 HEVC, 12 s | 1062×720 GIF @ 15 | 1.60 s, 7.5×, 7.4 MB | 1.50 s, **8.0×, 4.0 MB** |
| Real 5K display recording, 10.1 s | 1920×1080 @ 60, H.264 | 4.05 s, 2.5× | 3.45 s, **2.9×** |
| Real 5K display recording, 10.1 s | 3840×2160 @ 60, H.264 "studio" | 10.07 s, 1.0× | 11.31 s, 0.9× |
| Real 5K display recording, 10.1 s | 1280×720 GIF @ 15 | 3.34 s, 3.0×, 25.7 MB | 2.91 s, 3.5×, 12.1 MB (video on screen) |

GIF delta frames (only changed 32 px tiles after the first frame) measured
on the same 12:15 static recording: 25.7 MB → 4.7 MB; synthetic 7.4 → 4.0 MB,
at the same encode speed.

### Where the time goes (`GIMBAL_PROFILE=1`)

| Export | Decode + upload | Render + submit | Encoder back-pressure |
|---|---|---|---|
| 5K recording → 1080p60 | 2.41 s | 1.13 s | 0.11 s |
| Synthetic → 4K60 (two runs) | 0.03 s | 1.99–2.07 s | 7.36–9.10 s |

- **1080p from a 5K source is decode-bound**: decoding 605 frames of
  5120×2880 HEVC is the largest stage. Rendering a 1080p frame (background,
  shadow, rounded window, motion blur, cursor, effects) takes under 2 ms.
- **4K is encoder-bound**: the media engine encodes ~80–100 frames/s at
  3188×2160 (≈550–690 Mpixel/s) and that rate varies with other users of
  the engine (here, the browser's video decode); rendering runs at
  ~350 frames/s. Asking the encoder to prioritise speed over quality
  (`PrioritizeEncodingSpeedOverQuality`) made no difference.
- The GPU renderer has ample headroom for live preview: ~535 frames/s at
  1080p and ~350 frames/s at 4K in the export pipeline, versus 60 needed.

## Editor preview (real-time playback)

`daisycap bench-preview` runs the editor's actual preview path — decode the
source frame, wrap it as a GPU texture without copying, render the full
scene into a 2400×1500 px target (the preview area of a full-screen editor
on a Retina display) — over 240 consecutive 60 fps frames.

| Project | Preview frames/s | vs. 60 fps playback |
|---|---|---|
| Real 5K display recording (HEVC 5120×2880) | 189 | 3.1× headroom |
| Real 5K recording with a blur + highlight mask, 1600×1000 | 165 | 2.7× |
| Synthetic 2560×1600 recording | 600 | 10× |

Picking a wallpaper regenerates it (1920×1200, on the CPU, rows in
parallel): 8–10 ms, so swatch clicks and slider drags update the canvas
on the next frame. (Single-threaded with the glass ribbons it was 95 ms.)

## Editor audio

Playback mixes the edit's soundtrack in the background and streams it to
CoreAudio from a real-time callback; the playhead is the clock.

| | Measured |
|---|---|
| Re-mix after a mute / clip-volume / music edit, 74 s recording | 180 ms first, 40 ms after (decoded tracks cached by the OS) |
| Master volume change heard | next output buffer (~10 ms) |
| Audio vs playhead drift while playing | ≤ 45 ms (re-seeked past 60 ms) |
| After a seek | within 15 ms |

## Mask checks ("data may be visible")

`daisycap check-masks` runs the same background check the editor uses: every
blur/pixelate mask is compared with its first frame every 100 ms, all masks
sharing one forward decoding pass.

| Project | Masks | Time |
|---|---|---|
| Real 5K display recording, 3 s | 4 (each the full 3 s) | 1.22 s (4.60 s when each mask decoded its own range) |
| Synthetic 2560×1600, 12 s | 2 over 3.8 s | 0.59 s |

## Captions (Whisper)

`daisycap transcribe --engine whisper-base` on 7.0 s of synthesized speech
(`say`): 0.4 s including loading the model (Metal), so about 17× real
time; all 20 words correct. The first run also downloads the model
(148 MB, ~8 s here), and the first run of a newly built binary takes
about 6 s while Metal compiles whisper.cpp's shaders.

## Live source preview

Recording setup shows the selected source live (ScreenCaptureKit, 10 fps,
1280 px long edge, copied into an egui texture): the release build used
6.1 % of one core over 10 s with it on screen, and 0 % with the window
hidden (the stream stops). It starts in about 0.2 s (`daisycap preview`).

## Idle

With a project open in the editor and nothing playing, the app used 0.00 s
of CPU over 10 s (it only wakes for UI events, timers and background work)
at 107 MB resident.

## How to reproduce

```sh
scripts/bench.sh              # needs Screen Recording permission for the terminal
scripts/bench.sh --no-screen  # synthetic sources only
GIMBAL_PROFILE=1 dist/daisycap export Project.daisycap --height 2160
dist/daisycap bench-preview Project.daisycap
```
