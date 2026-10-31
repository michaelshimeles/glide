import adapter from '@sveltejs/adapter-vercel';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	plugins: [
		sveltekit({
			compilerOptions: {
				// Force runes mode for the project, except for libraries. Can be removed in svelte 6.
				runes: ({ filename }) =>
					filename.split(/[/\\]/).includes('node_modules') ? undefined : true
			},
			// The local Node version can be newer than Vercel's runtimes
			// (adapter-vercel refuses Node 26); pin a supported one.
			adapter: adapter({ runtime: 'nodejs24.x' }),
			// A one-page site has nothing to refresh, so don't poll for new deployments.
			version: { pollInterval: 0 }
		})
	]
});
