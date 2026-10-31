/**
 * Server-side rendering of a Flow Svelte component inside a plain `bun tests/<file>.ts` run, so a
 * suite can assert on the markup a component really produces (for example how many rows a
 * virtualised table puts in the DOM) instead of on its source text.
 *
 * `installSvelteSsr()` registers a Bun loader that compiles `.svelte` files for the server and
 * resolves the SvelteKit-only `$app/*` modules to inert stubs; `#lib` resolves through
 * `tsconfig.json` paths as usual. `onMount`/`$effect` never run during SSR, so only the initial
 * render is observable -- which is exactly the first paint a user gets. No locale is loaded, so
 * `$t(key)` renders the key itself (`src/lib/i18n/svelte-i18n.ts`).
 */

import { readFile } from 'node:fs/promises';

/** The slice of Bun's runtime plugin API used here (no `@types/bun` in this package). */
interface PluginBuild {
	module(specifier: string, load: () => { contents: string; loader: 'js' }): void;
	onLoad(
		options: { filter: RegExp },
		load: (args: { path: string }) => Promise<{ contents: string; loader: 'js' | 'ts' }>
	): void;
}
interface BunRuntime {
	plugin(options: { name: string; setup(build: PluginBuild): void }): void;
}

let installed = false;

export async function installSvelteSsr(): Promise<void> {
	if (installed) return;
	installed = true;
	const { compile } = await import('svelte/compiler');
	const bun = (globalThis as unknown as { Bun?: BunRuntime }).Bun;
	if (!bun) throw new Error('installSvelteSsr needs the Bun runtime');
	bun.plugin({
		name: 'flow-test-svelte-ssr',
		setup(build) {
			const stubs: Record<string, string> = {
				'$app/paths':
					'export const base = ""; export const assets = ""; export function resolve(path) { return path; }',
				'$app/navigation':
					'export async function goto() {} export function afterNavigate() {} export function beforeNavigate() {}',
				'$app/stores':
					'import { readable } from "svelte/store"; export const page = readable({ url: new URL("http://localhost/"), params: {} });',
				'$app/state':
					'export const page = { url: new URL("http://localhost/"), params: {} };',
				'$app/environment':
					'export const browser = false; export const dev = false; export const building = false;',
				'$app/env': 'export const browser = false; export const dev = false; export const building = false;',
				'svelte-i18n':
					'import { readable } from "svelte/store"; export const t = readable((key) => key); export const locale = readable("zh"); export function init() {} export function register() {} export function getLocaleFromNavigator() { return "zh"; }'
			};
			for (const [specifier, contents] of Object.entries(stubs)) {
				build.module(specifier, () => ({ contents, loader: 'js' }));
			}
			build.onLoad({ filter: /\.svelte$/ }, async (args) => {
				const source = await readFile(args.path, 'utf8');
				const { js } = compile(source, {
					filename: args.path,
					generate: 'server',
					dev: false
				});
				return { contents: js.code, loader: 'ts' };
			});
		}
	});
}

/** Renders `component` with `props` on the server and returns its HTML body. */
export async function renderToHtml(
	component: unknown,
	props: Record<string, unknown>
): Promise<string> {
	const { render } = await import('svelte/server');
	const result = render(component as Parameters<typeof render>[0], { props } as never);
	return result.body;
}
