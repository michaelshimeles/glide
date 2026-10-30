<script lang="ts">
	import { easeOut, easeOutBack, envelope, span } from '#lib/ease.js';
	import Cursor from '#lib/ui/Cursor.svelte';
	import Dashboard from '#lib/ui/Dashboard.svelte';
	import RecordBar from '#lib/ui/RecordBar.svelte';
	import EditorView from './EditorView.svelte';
	import StartView from './StartView.svelte';
	import {
		APP,
		BAR,
		CAPTIONS,
		DESK,
		T,
		padding,
		playhead,
		playing,
		pointer,
		pressed,
		pressedButton,
		roundness,
		wallpaper
	} from './script.js';

	let { t }: { t: number } = $props();

	let start = $derived(1 - easeOut(span(t, T.hide, T.hide + 260)));
	let editor = $derived(easeOut(span(t, T.editor, T.editor + 380)));
	let recording = $derived(t >= T.rec && t < T.stop);
	let bar = $derived(envelope(t, T.bar, T.bar + 260, T.stop + 60, T.stop + 260));
	let countdown = $derived(t < T.count[1] ? 3 : t < T.count[2] ? 2 : t < T.rec ? 1 : 0);
	let elapsed = $derived(Math.max(0, Math.min(t, T.stop) - T.rec));
	let held = $derived(pressed(t));
	let at = $derived(pointer(t));

	let caption = $derived(CAPTIONS.find((c) => t >= c.start - 200 && t < c.end + 900) ?? null);
	let captionShown = $derived(caption ? envelope(t, caption.start - 200, caption.start, caption.end + 600, caption.end + 900) : 0);
</script>

