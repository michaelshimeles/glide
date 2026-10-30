# DaisyCap download page

The download page for DaisyCap, an open-source screen recorder for the Mac. One prerendered SvelteKit page plus a `/download` redirect, deployed on Vercel. Run the commands below from this folder.

## Develop

You need Node 22.17 or later on the 22 line, or Node 24 or later. SvelteKit requires 22.17, the Svelte Vite plugin skips Node 23, and `.npmrc` turns engine mismatches into install errors.

```sh
npm install
npm run dev       # http://localhost:5173
npm run check     # svelte-check
npm run build     # writes .vercel/output
npm run preview   # serves the build on http://localhost:4173
```

## Download link

The Download button points at `/download`, which 302s to `https://github.com/michaelshimeles/daisycap/releases/latest/download/DaisyCap.dmg`, the DMG that `scripts/release-macos.sh` builds. The URL, version number and system requirements all live in `src/lib/site.ts`.

## Assets

The icons in `src/lib/assets` and `static`, and `static/og.jpg`, come from `scripts/assets.sh`. The script reads the app from the repo root, so run `scripts/build.sh` there first, or point `GIMBAL_REPO` at a checkout that already has a built `dist/DaisyCap.app`. It never writes to the app's files.

```sh
scripts/assets.sh
GIMBAL_REPO=~/src/daisycap scripts/assets.sh
```

The Open Graph card is `scripts/og/og.html` rendered by headless Chrome, around a frame of DaisyCap's own synthetic recording (`daisycap synth`) styled by `scripts/og/style.json`. You also need `cwebp` and ImageMagick (`brew install webp imagemagick`).

## Demo and cards

The demo and the feature cards are HTML and CSS recreations of DaisyCap's interface in `src/lib/demo`, `src/lib/cards` and `src/lib/ui`, so the page ships no video and shows no real desktop. The demo's timing, click targets, captions and sound cues live in `src/lib/demo/script.ts`. `src/lib/ui/recording.ts` replays the recording with the app's spring constants for zooms and cursor smoothing, so the motion matches what DaisyCap renders.

## Deploy

Import the repo as a Vercel project with the SvelteKit preset and set the Root Directory to `site`. No environment variables are needed. The canonical, Open Graph and JSON-LD URLs are built from `origin` in `src/lib/site.ts`, so every build, including previews and local builds, points them at `https://daisycap.rasmic.xyz`. `static/robots.txt` and `static/sitemap.xml` spell out the same domain, so update them too if it changes.

## Analytics

`src/routes/+layout.ts` loads Vercel Web Analytics, which sets no cookies and stores no personal data. Turn it on in the Vercel project's Analytics tab. Until then, the script returns 404 and nothing gets counted. On Vercel, the build gets a project-specific script path in `VITE_VERCEL_OBSERVABILITY_CLIENT_CONFIG`. Builds without it, like `npm run preview`, request `/_vercel/insights/script.js`. Under `npm run dev` the page loads Vercel's debug script, which logs events to the console and sends nothing.

The layout calls `inject()` from `@vercel/analytics` because the package's SvelteKit helper subscribes to `$app/stores`, which throws in SvelteKit 3. The package also limits its `@sveltejs/kit` peer to `^1 || ^2`, so `package.json` overrides that peer with the site's own Kit. Drop both workarounds once the package supports Kit 3.
