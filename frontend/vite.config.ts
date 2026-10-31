import adapter from '@sveltejs/adapter-static';
import tailwindcss from '@tailwindcss/vite';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	plugins: [
		tailwindcss(),
		sveltekit({
			adapter: adapter({
				pages: 'build',
				assets: 'build',
				fallback: 'index.html',
				precompress: false,
				strict: true
			}),
			// Local stand-in for the `svelte-i18n` import used across the app. Kit marks
			// `alias` deprecated in favor of package.json imports; this name is not a `#lib`
			// path, and both the typechecker and Vite still resolve it from here.
			alias: { 'svelte-i18n': 'src/lib/i18n/svelte-i18n.ts' }
		})
	],
	resolve: {
		alias: {
			'svelte-i18n': '/src/lib/i18n/svelte-i18n.ts',
			// `loro-crdt`'s conditional `browser` export map resolves `bundler/index.js` (an ESM
			// wasm-bindgen import Vite's dev pre-transform cannot handle) under `vite dev` and
			// `browser/index.js` (the synchronous-XHR entry, no top-level await) under `vite build`.
			// Forcing the browser entry in both modes keeps engine wiring identical dev vs prod.
			// See spikes/bundle-loro/vite.config.ts and contracts/ui-surface-v1.md item 1/2.
			'loro-crdt': 'loro-crdt/browser'
		}
	},
	build: {
		// contracts/ui-surface-v1.md item 2: the browser matrix this app targets supports
		// top-level await, and loro-crdt's browser entry deliberately avoids TLA anyway.
		target: 'es2022'
	},
	optimizeDeps: {
		// Never let Vite's dev-server esbuild pre-bundling pass re-process the wasm-bindgen glue
		// or the loro-prosemirror binding (contracts/ui-surface-v1.md item 3).
		exclude: ['loro-crdt', 'loro-prosemirror']
	}
});
