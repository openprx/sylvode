/**
 * Lets a plain `bun tests/<file>.ts` run import `.svelte.ts` modules (classes using runes). The
 * module is stripped of types by Bun, then compiled for the client by Svelte, so `$state` is a
 * real signal and reads and writes behave as in the browser. `installSvelteSsr()` is separate and
 * handles `.svelte` components.
 */
import { readFile } from 'node:fs/promises';

interface PluginBuild {
	onLoad(
		options: { filter: RegExp },
		load: (args: { path: string }) => Promise<{ contents: string; loader: 'js' }>
	): void;
}
interface BunRuntime {
	plugin(options: { name: string; setup(build: PluginBuild): void }): void;
	Transpiler: new (options: { loader: 'ts' }) => { transformSync(source: string): string };
}

let installed = false;

export async function installRuneModules(): Promise<void> {
	if (installed) return;
	installed = true;
	const { compileModule } = await import('svelte/compiler');
	const bun = (globalThis as unknown as { Bun?: BunRuntime }).Bun;
	if (!bun) throw new Error('installRuneModules needs the Bun runtime');
	const transpiler = new bun.Transpiler({ loader: 'ts' });
	bun.plugin({
		name: 'shell-test-rune-modules',
		setup(build) {
			build.onLoad({ filter: /\.svelte\.ts$/ }, async (args) => {
				const source = await readFile(args.path, 'utf8');
				const { js } = compileModule(transpiler.transformSync(source), {
					filename: args.path,
					generate: 'client',
					dev: false
				});
				return { contents: js.code, loader: 'js' };
			});
		}
	});
}
