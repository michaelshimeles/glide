<script lang="ts">
	/** DaisyCap's segmented control. `accent` fills the selected segment with blue, as in recording setup. */
	let {
		items,
		selected,
		pressed = -1,
		accent = false,
		stretch = false
	}: { items: readonly string[]; selected: number; pressed?: number; accent?: boolean; stretch?: boolean } = $props();
</script>

<div class={['segmented', { accent, stretch }]}>
	{#each items as item, i (item)}
		<span class={{ on: i === selected, pressed: i === pressed }}>{item}</span>
	{/each}
</div>

<style>
	.segmented {
		display: inline-flex;
		flex-shrink: 0;
		gap: 2px;
		padding: 2px;
		border-radius: 7px;
		background: var(--field);
		box-shadow: inset 0 0 0 0.5px var(--control-edge);
	}

	.stretch {
		display: flex;
	}

	.stretch span {
		flex: 1;
		text-align: center;
	}

	span {
		padding: 2px 8px;
		border-radius: 5px;
		color: var(--text);
		font-size: 12px;
		line-height: 15px;
		white-space: nowrap;
		transition:
			background-color 0.15s,
			color 0.15s;
	}

	.on {
		background: var(--segment);
		box-shadow: 0 1px 2px rgba(0, 0, 0, 0.18);
	}

	.accent .on {
		background: var(--accent);
		color: #fff;
	}

	.pressed {
		filter: brightness(0.86);
	}
</style>
