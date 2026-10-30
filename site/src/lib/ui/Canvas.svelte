<script lang="ts" module>
	import type { Point } from './screen.js';

	export type Trail = { points: Point[]; kind: 'raw' | 'smooth'; opacity?: number };
</script>

<script lang="ts">
	import type { Snippet } from 'svelte';
	import { clamp, lerp } from '#lib/ease.js';
	import Cursor from './Cursor.svelte';
	import Dashboard from './Dashboard.svelte';
	import Wallpaper from './Wallpaper.svelte';
	import { SCREEN } from './screen.js';
	import type { WallpaperId } from './wallpaper.js';

	type Props = {
		/** The box the output is fitted into, in pixels. */
		width: number;
		height: number;
		wallpaper: WallpaperId;
		/** Cross-fades from this wallpaper while `fade` goes from 0 to 1. */
		from?: WallpaperId;
		fade?: number;
		resolution?: [number, number];
		/** Background padding in percent, as in the inspector. */
		padding?: number;
		/** Corner radius in pixels of a 1920-wide recording. */
		roundness?: number;
		/** Zoom progress (springs may overshoot 1), the zoom level, and the point it centres. */
		zoom?: number;
		level?: number;
		focus?: Point;
		blur?: number;
		cursor?: Point | null;
		cursorSize?: number;
		/** Cursor scale while pressed, and tilt in degrees. */
		cursorScale?: number;
		cursorTilt?: number;
		circles?: { at: Point; alpha: number; radius: number; blur: number }[];
		trails?: Trail[];
		pressed?: string | null;
		/** Drawn over the output in its own coordinates (output pixels, before fitting). */
		overlay?: Snippet<[{ pad: number; width: number; height: number }]>;
	};

	let {
		width,
		height,
		wallpaper,
		from,
		fade = 1,
		resolution = [960, 640],
		padding = 9,
		roundness = 20,
		zoom = 0,
		level = 1.8,
		focus = { x: SCREEN.w / 2, y: SCREEN.h / 2 },
		blur = 0,
		cursor = null,
		cursorSize = 28,
		cursorScale = 1,
		cursorTilt = 0,
		circles = [],
		trails = [],
		pressed = null,
		overlay
	}: Props = $props();

	// Same geometry as DaisyCap's `Sizes`: padding is a share of the window's average side.
	let pad = $derived(Math.ceil(((SCREEN.w + SCREEN.h) / 2) * (padding / 100)));
	let out = $derived({ w: SCREEN.w + 2 * pad, h: SCREEN.h + 2 * pad });
	let fit = $derived(Math.min(width / out.w, height / out.h));

	let camera = $derived.by(() => {
		const s = 1 + (level - 1) * zoom;
		const settle = (centre: number, f: number, size: number, outSize: number) => {
			const x = centre - s * f;
			return s * size >= outSize ? clamp(x, outSize - s * size, 0) : x;
		};
		const k = clamp(zoom);
		const x = lerp(pad, settle(out.w / 2, focus.x, SCREEN.w, out.w), k);
		const y = lerp(pad, settle(out.h / 2, focus.y, SCREEN.h, out.h), k);
		return `translate(${x}px, ${y}px) scale(${s})`;
	});

	/** Fades the cursor out as it leaves the recorded window. */
	let cursorAlpha = $derived(
		cursor
			? clamp(Math.min(cursor.x + 12, SCREEN.w + 12 - cursor.x, cursor.y + 12, SCREEN.h + 12 - cursor.y) / 12)
			: 0
	);

	const path = (points: Point[]) => points.map((p) => `${p.x.toFixed(1)},${p.y.toFixed(1)}`).join(' ');
</script>

<div class="canvas" style:width="{out.w * fit}px" style:height="{out.h * fit}px">
	<div class="output" style:width="{out.w}px" style:height="{out.h}px" style:transform="scale({fit})">
		{#if from && fade < 1}
			<Wallpaper id={from} width={resolution[0]} height={resolution[1]} />
		{/if}
		<div class="layer" style:opacity={from ? fade : 1}>
			<Wallpaper id={wallpaper} width={resolution[0]} height={resolution[1]} />
		</div>
		<div class="camera" style:transform={camera} style:filter={blur > 0.3 ? `blur(${blur}px)` : null}>
			<div class="window" style:border-radius="{roundness / 2}px">
				<Dashboard {pressed} />
			</div>
			{#if trails.length}
				<svg class="trails" viewBox="0 0 {SCREEN.w} {SCREEN.h}" aria-hidden="true">
					{#each trails as trail, i (i)}
						<polyline class={trail.kind} points={path(trail.points)} opacity={trail.opacity ?? 1} />
					{/each}
				</svg>
			{/if}
			{#each circles as circle, i (i)}
				<span
					class="circle"
					style:left="{circle.at.x - circle.radius}px"
					style:top="{circle.at.y - circle.radius}px"
					style:width="{circle.radius * 2}px"
					style:height="{circle.radius * 2}px"
					style:opacity={circle.alpha}
					style:filter={circle.blur > 0.3 ? `blur(${circle.blur}px)` : null}
				></span>
			{/each}
			{#if cursor && cursorAlpha > 0}
				<div
					class="pointer"
					style:left="{cursor.x}px"
					style:top="{cursor.y}px"
					style:opacity={cursorAlpha}
					style:transform="rotate({cursorTilt}deg) scale({cursorScale})"
				>
					<Cursor size={cursorSize} />
				</div>
			{/if}
		</div>
		{@render overlay?.({ pad, width: out.w, height: out.h })}
	</div>
</div>

<style>
	.canvas {
		position: relative;
		flex-shrink: 0;
		overflow: hidden;
	}

	.output {
		position: absolute;
		top: 0;
		left: 0;
		overflow: hidden;
		transform-origin: 0 0;
	}

	.layer {
		position: absolute;
		inset: 0;
	}

	.camera {
		position: absolute;
		top: 0;
		left: 0;
		width: 960px;
		height: 600px;
		transform-origin: 0 0;
	}

	.window {
		position: absolute;
		inset: 0;
		overflow: hidden;
		box-shadow: 0 12px 22px rgba(0, 0, 0, 0.42);
	}

	.trails {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		overflow: visible;
	}

	polyline {
		fill: none;
		stroke-linecap: round;
		stroke-linejoin: round;
	}

	.raw {
		stroke: #8e8e93;
		stroke-width: 2.5;
		stroke-dasharray: 5 6;
	}

	.smooth {
		stroke: #0a84ff;
		stroke-width: 4;
	}

	.circle {
		position: absolute;
		border-radius: 50%;
		background: #dedede;
	}

	.pointer {
		position: absolute;
		transform-origin: 0 0;
	}
</style>
