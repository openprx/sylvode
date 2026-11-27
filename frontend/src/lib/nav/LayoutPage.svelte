<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import { layoutApi } from '#lib/layout/api.js';
	import LayoutView from '#lib/layout/LayoutView.svelte';
	import type { LayoutRecord } from '#lib/layout/types.js';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';

	/**
	 * A full console page for a layout (`page` slot), reached from a navigation entry or by its
	 * URL. The layout is read with its own permission check; the navigation grants nothing.
	 */
	let {
		workspaceId,
		projectId = null,
		layoutId
	}: { workspaceId: string; projectId?: string | null; layoutId: string } = $props();

	type Loaded =
		| { status: 'loading' }
		| { status: 'ready'; layout: LayoutRecord }
		| { status: 'missing' }
		| { status: 'forbidden' }
		| { status: 'failed' };

	let loaded = $state.raw<Loaded>({ status: 'loading' });

	$effect(() => {
		const id = layoutId;
		let cancelled = false;
		loaded = { status: 'loading' };
		void layoutApi
			.getLayout(id)
			.then((result) => {
				if (cancelled) return;
				if (result.code === 0 && result.data) loaded = { status: 'ready', layout: result.data };
				else if (result.code === 404) loaded = { status: 'missing' };
				else if (result.code === 403) loaded = { status: 'forbidden' };
				else loaded = { status: 'failed' };
			})
			.catch(() => {
				if (!cancelled) loaded = { status: 'failed' };
			});
		return () => {
			cancelled = true;
		};
	});

	// A workspace page has no project; the slot context says so with an empty id.
	const context = $derived({ project_id: projectId ?? '' });
</script>

<div class="page" data-testid="layout-page" data-layout-id={layoutId}>
	{#if loaded.status === 'loading'}
		<StateBlock kind="loading" thing={$t('nav.page.thing')} />
	{:else if loaded.status === 'ready'}
		{#key `${loaded.layout.id}:${loaded.layout.version}`}
			<LayoutView spec={loaded.layout.spec} {context} {workspaceId} installations={{}} />
		{/key}
	{:else if loaded.status === 'forbidden'}
		<StateBlock kind="forbidden" thing={$t('nav.page.thing')} />
	{:else if loaded.status === 'missing'}
		<StateBlock kind="notFound" object={$t('nav.page.object')} headingLevel={1} />
	{:else}
		<StateBlock kind="loadFailed" thing={$t('nav.page.thing')} />
	{/if}
</div>

<style>
	.page {
		padding: var(--rf-space-9);
		font-family: var(--rf-font-sans-stack);
		color: var(--rf-text);
	}
</style>
