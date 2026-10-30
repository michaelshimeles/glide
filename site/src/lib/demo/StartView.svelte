<script lang="ts">
	import Dashboard from '#lib/ui/Dashboard.svelte';
	import Segmented from '#lib/ui/Segmented.svelte';
	import TrafficLights from '#lib/ui/TrafficLights.svelte';
	import { DESK, STAGE } from './script.js';

	/** DaisyCap's start window (`home.rs`), in points: toolbar, live preview, settings and record bar. */
	let {
		window,
		pressed = null,
		t
	}: { window: number; pressed?: 'window' | 'record' | null; t: number } = $props();

	let chosen = $derived(window > 0.5);
	const meter = (seed: number) => 0.52 + 0.1 * Math.sin(t / 130 + seed) + 0.06 * Math.sin(t / 47 + seed * 3);
</script>

{#snippet check(label: string, on: boolean)}
	<div class="check">
		<span class={['box', { on }]}>{#if on}✓{/if}</span>{label}
	</div>
{/snippet}

{#snippet combo(label: string)}
	<span class="combo">
		{label}
		<svg viewBox="0 0 8 5"><path d="M0 0h8L4 5z" /></svg>
	</span>
{/snippet}

<div class="start">
	<div class="toolbar">
		<TrafficLights />
		<span class="title">New recording <svg viewBox="0 0 8 5"><path d="M.8.8 4 4 7.2.8" /></svg></span>
		<span class="pill">PREVIEW</span>
	</div>

	<div class="preview">
		<div class="screen" style:opacity={1 - window}>
			<div class="desktop" style:transform="scale({988 / STAGE.w})">
				<div class="desk" style:left="{DESK.x}px" style:top="{DESK.y}px" style:transform="scale({DESK.scale})">
					<Dashboard />
				</div>
			</div>
		</div>
		<div class="screen" style:opacity={window}>
			<div class="only"><Dashboard /></div>
		</div>
	</div>

	<aside class="sidebar">
		<h4>SCENE</h4>
		<div class="choice">
			<span class="on">Screen</span><span>Screen + Cam</span>
		</div>
		<h4>SCREEN</h4>
		<div class="choice">
			<span class={{ on: !chosen }}>Display</span>
			<span class={{ on: chosen, pressed: pressed === 'window' }} data-target="window">Window</span>
			<span>Area</span>
			<span>iPhone</span>
		</div>
		<div class="row">
			{#if chosen}
				{@render combo('Dashboard')}
				<span class="button square">↻</span>
			{:else}
				{@render combo('Main display')}
			{/if}
		</div>
		{#if chosen}<span class="small">Pick on screen…</span>{/if}
		<h4>AUDIO</h4>
		{@render check('System audio', true)}
		{@render check('Microphone', true)}
		<div class="row">{@render combo('Default microphone')}</div>
		<h4>OPTIONS</h4>
		{@render check('Record key presses', true)}
		{@render check('3 s countdown', true)}
		{@render check('Hide desktop icons', false)}
		{@render check('Hide Dock', false)}
		<div class="rate">
			<span>Frame rate</span>
			<Segmented items={['60', '30']} selected={0} />
		</div>
	</aside>

	<div class="controls">
		<span class={['record', { pressed: pressed === 'record' }]} data-target="record"><span></span></span>
		<span class="clock">00:00</span>
		<div class="meters">
			{#each ['System', 'Mic'] as label, i (label)}
				<div class="meter">
					<span>{label}</span>
					<span class="level"><span style:width="{meter(i * 2.1) * 100}%"></span></span>
				</div>
			{/each}
		</div>
		<span class="spacer"></span>
		<span class="summary">Record {chosen ? 'window' : 'display'} · 3 s countdown · ⌥⇧⌘R</span>
		<span class="button">Open Folder</span>
	</div>
</div>

<style>
	.start {
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
		gap: 8px;
		height: 40px;
		padding-left: 88px;
		border-bottom: 1px solid var(--separator);
	}

	.title {
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 3px 8px;
		border-radius: 6px;
		background: var(--control);
		font-weight: 600;
	}

	.title svg {
		width: 8px;
		height: 5px;
		fill: none;
		stroke: var(--text);
		stroke-width: 1.4;
	}

	.pill {
		padding: 3px 8px;
		border-radius: 9px;
		background: var(--control);
		color: var(--secondary);
		font-size: 11px;
		font-weight: 600;
	}

	.preview {
		position: absolute;
		top: 40px;
		left: 0;
		width: 1020px;
		height: 684px;
		background: var(--window);
	}

	.screen {
		position: absolute;
		inset: 16px;
		overflow: hidden;
		border-radius: 10px;
		background: var(--stage);
	}

	.desktop {
		position: absolute;
		top: calc((652px - 988px * 0.6) / 2);
		left: 0;
		width: 1200px;
		height: 720px;
		background:
			radial-gradient(circle farthest-corner at 50% 50%, rgba(64, 110, 255, 0.42), transparent 62%),
			radial-gradient(circle farthest-corner at 4% 112%, rgba(40, 200, 200, 0.26), transparent 60%),
			radial-gradient(circle farthest-corner at 104% 96%, rgba(130, 90, 255, 0.3), transparent 62%),
			linear-gradient(158deg, #121830 0%, #0a0d1a 46%, #0e1222 100%);
		transform-origin: 0 0;
	}

	.desk {
		position: absolute;
		width: 960px;
		height: 600px;
		overflow: hidden;
		border-radius: 10px;
		box-shadow: 0 20px 50px rgba(0, 0, 0, 0.5);
		transform-origin: 0 0;
	}

	.only {
		position: absolute;
		top: 40px;
		left: 40px;
		width: 960px;
		height: 600px;
		overflow: hidden;
		border-radius: 10px;
		transform: scale(0.946);
		transform-origin: 0 0;
	}

	.sidebar {
		position: absolute;
		top: 40px;
		right: 0;
		bottom: 68px;
		display: flex;
		flex-direction: column;
		width: 300px;
		padding: 4px 18px 6px 14px;
		background: var(--sidebar);
	}

	h4 {
		margin: 12px 0 4px;
		color: var(--secondary);
		font-size: 11px;
		font-weight: 600;
		line-height: 14px;
	}

	.choice {
		display: flex;
		gap: 4px;
	}

	.choice span {
		flex: 1;
		height: 26px;
		border-radius: 6px;
		font-size: 12px;
		line-height: 26px;
		text-align: center;
		white-space: nowrap;
		transition:
			background-color 0.15s,
			filter 0.1s;
	}

	.choice .on {
		background: var(--accent);
		color: #fff;
	}

	.choice .pressed {
		filter: brightness(0.82);
	}

	.row {
		display: flex;
		gap: 6px;
		margin-top: 6px;
	}

	.combo {
		display: flex;
		flex: 1;
		align-items: center;
		justify-content: space-between;
		height: 24px;
		padding: 0 8px;
		border-radius: 6px;
		background: var(--control);
		box-shadow: inset 0 0 0 0.5px var(--control-edge);
	}

	.combo svg {
		width: 8px;
		height: 5px;
		fill: var(--text);
	}

	.button {
		display: grid;
		place-items: center;
		height: 24px;
		padding: 0 10px;
		border-radius: 6px;
		background: var(--control);
		box-shadow: inset 0 0 0 0.5px var(--control-edge);
		white-space: nowrap;
	}

	.square {
		width: 24px;
		padding: 0;
	}

	.small {
		align-self: flex-start;
		margin-top: 6px;
		padding: 1px 6px;
		border-radius: 5px;
		background: var(--control);
		font-size: 12px;
	}

	.check {
		display: flex;
		align-items: center;
		gap: 6px;
		height: 22px;
	}

	.box {
		display: grid;
		place-items: center;
		width: 14px;
		height: 14px;
		border-radius: 4px;
		background: var(--field);
		box-shadow: inset 0 0 0 0.5px var(--control-edge);
		font-size: 10px;
	}

	.rate {
		display: flex;
		align-items: center;
		justify-content: space-between;
		margin-top: 4px;
	}

	.controls {
		position: absolute;
		inset: auto 0 0;
		display: flex;
		align-items: center;
		height: 68px;
		padding: 0 16px;
		border-top: 1px solid var(--separator);
		background: var(--window);
	}

	.record {
		display: grid;
		place-items: center;
		width: 44px;
		height: 44px;
		border-radius: 50%;
		box-shadow: inset 0 0 0 3px var(--secondary);
		transition: scale 0.1s;
	}

	.record span {
		width: 32px;
		height: 32px;
		border-radius: 50%;
		background: #ff3b30;
	}

	.record.pressed {
		scale: 0.9;
	}

	.clock {
		margin: 0 16px 0 10px;
		color: var(--tertiary);
		font: 22px var(--mono);
	}

	.meters {
		display: flex;
		flex-direction: column;
		gap: 5px;
		font-size: 11px;
	}

	.meter {
		display: flex;
		align-items: center;
		gap: 8px;
	}

	.meter > span:first-child {
		width: 38px;
		color: var(--secondary);
	}

	.level {
		width: 140px;
		height: 6px;
		overflow: hidden;
		border-radius: 3px;
		background: var(--field);
	}

	.level span {
		display: block;
		height: 100%;
		background: linear-gradient(90deg, #34c759 0 78%, #ffd60a 78% 92%, #ff453a 92%);
		background-size: 140px 100%;
	}

	.spacer {
		flex: 1;
	}

	.summary {
		margin-right: 8px;
		color: var(--secondary);
		font-size: 11px;
	}
</style>