<div class="scene">
	<div class="wallpaper"></div>

	<div class="desk" style:left="{DESK.x}px" style:top="{DESK.y}px" style:transform="scale({DESK.scale})">
		<Dashboard pressed={recording ? pressedButton(t - T.rec) : null} />
	</div>

	{#if start > 0}
		<div
			class="app"
			style:left="{APP.x}px"
			style:top="{APP.y}px"
			style:opacity={start}
			style:transform="scale({APP.scale * (0.97 + 0.03 * start)})"
		>
			<StartView window={easeOut(span(t, T.source, T.source + 220))} pressed={held === 'window' || held === 'record' ? held : null} {t} />
		</div>
	{/if}

	{#if editor > 0}
		<div
			class="app"
			style:left="{APP.x}px"
			style:top="{APP.y}px"
			style:opacity={editor}
			style:transform="scale({APP.scale * (0.97 + 0.03 * editor)})"
		>
			<EditorView
				r={playhead(t)}
				playing={playing(t)}
				tab={t >= T.cursor && t < T.background ? 'cursor' : 'background'}
				smoothing={t >= T.off && t < T.smooth ? 'Off' : 'Smooth'}
				wallpaper={wallpaper(t)}
				padding={padding(t)}
				roundness={roundness(t)}
				zooms={[easeOutBack(span(t, T.pops[0], T.pops[0] + 320)), easeOutBack(span(t, T.pops[1], T.pops[1] + 320))]}
				pressed={held}
				sheet={{
					shown: easeOut(span(t, T.sheet, T.sheet + 240)),
					progress: t >= T.progress[0] ? span(t, T.progress[0], T.progress[1]) : null,
					done: t >= T.done
				}}
			/>
		</div>
	{/if}

	{#if bar > 0}
		<div
			class="bar"
			style:left="{BAR.x}px"
			style:top="{BAR.y}px"
			style:opacity={bar}
			style:transform="translateY({(1 - bar) * 10}px) scale({BAR.scale})"
		>
			<RecordBar {countdown} {elapsed} pressed={held === 'stop'} />
		</div>
	{/if}

	<div class="menubar">
		<svg class="apple" viewBox="0 0 17 21" aria-hidden="true">
			<path
				d="M11.2 4.3c.6-.8 1-1.8.9-2.8-.9 0-2 .6-2.6 1.4-.6.7-1.1 1.7-.9 2.7 1 .1 2-.5 2.6-1.3zM12.1 5.8c-1.4-.1-2.7.8-3.3.8-.7 0-1.7-.8-2.9-.8C4.4 5.9 3 6.8 2.2 8.1c-1.6 2.7-.4 6.8 1.1 9 .8 1.1 1.7 2.3 2.8 2.3 1.1 0 1.6-.7 2.9-.7 1.4 0 1.8.7 2.9.7 1.2 0 2-1.1 2.7-2.2.9-1.3 1.2-2.5 1.2-2.6 0 0-2.4-.9-2.4-3.6 0-2.2 1.8-3.3 1.9-3.4-1-1.5-2.6-1.7-3.2-1.7z"
			/>
		</svg>
		<b>DaisyCap</b>
		<span>File</span><span>Edit</span><span>View</span><span>Window</span><span>Help</span>
		<span class="spacer"></span>
		<span class={['status', { recording }]}>
			<svg viewBox="0 0 16 16" aria-hidden="true">
				<circle cx="8" cy="8" r="6.4" class="ring" />
				<circle cx="8" cy="8" r={recording ? 6.4 : 3.6} class="core" />
			</svg>
			{#if recording}00:{String(Math.floor(elapsed / 1000)).padStart(2, '0')}{/if}
		</span>
		<span>Tue Oct 20&nbsp;&nbsp;9:41 AM</span>
	</div>

	{#if caption && captionShown > 0}
		<div class="caption" style:opacity={captionShown} style:transform="translateY({(1 - captionShown) * 6}px)">
			{#each caption.words as w, i (i)}
				<span style:opacity={0.32 + 0.68 * span(t, w.at - 60, w.at + 140)}>{w.text}</span>
			{/each}
		</div>
	{/if}

	<div class="pointer" style:left="{at.x}px" style:top="{at.y}px">
		<Cursor size={22} />
	</div>
</div>

<style>
	.scene {
		position: relative;
		width: 1200px;
		height: 720px;
		overflow: hidden;
		font-family: var(--sans);
		user-select: none;
	}

	.wallpaper {
		position: absolute;
		inset: 0;
		background-color: #0b0d18;
		background-image:
			radial-gradient(circle farthest-corner at 50% 50%, rgba(64, 110, 255, 0.42), transparent 62%),
			radial-gradient(circle farthest-corner at 4% 112%, rgba(40, 200, 200, 0.26), transparent 60%),
			radial-gradient(circle farthest-corner at 104% 96%, rgba(130, 90, 255, 0.3), transparent 62%),
			radial-gradient(circle farthest-corner at 18% 8%, rgba(60, 90, 220, 0.3), transparent 64%),
			linear-gradient(158deg, #121830 0%, #0a0d1a 46%, #0e1222 100%);
	}

	.desk {
		position: absolute;
		width: 960px;
		height: 600px;
		overflow: hidden;
		border-radius: 12px;
		box-shadow:
			0 0 0 0.5px rgba(255, 255, 255, 0.18),
			0 30px 80px -10px rgba(0, 0, 0, 0.7);
		transform-origin: 0 0;
	}

	.app {
		position: absolute;
		width: 1320px;
		height: 792px;
		overflow: hidden;
		border-radius: 14px;
		box-shadow:
			0 0 0 0.5px rgba(255, 255, 255, 0.2),
			0 40px 100px -10px rgba(0, 0, 0, 0.75);
		transform-origin: 50% 50%;
		translate: calc(-1320px * (1 - 1040 / 1320) / 2) calc(-792px * (1 - 1040 / 1320) / 2);
	}

	.bar {
		position: absolute;
		transform-origin: 0 0;
	}

	.menubar {
		position: absolute;
		inset: 0 0 auto;
		display: flex;
		align-items: center;
		gap: 18px;
		height: 28px;
		padding: 0 18px;
		background: rgba(10, 10, 14, 0.32);
		backdrop-filter: blur(20px) saturate(180%);
		color: #f4f4f5;
		font-size: 13px;
	}

	.apple {
		width: 13px;
		height: 16px;
		fill: #f4f4f5;
	}

	.spacer {
		flex: 1;
	}

	.status {
		display: flex;
		align-items: center;
		gap: 5px;
		font-variant-numeric: tabular-nums;
	}

	.status svg {
		width: 15px;
		height: 15px;
	}

	.status .ring {
		fill: none;
		stroke: #f4f4f5;
		stroke-width: 1.3;
	}

	.status .core {
		fill: #f4f4f5;
	}

	.status.recording .ring {
		stroke: #ff453a;
	}

	.status.recording .core {
		fill: #ff453a;
	}

	.caption {
		position: absolute;
		left: 50%;
		bottom: 30px;
		display: flex;
		flex-wrap: wrap;
		justify-content: center;
		gap: 0 0.28em;
		max-width: 760px;
		padding: 9px 18px;
		border-radius: 14px;
		background: rgba(8, 8, 10, 0.62);
		backdrop-filter: blur(16px);
		color: #fff;
		font-size: 21px;
		font-weight: 600;
		letter-spacing: -0.01em;
		line-height: 28px;
		translate: -50% 0;
		white-space: nowrap;
	}

	.pointer {
		position: absolute;
		z-index: 2;
	}
</style>
