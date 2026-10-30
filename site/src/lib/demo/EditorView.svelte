<script lang="ts">
	import Canvas from '#lib/ui/Canvas.svelte';
	import ExportSheet from '#lib/ui/ExportSheet.svelte';
	import Timeline from '#lib/ui/Timeline.svelte';
	import TrafficLights from '#lib/ui/TrafficLights.svelte';
	import type { WallpaperId } from '#lib/ui/wallpaper.js';
	import Inspector, { type Smoothing, type Tab } from './Inspector.svelte';
	import { EDITOR, REC, pressedButton } from './script.js';

	type Props = {
		r: number;
		playing: boolean;
		tab: Tab;
		smoothing: Smoothing;
		wallpaper: { id: WallpaperId; from?: WallpaperId; fade: number };
		padding: number;
		roundness: number;
		/** How far each automatic zoom has popped onto the timeline. */
		zooms: [number, number];
		pressed: string | null;
		sheet: { shown: number; progress: number | null; done: boolean };
	};

	/** DaisyCap's editor (`editor.rs`) playing back the recording from the first chapter. */
	let { r, playing, tab, smoothing, wallpaper, padding, roundness, zooms, pressed, sheet }: Props = $props();

	let raw = $derived(smoothing === 'Off');
	let camera = $derived(REC.camera(r));
</script>

<div class="editor">
	<div class="toolbar">
		<TrafficLights />
		<span class="button">‹ Back</span>
		<span class="button title">Product tour <svg viewBox="0 0 8 5"><path d="M.8.8 4 4 7.2.8" /></svg></span>
		<span class="spacer"></span>
		<span class="button record"><i></i>Record</span>
		<span class="button">Window <svg viewBox="0 0 8 5"><path d="M.8.8 4 4 7.2.8" /></svg></span>
		<span class="icons">
			<svg viewBox="0 0 16 16"><path d="M6 4.5 3 7.5l3 3M3.5 7.5h6a3 3 0 0 1 0 6H8" /></svg>
			<svg viewBox="0 0 16 16"><path d="M10 4.5l3 3-3 3M12.5 7.5h-6a3 3 0 0 0 0 6H8" /></svg>
		</span>
		<span class="button">Presets</span>
		<span class={['primary', { pressed: pressed === 'export' }]} data-target="export">Export</span>
	</div>

	<div
		class="preview"
		style:left="{EDITOR.preview.x}px"
		style:top="{EDITOR.preview.y}px"
		style:width="{EDITOR.preview.w}px"
		style:height="{EDITOR.preview.h}px"
	>
		<Canvas
			width={EDITOR.canvas.w}
			height={EDITOR.canvas.h}
			wallpaper={wallpaper.id}
			from={wallpaper.from}
			fade={wallpaper.fade}
			{padding}
			{roundness}
			zoom={camera.zoom}
			level={camera.level}
			focus={camera.focus}
			blur={camera.blur}
			cursor={raw ? REC.raw(r) : REC.smooth(r)}
			cursorSize={30}
			cursorScale={REC.pressing(r) ? 0.8 : 1}
			cursorTilt={raw ? 0 : REC.tilt(r)}
			circles={REC.circles(r)}
			pressed={pressedButton(r)}
		/>
	</div>

	<Inspector {tab} {smoothing} wallpaper={wallpaper.id} {padding} {roundness} {pressed} />

	<div class="timeline">
		<Timeline
			duration={REC.duration}
			width={EDITOR.timeline.w}
			playhead={r}
			step={500}
			{playing}
			zooms={REC.zooms.map((z, i) => ({ start: z.start, end: z.end, label: '1.8× Auto', shown: zooms[i] }))}
		/>
	</div>

	{#if sheet.shown > 0}
		<div
			class="sheet"
			style:left="{EDITOR.sheet.x}px"
			style:top="{EDITOR.sheet.y}px"
			style:opacity={sheet.shown}
			style:transform="scale({0.96 + 0.04 * sheet.shown})"
		>
			<ExportSheet
				progress={sheet.progress}
				done={sheet.done}
				pressed={pressed === 'go'}
				seconds={REC.duration / 1000}
				estimate={2}
				size={1.7}
				speed={3.9}
			/>
		</div>
	{/if}
</div>

<style>
	.editor {
		position: absolute;
		inset: 0;
		background: var(--window);
		color: var(--text);
		font-size: 13px;
	}

	.toolbar {
		position: absolute;
		inset: 0 0 auto;
		display: flex;
		align-items: center;
		gap: 6px;
		height: 40px;
		padding: 0 12px 0 88px;
		border-bottom: 1px solid var(--separator);
	}

	.button,
	.primary {
		display: flex;
		align-items: center;
		gap: 6px;
		height: 24px;
		padding: 0 9px;
		border-radius: 6px;
		background: var(--control);
		box-shadow: inset 0 0 0 0.5px var(--control-edge);
		white-space: nowrap;
	}

	.title {
		font-weight: 600;
	}

	svg {
		width: 8px;
		height: 5px;
		fill: none;
		stroke: var(--text);
		stroke-width: 1.4;
	}

	.spacer {
		flex: 1;
	}

	.record {
		color: var(--record);
		font-weight: 600;
	}

	.record i {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		background: var(--record);
	}

	.icons {
		display: flex;
		gap: 4px;
		margin: 0 4px 0 6px;
	}

	.icons svg {
		width: 26px;
		height: 16px;
		stroke: var(--tertiary);
		stroke-width: 1.5;
		stroke-linecap: round;
		stroke-linejoin: round;
	}

	.primary {
		margin-left: 2px;
		background: var(--accent);
		box-shadow: none;
		color: #fff;
		font-weight: 600;
		transition: filter 0.1s;
	}

	.primary.pressed {
		filter: brightness(0.82);
	}

	.preview {
		position: absolute;
		display: grid;
		place-items: center;
		background: var(--stage);
	}

	.timeline {
		position: absolute;
		left: 0;
		bottom: 0;
		width: 1000px;
		height: 236px;
		padding: 8px 12px;
		border-top: 1px solid var(--separator);
		background: var(--sidebar);
	}

	.sheet {
		position: absolute;
		transform-origin: 50% 0;
	}
</style>
