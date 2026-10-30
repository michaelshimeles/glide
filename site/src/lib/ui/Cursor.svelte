<script lang="ts">
	/** DaisyCap's vector cursor (`art.rs`), with its tip at the element's top-left corner. */
	let { size = 24 }: { size?: number } = $props();
</script>

<svg
	class="cursor"
	width={(size * 15.5) / 22.5}
	height={size}
	viewBox="-1.5 -1.5 15.5 22.5"
	style:margin="{(-size * 1.5) / 22.5}px 0 0 {(-size * 1.5) / 22.5}px"
	aria-hidden="true"
>
	<path d="M0 0L0 16.8 4.1 12.9 6.9 19.4 9.3 18.4 6.6 12 12.2 12Z" />
</svg>

<style>
	.cursor {
		display: block;
		overflow: visible;
		filter: drop-shadow(0 1px 1.5px rgba(0, 0, 0, 0.35));
	}

	path {
		fill: #000;
		stroke: #fff;
		stroke-width: 1.3;
		stroke-linejoin: round;
	}
</style>
