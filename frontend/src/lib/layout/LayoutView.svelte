<svelte:options runes />

<script lang="ts">
	import { goto } from '$app/navigation';
	import { t } from 'svelte-i18n';
	import { onMount, untrack } from 'svelte';
	import { provideRuntime } from './context';
	import { LayoutRuntime } from './runtime.svelte';
	import { currentViewer, paramsFromSlot, slotValues } from './scope';
	import LayoutNode from './LayoutNode.svelte';
	import type { LayoutSpec, SlotContextParams } from './types';

	let {
		spec,
		context,
		workspaceId,
		installations
	}: {
		spec: LayoutSpec;
		context: SlotContextParams;
		workspaceId: string;
		installations: Record<string, string>;
	} = $props();

	// The runtime is built once per mounted placement; the slot re-keys it when the context changes.
	const runtime = untrack(() => {
		const slot = slotValues(context);
		const params = paramsFromSlot(spec, slot);
		return new LayoutRuntime({
			spec,
			context,
			workspaceId,
			slot,
			params,
			installations,
			viewer: currentViewer(),
			// eslint-disable-next-line svelte/no-navigation-without-resolve -- the runtime builds full app paths
			navigate: (path) => void goto(path)
		});
	});
	provideRuntime(runtime);

	onMount(() => {
		void runtime.load();
	});

	const failedSources = $derived(Object.keys(runtime.errors).length > 0);
</script>

<div class="min-w-0" data-layout-key={spec.key} aria-busy={runtime.loading || undefined}>
	<LayoutNode node={spec.root} />
	{#if failedSources}
		<p class="mt-2 text-xs text-fg-muted" data-layout-source-error>{$t('layout.sourceError')}</p>
	{/if}
</div>
