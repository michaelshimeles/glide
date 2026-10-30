<script lang="ts">
	/** DaisyCap's selectable labels, like the inspector tabs and the Smoothing choices: blue when selected. */
	let {
		items,
		selected,
		pressed = null
	}: { items: readonly string[]; selected: string; pressed?: string | null } = $props();
</script>

<div class="chips">
	{#each items as item (item)}
		<span class={{ on: item === selected, pressed: item === pressed }} data-target={item.toLowerCase()}>{item}</span>
	{/each}
</div>

<style>
	.chips {
		display: flex;
		flex-wrap: wrap;
		gap: 4px;
	}

	span {
		padding: 2px 8px;
		border-radius: 12px;
		background: var(--control);
		box-shadow: inset 0 0 0 0.5px var(--control-edge);
		color: var(--text);
		font-size: 12px;
		line-height: 16px;
		white-space: nowrap;
		transition:
			background-color 0.15s,
			color 0.15s,
			filter 0.1s;
	}

	.on {
		background: var(--accent);
		box-shadow: none;
		color: #fff;
	}

	.pressed {
		filter: brightness(0.8);
	}
</style>
