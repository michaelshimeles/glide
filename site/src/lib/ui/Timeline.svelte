<script lang="ts" module>
	export type Segment = { start: number; end: number; label: string; selected?: boolean; shown?: number };

	/** DaisyCap's `fmt_time`: minutes, seconds and tenths. */
	export const time = (ms: number) => {
		const s = Math.max(0, ms) / 1000;
		return `${Math.floor(s / 60)}:${(s % 60).toFixed(1).padStart(4, '0')}`;
	};
</script>

<script lang="ts">
	import { clamp } from '#lib/ease.js';

	type Props = {
		duration: number;
		/** Width of the tracks in pixels. */
		width: number;
		playhead: number;
		step: number;
		clips?: Segment[];
		zooms?: Segment[];
		masks?: Segment[];
		voiceovers?: { start: number; end: number }[];
		transport?: boolean;
		playing?: boolean;
		/** Leaves out the Delete button, for narrow timelines. */
		compact?: boolean;
	};

	let {
		duration,
		width,
		playhead,
		step,
		clips = [{ start: 0, end: duration, label: 'Clip' }],
		zooms = [],
		masks = [],
		voiceovers = [],
		transport = true,
		playing = false,
		compact = false
	}: Props = $props();

	const x = (ms: number) => (clamp(ms, 0, duration) / duration) * width;
	let ticks = $derived(Array.from({ length: Math.floor(duration / step) + 1 }, (_, i) => i * step));
	let rows = $derived([
		{ name: 'Clips', segments: clips, kind: 'clip' },
		{ name: 'Zoom', segments: zooms, kind: 'zoom' },
		{ name: 'Masks', segments: masks, kind: 'mask' }
	]);
</script>

<div class="timeline" style:width="{width}px">
	{#if transport}
		<div class="transport">
			<span class="button play" data-target="play">
				{#if playing}
					<svg viewBox="0 0 10 10"><path d="M2 1.2h2.2v7.6H2zM5.8 1.2H8v7.6H5.8z" /></svg>
				{:else}
					<svg viewBox="0 0 10 10"><path d="M2 .8v8.4L9 5z" /></svg>
				{/if}
			</span>
			<span class="clock">{time(playhead)} / {time(duration)}</span>
			<span class="plain">Loop</span>
			<span class="divider"></span>
			<span class="button">✂ Split</span>
			<span class="button">+ Zoom</span>
			<span class="button">+ Mask</span>
			{#if !compact}<span class="button off">Delete</span>{/if}
		</div>
	{/if}
	<div class="tracks">
		{#each ticks as tick (tick)}
			<span class="tick" style:left="{x(tick)}px"></span>
			{#if x(tick) + 40 <= width}
				<span class="tick-label" style:left="{x(tick) + 3}px">{time(tick)}</span>
			{/if}
		{/each}
		{#each rows as row, i (row.name)}
			<div class="track" style:top="{20 + i * 46}px">
				<span class="name">{row.name}</span>
				{#each row.segments as s (s.start)}
					{@const shown = s.shown ?? 1}
					{#if shown > 0}
						<span
							class={['segment', row.kind, { selected: s.selected }]}
							style:left="{x(s.start) + 1}px"
							style:width="{Math.max(0, x(s.end) - x(s.start) - 2)}px"
							style:opacity={Math.min(1, shown * 1.5)}
							style:transform="scale({0.7 + 0.3 * shown})"
						>
							{#if x(s.end) - x(s.start) > 40}{s.label}{/if}
						</span>
					{/if}
				{/each}
				{#if i === 0}
					{#each voiceovers as v (v.start)}
						<span class="voice" style:left="{x(v.start) + 2}px" style:width="{x(v.end) - x(v.start) - 4}px"></span>
					{/each}
				{/if}
			</div>
		{/each}
		<span class="playhead" style:left="{x(playhead)}px"></span>
	</div>
</div>

<style>
	.timeline {
		display: flex;
		flex-direction: column;
		gap: 8px;
		color: var(--text);
	}

	.transport {
		display: flex;
		align-items: center;
		gap: 6px;
		height: 24px;
		font-size: 13px;
	}

	.button {
		display: flex;
		align-items: center;
		height: 22px;
		padding: 0 9px;
		border-radius: 6px;
		background: var(--control);
		box-shadow: inset 0 0 0 0.5px var(--control-edge);
		white-space: nowrap;
	}

	.button.off {
		color: var(--tertiary);
	}

	.play {
		justify-content: center;
		width: 32px;
		height: 24px;
		padding: 0;
	}

	.play svg {
		width: 10px;
		height: 10px;
		fill: var(--text);
	}

	.clock {
		margin: 0 2px;
		font: 13px var(--mono);
		font-variant-numeric: tabular-nums;
		white-space: nowrap;
	}

	.plain {
		padding: 0 4px;
	}

	.divider {
		width: 1px;
		height: 16px;
		margin: 0 2px;
		background: var(--separator);
	}

	.tracks {
		position: relative;
		height: 160px;
	}

	.tick {
		position: absolute;
		top: 13px;
		width: 1px;
		height: 5px;
		background: var(--tertiary);
	}

	.tick-label {
		position: absolute;
		top: 1px;
		color: var(--secondary);
		font-size: 10px;
		line-height: 12px;
		white-space: nowrap;
	}

	.track {
		position: absolute;
		left: 0;
		right: 0;
		height: 40px;
		border-radius: 6px;
		background: var(--track);
	}

	.name {
		position: absolute;
		top: 13px;
		right: 8px;
		color: var(--tertiary);
		font-size: 10px;
		line-height: 13px;
	}

	.segment {
		position: absolute;
		top: 0;
		bottom: 0;
		display: flex;
		align-items: center;
		overflow: hidden;
		padding-left: 8px;
		border-radius: 6px;
		color: #fff;
		font-size: 11px;
		font-weight: 600;
		white-space: nowrap;
		transform-origin: 0 50%;
	}

	.clip {
		background: var(--clip);
	}

	.zoom {
		background: var(--zoom);
	}

	.mask {
		background: var(--mask);
	}

	.selected {
		box-shadow: inset 0 0 0 2px #fff;
	}

	.clip.selected {
		background: var(--clip-on);
	}

	.voice {
		position: absolute;
		bottom: 3px;
		height: 4px;
		border-radius: 2px;
		background: #06b6d4;
	}

	.playhead {
		position: absolute;
		top: 0;
		bottom: 0;
		width: 2px;
		margin-left: -1px;
		background: var(--accent);
	}

	.playhead::before {
		content: '';
		position: absolute;
		top: 0;
		left: -5px;
		width: 12px;
		height: 8px;
		background: var(--accent);
		clip-path: polygon(0 0, 100% 0, 50% 100%);
	}
</style>
