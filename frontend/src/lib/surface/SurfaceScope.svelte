<script lang="ts">
	import type { Snippet } from 'svelte';
	import { locale, t } from 'svelte-i18n';
	import { get } from 'svelte/store';
	import { getSurfaceHostProvider, provideSurfaceHostProvider } from './context';
	import type { SurfaceOpenMode, SurfaceSlot, SurfaceTarget } from './context';
	import { SurfaceScopeProvider } from './standalone-host.svelte';
	import type { SurfaceSpec } from './use-surface.svelte';
	import { measureWidth } from './width';

	interface Props {
		slot: Exclude<SurfaceSlot, 'main'>;
		/** Receives the whole spec the scoped surface declares; the parent draws it in its own chrome. */
		onspec: (spec: SurfaceSpec) => void;
		onopen?: (target: SurfaceTarget, how: SurfaceOpenMode) => void;
		onclose?: () => void;
		children: Snippet;
	}

	let { slot, onspec, onopen, onclose, children }: Props = $props();

	// The scope is created once; the options read the current props on every call.
	const provider = new SurfaceScopeProvider({
		get slot() {
			return slot;
		},
		parent: getSurfaceHostProvider(),
		onspec: (spec) => onspec(spec),
		get onopen() {
			return onopen;
		},
		get onclose() {
			return onclose;
		},
		translate: (key, values) => get(t)(key, { values }),
		locale: () => get(locale) ?? 'en'
	});
	provideSurfaceHostProvider(provider);
</script>

<div
	class="surface-scope"
	data-surface-scope={slot}
	data-surface-width={provider.widthClass}
	use:measureWidth={(px) => provider.setWidth(px)}
>
	{@render children()}
</div>

<style>
	.surface-scope {
		container-type: inline-size;
		container-name: surface;
		min-width: 0;
	}
</style>
