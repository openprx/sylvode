<svelte:options runes />

<script lang="ts">
	import { goto } from '$app/navigation';
	import { locale, t } from 'svelte-i18n';
	import { onMount, untrack } from 'svelte';
	import { authStore } from '#lib/stores/auth.js';
	import { resolvedTheme } from '#lib/theme.js';
	import { get } from 'svelte/store';
	import { provideRuntime } from './context';
	import { LayoutRuntime } from './runtime.svelte';
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
		const slot: Record<string, unknown> = { ...context };
		const params: Record<string, unknown> = {};
		for (const name of Object.keys(spec.params ?? {})) {
			if (name in slot) params[name] = slot[name];
		}
		return new LayoutRuntime({
			spec,
			context,
			workspaceId,
			slot,
			params,
			installations,
			viewer: {
				id: get(authStore).user?.id ?? '',
				role: get(authStore).user?.role ?? 'member',
				locale: get(locale) ?? 'en',
				theme: get(resolvedTheme)
			},
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
