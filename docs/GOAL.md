# Goal: open-source, fast Rust screen recorder with Screen Studio feature parity

Working name: `daisycap-rs` (you may pick a better original name; never use the trademark "Screen Studio" as the product name).

Target bundle for analysis (outside the repo, never commit anything from it): `~/re-targets/screen-studio/Screen Studio.app` (Electron app: `Contents/Resources/app.asar` + `app.asar.unpacked`).

## Build machine
The build machine is Micky's MacBook Pro (this machine). Use `CARGO_TARGET_DIR=~/.cache/cargo-target/screenstudio-rs`.
A rec-rs soak test may still be running until about 10:47 ET on 2026-10-11. Before your first big cargo build, check that `pgrep -f soak` returns nothing; if it does, wait. Never touch the Rec app, the herdr session `rec`, or /Users/Shared/projects/rec-rs (cloning rec-rs from GitHub elsewhere is fine).

Hard rule: never commit any of Screen Studio's code, assets, sounds, wallpapers, cursors or fonts. Behaviour notes only; all code written independently.

## 1. Reverse engineer with REA
Use the REA MCP tools to analyse Screen Studio thoroughly: architecture, recording pipeline, data/project format, every feature. Write `analysis/SCREEN_STUDIO.md` with a complete feature inventory as a parity checklist, including:
- screen, window and area recording; system audio, mic and camera
- automatic zoom following clicks and cursor activity, with smooth spring-based easing
- cursor smoothing, resizing, hiding when idle, click highlights
- motion blur
- backgrounds, wallpapers, gradients, padding, rounded corners, shadows, insets
- camera overlay: shape, position, auto-hide
- timeline editor: zoom segments, trim, cut, speed, layouts
- captions/transcripts if present; keyboard shortcut overlays; iOS device recording if present
- export presets: MP4, GIF, resolutions, frame rates, quality, size-targeted export; share links if present

## 2. Build
A native Rust macOS app with feature parity: GPU-accelerated (Metal via wgpu or similar), real-time preview, hardware encoding (VideoToolbox), polished UI in the same native style as Micky's paper-rs and Rec apps (egui with Inter).
Prior art: Micky's own rec-rs (https://github.com/michaelshimeles/rec-rs, MIT/Apache, same author) has a fast ScreenCaptureKit + VideoToolbox + audio pipeline in Rust. Clone it and reuse freely, with credit.
Keep CARGO_TARGET_DIR outside the repo (~/.cache/cargo-target/screenstudio-rs).

## 3. Process
Track milestones in `PROGRESS.md`, add tests, keep `PERFORMANCE.md` with real measurements (recording overhead, export speed vs real time). Commit as you go using Conventional Commits.

## 4. Ship
Bundle as a .app with a nice icon; verify on this MacBook with real recordings and exports. If permissions (Screen Recording, Mic, Camera) block real-capture tests, write the exact steps Micky must take to grant them in PROGRESS.md and keep testing with synthetic sources.

## 5. Don't push
Do not push to GitHub; Micky handles publishing.
