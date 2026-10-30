# Agent workflow

This repo holds DaisyCap, a native macOS screen recorder written in Rust, and its download page in `site/`. Pick the parts of this workflow that fit the task. The workflow and its skills come from [michaelshimeles/skills](https://github.com/michaelshimeles/skills).

- Read-only reviews, explanations and investigations need no worktree or commit. If an investigation turns into edits, isolate those edits.
- Code and copy changes get isolation, the checks below, and evidence of what changed. For a bug, capture the failure before fixing it.
- Small documentation edits get an isolated branch and a careful read. Don't manufacture runtime evidence for them.
- Ship only when the task asks for a finished change. A plan, review or local experiment doesn't imply committing or publishing.

## Workflow

1. **Isolate.** Use `/new-feature` before editing, and branch from `origin/main`. Micky's main checkout at `/Users/Shared/projects/screenstudio-rs` is synced to four other machines with Mutagen, so don't add worktrees, `node_modules` or build output there. Work in a fresh clone at `~/Desktop/daisycap-worktrees/<task-name>` on a branch named `agent/<task-name>`, and run `npm install` in its `site/` before site checks.
2. **Build.** Follow the layout in the README. The app lives in `src/`, with the frame model in `src/engine`, and the download page lives in `site/`. Reach for `/code-structure` when the same operational logic shows up in more than one flow.
3. **Prove.** Run the checks below for the part you changed. Use `/evidence-driven-testing` when behavior changes. For visible site changes, check desktop width and 390 px in light and dark, and keep the screenshots.
4. **Ship.** Open a PR against `main`. Add a before and after table from `/before-and-after` for visible changes, and run `/greploop` for code changes. Aim for 5/5 with no unresolved actionable findings on the current commit. Stop at the iteration cap, a review timeout or an unavailable integration, and report the real score and what's left.

## Project rules

- Commit messages follow the repo's style, `type(scope): lowercase summary`, such as `feat(app): ...`, `fix(audio): ...` or `docs: ...`.
- Keep build output out of the repo. `scripts/env.sh` puts Cargo's target directory in `~/.cache/cargo-target/screenstudio-rs`, and test output goes to `~/Movies/daisycap-tests`.
- Screenshots and demos come from DaisyCap's synthetic recordings (`daisycap synth`), never from a real desktop.
- Don't deploy to Vercel, create Vercel projects, publish GitHub releases or push tags unless Micky asks.
- The download page claims only what the app ships. Check a feature in `src/` before the page mentions it.
- Keep Screen Studio's name out of the page's headline, logo and assets. It appears once, in the footer's not-affiliated note.
- The page's only analytics is Vercel Web Analytics, which sets no cookies and stores no personal data. Don't add other analytics, tracking or third-party scripts.

## Writing for humans

Run `/unslop` over anything a person will read before you commit, post or send it. That covers commit messages, PR titles and bodies, README edits, code comments, page copy and your closing reply. Apply it to text you wrote or changed, not to prose you didn't touch.

## Multi-agent rules

- Never commit directly to `main`.
- One clone or worktree and one branch per task and per agent. Never reuse or change another agent's checkout, branch or uncommitted work.
- Before starting, skim the open PRs with `gh pr list` and `gh pr diff <n> --name-only`, and look for uncommitted work in shared checkouts without changing it. Independent edits to the same file can go ahead in your own checkout. Ask only when changes conflict in behavior, depend on unfinished work, or ownership is unclear.
- Never use plain `--force`. Use `--force-with-lease`, and only on your own task branch.
- Resolve `Cargo.lock` and `site/package-lock.json` conflicts by regenerating them, never by hand-merging.
- Every checkout and repo on this Mac shares the same ports, `/tmp` and Cargo target directory. Confirm a preview port answers your process before trusting what it serves, and put scratch files in `mktemp -d` directories instead of fixed `/tmp` paths.
- If you can't resolve a conflict with confidence, stop and report it.

## Finishing a task

1. Keep changes to the assigned task.
2. Run the checks below.
3. Collect the evidence: screenshots for visible changes, command output for the rest.
4. Commit with a clear message, rebase onto the latest `origin/main`, and rerun the checks.
5. Push with `git push -u origin <branch>` and open a PR. The body says what changed, how you tested it with evidence for each claim, before and after proof, and any risks or follow-up.
6. Run `/greploop` on the PR when it applies.

Don't merge unless Micky asks. Keep the clone until the PR is merged or closed.

## Checks

For the app, from the repo root:

- `. scripts/env.sh && cargo test` runs the unit and integration tests. `env.sh` sets `SDKROOT` for the whisper.cpp build and keeps Cargo output outside the repo.
- `scripts/build.sh` makes the release app in `dist/`. `scripts/selftest.sh` runs the end-to-end checks. It plays a quiet test tone, and `--screen` records the display.

For the download page, from `site/`:

- `npm run check` runs svelte-check and should report 0 errors and 0 warnings.
- `npm run build` must pass. It writes `.vercel/output`, with the page prerendered and `/download` as a function.
- Run the Svelte MCP `svelte-autofixer` on every `.svelte` file you change until it reports no issues.
- After changing the page, run the preview and check it at desktop width and 390 px, in light and dark. `curl -sI http://localhost:<port>/download` should return a 302 to the DMG URL in `site/src/lib/site.ts`.
- Other site repos on this Mac also preview on Vite's default port 4173, often from agents running at the same time, so `localhost:4173` may serve a different site. Pick a port with nothing listening on it, run `npm run preview -- --port <port> --strictPort`, and confirm with `lsof -nP -iTCP:<port> -sTCP:LISTEN` that the listener is your process.
- Restart `npm run preview` after every build. It indexes files at startup, so a new build's assets 404, and a request for a deleted asset from the old build crashes it.
- If you change `site/scripts/assets.sh` or anything in `site/scripts/og/`, rerun the script and check the sizes it prints. It needs a built app, so run `scripts/build.sh` first.
- The page's demo and feature cards are HTML. If the app's interface or behavior changes, update `site/src/lib/ui` and `site/src/lib/demo` to match, and remeasure the click targets in `site/src/lib/demo/script.ts`.

For every change, `git diff --check` catches whitespace errors. Keep generated evidence in the ignored `.artifacts/` folder.

## Skills

`npx skills add michaelshimeles/skills` installed the skills into `.agents/skills`, which Cursor and Codex read. `.claude/skills` holds symlinks to them for Claude Code, and `skills-lock.json` records what was installed. Update them with `npx skills update -p`, then review the diff before committing, since skills run with full agent permissions.

`/before-and-after` uploads to 0x0.st by default, which is public. That's fine for screenshots of the download page. Pass `--upload-url` for anything else.
