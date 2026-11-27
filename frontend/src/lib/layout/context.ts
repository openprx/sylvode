import { getContext, setContext } from 'svelte';
import type { LayoutRuntime } from './runtime.svelte';

const KEY = Symbol('sv-layout-runtime');

export function provideRuntime(runtime: LayoutRuntime): void {
	setContext(KEY, runtime);
}

export function useRuntime(): LayoutRuntime {
	const runtime = getContext<LayoutRuntime | undefined>(KEY);
	if (!runtime) throw new Error('layout node rendered outside a layout runtime');
	return runtime;
}
