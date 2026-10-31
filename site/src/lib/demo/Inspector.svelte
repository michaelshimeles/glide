<script lang="ts" module>
	export type Tab = 'background' | 'cursor';
	export type Smoothing = 'Smooth' | 'Off';
</script>

<script lang="ts">
	import Chips from '#lib/ui/Chips.svelte';
	import Segmented from '#lib/ui/Segmented.svelte';
	import Slider from '#lib/ui/Slider.svelte';
	import Swatches from '#lib/ui/Swatches.svelte';
	import type { WallpaperId } from '#lib/ui/wallpaper.js';

	type Props = {
		tab: Tab;
		smoothing: Smoothing;
		wallpaper: WallpaperId;
		padding: number;
		roundness: number;
		/** The control under the pointer's button, if any. */
		pressed?: string | null;
	};

	/** The editor's inspector (`inspector.rs`) with the Background and Cursor tabs. */
	let { tab, smoothing, wallpaper, padding, roundness, pressed = null }: Props = $props();

	const TABS = ['Background', 'Cursor', 'Camera', 'Captions', 'Audio', 'Keys', 'Motion'];
	const SMOOTHING = ['Smooth', 'Medium', 'Rapid', 'Off'];
</script>

{#snippet toggle(label: string, on: boolean)}
	<div class="row">
		<span>{label}</span>
		<span class={['switch', { on }]}><span></span></span>
	</div>
{/snippet}

<div class="inspector">
	<div class="tabs">
		<Chips items={TABS} selected={tab === 'cursor' ? 'Cursor' : 'Background'} pressed={TABS.find((l) => l.toLowerCase() === pressed)} />
	</div>

	{#if tab === 'background'}
		<section>
			<h5>Background</h5>
			<Segmented items={['Wallpaper', 'Gradient', 'Color', 'Image']} selected={0} />
			<div class="swatches">
				<Swatches selected={wallpaper} pressed={pressed?.startsWith('wp:') ? (pressed.slice(3) as WallpaperId) : null} />
			</div>
			<Slider label="Blur" value={0} text="0.0 px" />
		</section>
		<section>
			<h5>Frame</h5>
			<div data-target="padding">
				<Slider label="Padding" value={padding / 40} text="{padding.toFixed(1)}%" active={pressed === 'padding'} />
			</div>
			<div class="row">
				<span>Device</span>
				<Segmented items={['None', 'Laptop', 'Phone', 'Tablet']} selected={0} />
			</div>
			<div data-target="roundness">
				<Slider
					label="Roundness"
					value={roundness / 60}
					text="{Math.round(roundness)} px"
					active={pressed === 'roundness'}
				/>
			</div>
			<Slider label="Inset" value={0} text="0.0 px" />
			<div class="row">
				<span>Aspect ratio</span>
				<span class="combo">Auto <svg viewBox="0 0 8 5"><path d="M0 0h8L4 5z" /></svg></span>
			</div>
		</section>
		<section>
			<h5>Shadow</h5>
			<Slider label="Intensity" value={0.75} text="0.75" />
			<Slider label="Angle" value={0.25} text="90°" />
			<Slider label="Distance" value={0.25} text="25 px" />
			<Slider label="Blur" value={0.2} text="20 px" />
		</section>
		<section>
			<h5>Watermark</h5>
			<div class="row">
				<span>Daisy mark <span class="pro">PRO</span></span>
				<span class="switch on"><span></span></span>
			</div>
		</section>
	{:else}
		<section>
			<h5>Cursor</h5>
			{@render toggle('Hide cursor', false)}
			<Slider label="Size" value={(1.5 - 0.5) / 3.5} text="1.5×" />
			<div class="row">
				<span>Style</span>
				<Segmented items={['Touch', 'Vector', 'System']} selected={2} />
			</div>
		</section>
		<section>
			<h5>Movement</h5>
			<span class="label">Smoothing</span>
			<div class="chips">
				<Chips items={SMOOTHING} selected={smoothing} pressed={SMOOTHING.find((l) => l.toLowerCase() === pressed)} />
			</div>
			<Slider label="Tilt" value={0.5} text="0.5" />
			{@render toggle('Hide when idle', false)}
			{@render toggle('Loop position at end', false)}
			{@render toggle('Freeze at end', false)}
		</section>
		<section>
			<h5>Clicks</h5>
			<Segmented items={['None', 'Circle', 'Ripple']} selected={1} />
			{@render toggle('Click sounds', true)}
			<div class="row">
				<span>Sound</span>
				<span class="combo">Classic <svg viewBox="0 0 8 5"><path d="M0 0h8L4 5z" /></svg></span>
			</div>
			<Slider label="Click volume" value={0.25} text="0.25" />
		</section>
	{/if}
</div>

<style>
	.inspector {
		position: absolute;
		top: 40px;
		right: 0;
		bottom: 0;
		width: 320px;
		overflow: hidden;
		padding: 10px 14px;
		background: var(--inspector);
		color: var(--text);
		font-size: 13px;
	}

	.tabs {
		padding-bottom: 8px;
		border-bottom: 1px solid var(--separator);
	}

	section {
		display: flex;
		flex-direction: column;
		align-items: stretch;
		gap: 4px;
		padding: 6px 0 9px;
		border-bottom: 1px solid var(--separator);
	}

	section > :global(.segmented) {
		align-self: flex-start;
	}

	h5 {
		margin: 0 0 1px;
		color: var(--secondary);
		font-size: 12px;
		font-weight: 600;
		line-height: 18px;
	}

	.swatches {
		margin: 4px 0 6px;
	}

	.row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		min-height: 22px;
	}

	.pro {
		display: inline-block;
		margin-left: 5px;
		padding: 0 5px;
		border: 1px solid color-mix(in srgb, var(--accent) 70%, transparent);
		border-radius: 7px;
		color: var(--accent);
		font-size: 9px;
		font-weight: 600;
		line-height: 12px;
		vertical-align: 1px;
	}

	.label {
		line-height: 20px;
	}

	.chips {
		margin-bottom: 2px;
	}

	.switch {
		position: relative;
		width: 30px;
		height: 18px;
		border-radius: 9px;
		background: var(--separator);
	}

	.switch span {
		position: absolute;
		top: 2px;
		left: 2px;
		width: 14px;
		height: 14px;
		border-radius: 50%;
		background: #fff;
		box-shadow: 0 1px 2px rgba(0, 0, 0, 0.3);
	}

	.switch.on {
		background: var(--accent);
	}

	.switch.on span {
		left: 14px;
	}

	.combo {
		display: flex;
		align-items: center;
		justify-content: space-between;
		width: 76px;
		height: 22px;
		padding: 0 8px;
		border-radius: 6px;
		background: var(--control);
		box-shadow: inset 0 0 0 0.5px var(--control-edge);
		font-size: 12px;
	}

	.combo svg {
		width: 8px;
		height: 5px;
		fill: var(--text);
	}
</style>
