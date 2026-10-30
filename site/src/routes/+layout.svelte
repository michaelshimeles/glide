<script lang="ts">
	import '@fontsource-variable/inter';
	import '../app.css';
	import inter from '@fontsource-variable/inter/files/inter-latin-wght-normal.woff2?url';
	import type { LayoutProps } from './$types';

	let { children }: LayoutProps = $props();
</script>

<svelte:head>
	<!-- DaisyCap's interface is set in Inter, and the demo that recreates it sits above the fold. -->
	<link rel="preload" as="font" type="font/woff2" href={inter} crossorigin="anonymous" />
</svelte:head>

{@render children()}
