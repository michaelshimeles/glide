<script lang="ts">
	import type { Attachment } from 'svelte/attachments';
	import { span } from '#lib/ease.js';
	import Scene from './Scene.svelte';
	import type { Player } from './player.svelte.js';
	import { CHAPTERS, DURATION, STAGE } from './script.js';

	let { player }: { player: Player } = $props();
	let scale = $state(1);

	const fit: Attachment<HTMLElement> = (node) => {
		const observer = new ResizeObserver(() => (scale = node.clientWidth / STAGE.w));
		observer.observe(node);
		return () => observer.disconnect();
	};

	const chapters = CHAPTERS.map((c, i) => ({ ...c, end: CHAPTERS[i + 1]?.t ?? DURATION }));
</script>

<div class="showcase">
	<div
		class="frame"
		role="img"
		aria-label="Demo: DaisyCap records a window while the pointer clicks two buttons, then the editor plays it back with automatic zooms on each click. Cursor smoothing is switched off and on, the background changes from Aurora to Ocean, Grape and Lagoon with more padding and rounder corners, and the video exports at 3.9 times real time."
		{@attach fit}
		{@attach player.observe}
	>
		<div class="stage" style:transform="scale({scale})">
			<Scene t={player.time} />
		</div>
		{#if !player.sound}
			<button type="button" class="listen glass" onclick={() => player.playWithSound()}>
				<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4.5 3.2v9.6L12.8 8z" /></svg>
				Play with sound
			</button>
		{/if}
	</div>

	<div class="controls">
		<button type="button" class="icon" aria-label={player.paused ? 'Play' : 'Pause'} onclick={() => player.toggle()}>
			{#if player.paused}
				<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M5 3.2v9.6L12.6 8z" /></svg>
			{:else}
				<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4.5 3h2.4v10H4.5zM9.1 3h2.4v10H9.1z" /></svg>
			{/if}
		</button>
		<button type="button" class="icon" aria-label="Replay" onclick={() => player.replay()}>
			<svg viewBox="0 0 16 16" aria-hidden="true" class="stroke">
				<path d="M3.2 8a4.8 4.8 0 1 0 1.5-3.5" />
				<path d="M3 2.6v2.6h2.6" />
			</svg>
		</button>
		<button
			type="button"
			class="sound"
			aria-pressed={player.sound}
			aria-label={player.sound ? 'Mute sound' : 'Turn on sound'}
			onclick={() => player.toggleSound()}
		>
			<svg viewBox="0 0 16 16" aria-hidden="true" class="stroke">
				<path d="M2.5 6h2.2L8 3.3v9.4L4.7 10H2.5z" class="fill" />
				{#if player.sound}
					<path d="M10.5 5.6a3.4 3.4 0 0 1 0 4.8M12.3 3.9a5.8 5.8 0 0 1 0 8.2" />
				{:else}
					<path d="M10.6 6.1l3.4 3.8M14 6.1l-3.4 3.8" />
				{/if}
			</svg>
			Sound
		</button>
		<ol class="chapters">
			{#each chapters as c, i (c.label)}
				{@const p = span(player.time, c.t, c.end)}
				<li style:flex-grow={c.end - c.t}>
					<button
						type="button"
						class={{ active: p > 0 && p < 1 }}
						aria-label="Chapter {i + 1}: {c.label}"
						onclick={() => player.seek(c.t)}
					>
						<span class="track"><span class="fill" style:transform="scaleX({p})"></span></span>
						<span class="label"><span class="num">{i + 1}</span>{c.label}</span>
					</button>
				</li>
			{/each}
		</ol>
	</div>
</div>

<style>
	.showcase {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 14px;
		width: 100%;
	}

	.glass {
		background: rgba(18, 18, 24, 0.42);
		backdrop-filter: blur(14px) saturate(190%) brightness(106%);
		box-shadow:
			inset 0 0.5px 0 rgba(255, 255, 255, 0.28),
			inset 0 0 0 0.5px rgba(255, 255, 255, 0.14),
			0 10px 28px -8px rgba(0, 0, 0, 0.5);
	}

	.frame {
		position: relative;
		width: 100%;
		max-width: 1200px;
		aspect-ratio: 5 / 3;
		overflow: hidden;
		border-radius: 16px;
		background: #0b0d18;
		box-shadow:
			0 0 0 1px rgba(255, 255, 255, 0.09),
			0 40px 120px -20px rgba(0, 0, 0, 0.8);
	}

	:global(:root[data-theme='light']) .frame {
		box-shadow:
			0 0 0 1px rgba(0, 0, 0, 0.08),
			0 40px 100px -24px rgba(0, 0, 0, 0.35);
	}

	.stage {
		position: absolute;
		top: 0;
		left: 0;
		transform-origin: 0 0;
	}

	.listen {
		position: absolute;
		right: 20px;
		bottom: 20px;
		display: flex;
		align-items: center;
		gap: 8px;
		height: 44px;
		padding: 0 20px 0 16px;
		border: 0;
		border-radius: 999px;
		color: #f4f4f5;
		font: 600 15px var(--sans);
		cursor: pointer;
		transition: scale 0.2s;
	}

	.listen:hover {
		scale: 1.04;
	}

	.listen svg {
		width: 15px;
		height: 15px;
		fill: currentColor;
	}

	.controls {
		display: flex;
		align-items: flex-start;
		gap: 8px;
		width: 100%;
		max-width: 1200px;
	}

	.icon,
	.sound {
		display: flex;
		flex-shrink: 0;
		align-items: center;
		justify-content: center;
		gap: 6px;
		height: 30px;
		border: 0;
		border-radius: 999px;
		background: color-mix(in srgb, var(--fg) 7%, transparent);
		color: var(--fg);
		font: 500 13px var(--sans);
		cursor: pointer;
		transition: background 0.15s;
	}

	.icon {
		width: 30px;
	}

	.sound {
		padding: 0 12px 0 10px;
		color: var(--muted);
	}

	.sound[aria-pressed='true'] {
		color: var(--fg);
	}

	.icon:hover,
	.sound:hover {
		background: color-mix(in srgb, var(--fg) 12%, transparent);
	}

	.controls svg {
		width: 14px;
		height: 14px;
		fill: currentColor;
	}

	.controls svg.stroke {
		fill: none;
		stroke: currentColor;
		stroke-width: 1.5;
		stroke-linecap: round;
		stroke-linejoin: round;
	}

	.controls svg .fill {
		fill: currentColor;
	}

	.chapters {
		display: flex;
		flex: 1;
		gap: 4px;
		min-width: 0;
		margin: 0 0 0 8px;
		padding: 0;
		list-style: none;
	}

	.chapters li {
		flex-basis: 0;
		min-width: 0;
	}

	.chapters button {
		display: flex;
		flex-direction: column;
		gap: 8px;
		width: 100%;
		padding: 13px 0 0;
		border: 0;
		background: none;
		color: var(--muted);
		font: 12px var(--sans);
		text-align: left;
		cursor: pointer;
	}

	.track {
		display: block;
		height: 4px;
		overflow: hidden;
		border-radius: 2px;
		background: color-mix(in srgb, var(--fg) 12%, transparent);
	}

	.chapters .fill {
		display: block;
		height: 100%;
		background: var(--fg);
		transform-origin: 0 50%;
	}

	.label {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.num {
		margin-right: 6px;
		font-family: var(--mono);
		font-size: 11px;
	}

	.chapters button.active,
	.chapters button:hover {
		color: var(--fg);
	}

	@media (max-width: 640px) {
		.listen {
			top: 8px;
			right: 8px;
			bottom: auto;
			gap: 6px;
			height: 28px;
			padding: 0 11px 0 9px;
			font-size: 12px;
		}

		.label {
			display: none;
		}

		.chapters button {
			padding-top: 13px;
		}
	}
</style>
