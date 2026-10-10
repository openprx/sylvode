<script lang="ts">
	// A Collection embedded in a Page (the server's `embed_type=collection` block). It renders the
	// registry's `collection:table` renderer over the same `CollectionViewController` the
	// Collection route uses -- the Page holds only the block reference, never the Collection's
	// state -- plus an "Open" link to the Collection itself. Opening a record goes to the
	// Collection route, where the record panel lives.
	import { getContext, onDestroy, onMount, type Component } from 'svelte';
	import { t } from 'svelte-i18n';
	import { resolve } from '$app/paths';
	import { goto } from '$app/navigation';
	import type { FlowObjectRepository } from '$lib/flow/object-repository';
	import { FLOW_REPOSITORY_CONTEXT } from '$lib/flow/context-keys';
	import { FlowCommandService } from '$lib/flow/command-service';
	import { CollectionViewController } from '$lib/flow/collection-view';
	import { resolveRenderer } from '$lib/flow/renderer-registry';

	interface Props {
		workspaceId: string;
		collectionId: string;
		blockId: string;
	}

	let { workspaceId, collectionId, blockId }: Props = $props();

	const repository = getContext<FlowObjectRepository>(FLOW_REPOSITORY_CONTEXT);
	// svelte-ignore state_referenced_locally
	const controller = new CollectionViewController(
		collectionId,
		repository,
		new FlowCommandService()
	);
	const viewState = controller.state;

	// eslint-disable-next-line @typescript-eslint/no-explicit-any
	let tableComponent = $state<Component<any> | null>(null);
	let unsupported = $state(false);

	onMount(async () => {
		const registration = resolveRenderer('collection', 'table');
		if (!registration?.load) {
			unsupported = true;
			return;
		}
		tableComponent = await registration.load();
		await controller.load();
	});

	// The record panel belongs to the Collection route: opening a row here navigates to the
	// record's own page instead of squeezing a side panel into the Page column.
	$effect(() => {
		const recordId = $viewState.panel?.recordId;
		if (!recordId) return;
		controller.closePanel();
		void goto(
			resolve('/(app)/workspace/[workspaceId]/flow/[objectId]', { workspaceId, objectId: recordId })
		);
	});

	onDestroy(() => controller.dispose());
</script>

<section
	class="not-prose my-4 overflow-hidden rounded-lg border border-slate-200 dark:border-slate-700"
	data-testid="flow-embedded-collection"
	data-collection-id={collectionId}
	data-block-id={blockId}
	aria-label={$t('flow.canvas.embed.collectionLabel', {
		values: { title: $viewState.collection?.title ?? '' }
	})}
>
	<header
		class="flex items-center gap-3 border-b border-slate-200 bg-slate-50 px-4 py-2 dark:border-slate-700 dark:bg-slate-900"
	>
		<h3 class="flex-1 truncate text-sm font-semibold text-slate-900 dark:text-slate-100">
			{$viewState.collection?.title || $t('flow.nav.untitled')}
		</h3>
		<a
			class="inline-flex min-h-11 items-center rounded px-3 text-sm font-medium text-blue-700 hover:underline focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:text-blue-300"
			href={resolve('/(app)/workspace/[workspaceId]/flow/[objectId]', {
				workspaceId,
				objectId: collectionId
			})}
			data-testid="flow-embedded-collection-open"
		>
			{$t('flow.canvas.embed.open')}
		</a>
	</header>
	{#if unsupported}
		<p class="p-4 text-sm text-slate-500">{$t('flow.canvas.unsupported')}</p>
	{:else if $viewState.status === 'error'}
		<p class="p-4 text-sm text-red-600" role="alert">
			{$t($viewState.errorKey ?? 'flow.collection.loadFailed')}
		</p>
	{:else if tableComponent && $viewState.collection}
		{@const Table = tableComponent}
		<div class="max-h-[28rem] overflow-auto">
			<Table {controller} snapshot={$viewState} {workspaceId} />
		</div>
	{:else}
		<p class="p-4 text-sm text-slate-500" aria-live="polite">{$t('common.loading')}</p>
	{/if}
</section>
