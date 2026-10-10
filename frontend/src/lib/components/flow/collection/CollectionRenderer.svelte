<script lang="ts">
	// Hosts one Collection on the object route: the view switcher (from the Collection's
	// `views[]`), the schema-mode switch (button + `?mode=schema`), the active view's
	// renderer and the record panel. The view and panel components come from the renderer
	// registry (`collection:table`, `collection:board`, `record:panel`) through their `load()`
	// dynamic imports. For a Record object (`recordId`) it shows that record's panel standalone.
	import { getContext, onDestroy, onMount, type Component } from 'svelte';
	import { t } from 'svelte-i18n';
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import type { FlowObjectRepository } from '$lib/flow/object-repository';
	import { FLOW_REPOSITORY_CONTEXT } from '$lib/flow/context-keys';
	import { FlowCommandService } from '$lib/flow/command-service';
	import { CollectionViewController, activeViewIn } from '$lib/flow/collection-view';
	import { resolveRenderer } from '$lib/flow/renderer-registry';
	import CollectionSchemaEditor from './CollectionSchemaEditor.svelte';

	interface Props {
		workspaceId: string;
		collectionId: string;
		/** Standalone record page: the record whose panel to show. */
		recordId?: string | null;
	}

	let { workspaceId, collectionId, recordId = null }: Props = $props();

	const repository = getContext<FlowObjectRepository>(FLOW_REPOSITORY_CONTEXT);
	// svelte-ignore state_referenced_locally
	const controller = new CollectionViewController(
		collectionId,
		repository,
		new FlowCommandService()
	);
	const viewState = controller.state;

	// eslint-disable-next-line @typescript-eslint/no-explicit-any
	type AnyComponent = Component<any>;
	let tableComponent = $state<AnyComponent | null>(null);
	let boardComponent = $state<AnyComponent | null>(null);
	let panelComponent = $state<AnyComponent | null>(null);
	let missingRecord = $state(false);

	const schemaMode = $derived($page.url.searchParams.get('mode') === 'schema');
	const activeView = $derived(activeViewIn($viewState));
	const activeKind = $derived(activeView?.kind ?? 'table');
	const viewComponent = $derived(activeKind === 'board' ? boardComponent : tableComponent);

	async function loadRenderer(kind: 'table' | 'board'): Promise<void> {
		const registration = resolveRenderer('collection', kind);
		if (!registration?.load) return;
		const component = await registration.load();
		if (kind === 'table') tableComponent = component;
		else boardComponent = component;
	}

	async function loadPanel(): Promise<void> {
		const registration = resolveRenderer('record', 'panel');
		if (registration?.load) panelComponent = await registration.load();
	}

	$effect(() => {
		controller.setMode(schemaMode ? 'schema' : 'entry');
	});

	$effect(() => {
		if (activeKind === 'board' && !boardComponent) void loadRenderer('board');
		if (activeKind === 'table' && !tableComponent) void loadRenderer('table');
	});

	$effect(() => {
		if ($viewState.panel && !panelComponent) void loadPanel();
	});

	onMount(async () => {
		await controller.load();
		if (recordId) {
			const row = await controller.findRecord(recordId);
			if (row) await controller.openPanel(recordId);
			else missingRecord = true;
		}
	});

	onDestroy(() => controller.dispose());

	async function toggleSchemaMode(): Promise<void> {
		const url = new URL($page.url);
		if (schemaMode) url.searchParams.delete('mode');
		else url.searchParams.set('mode', 'schema');
		// eslint-disable-next-line svelte/no-navigation-without-resolve -- same route, query only
		await goto(`${url.pathname}${url.search}`, {
			replaceState: true,
			keepFocus: true,
			noScroll: true
		});
	}

	function onTabKeydown(event: KeyboardEvent, index: number): void {
		const views = $viewState.collection?.views ?? [];
		let next = -1;
		if (event.key === 'ArrowRight') next = (index + 1) % views.length;
		else if (event.key === 'ArrowLeft') next = (index - 1 + views.length) % views.length;
		else if (event.key === 'Home') next = 0;
		else if (event.key === 'End') next = views.length - 1;
		if (next < 0) return;
		event.preventDefault();
		const view = views[next];
		if (!view) return;
		void controller.setActiveView(view.viewId);
		queueMicrotask(() => document.getElementById(`collection-view-tab-${view.viewId}`)?.focus());
	}
</script>

{#if $viewState.status === 'error'}
	<div class="p-8 text-center text-sm text-red-600" role="alert">
		{$t($viewState.errorKey ?? 'flow.collection.loadFailed')}
	</div>
{:else if $viewState.status === 'loading' && !$viewState.collection}
	<div class="p-8 text-center text-sm text-slate-500" aria-live="polite">
		{$t('common.loading')}
	</div>
{:else if recordId}
	{#if missingRecord}
		<p class="p-8 text-center text-sm text-slate-500" role="alert">{$t('flow.record.notFound')}</p>
	{:else if panelComponent}
		{@const Panel = panelComponent}
		<Panel {controller} snapshot={$viewState} {workspaceId} standalone />
	{/if}
{:else}
	<div
		class="flex min-w-0 flex-1 flex-col lg:flex-row"
		data-testid="collection-renderer"
		data-mode={$viewState.mode}
	>
		<div class="flex min-w-0 flex-1 flex-col">
			<div
				class="flex flex-wrap items-center gap-2 border-b border-slate-200 px-4 py-2 dark:border-slate-800"
			>
				<div
					role="tablist"
					aria-label={$t('flow.collection.view.tabs')}
					class="flex flex-wrap gap-1"
				>
					{#each $viewState.collection?.views ?? [] as view, index (view.viewId)}
						<button
							type="button"
							role="tab"
							id={`collection-view-tab-${view.viewId}`}
							aria-selected={view.viewId === $viewState.activeViewId}
							tabindex={view.viewId === $viewState.activeViewId ? 0 : -1}
							class="min-h-11 rounded px-3 text-sm font-medium focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 {view.viewId ===
							$viewState.activeViewId
								? 'bg-slate-900 text-white dark:bg-slate-100 dark:text-slate-900'
								: 'text-slate-600 hover:bg-slate-100 dark:text-slate-300 dark:hover:bg-slate-800'}"
							data-view-kind={view.kind ?? ''}
							data-testid="collection-view-tab"
							onclick={() => void controller.setActiveView(view.viewId)}
							onkeydown={(event) => onTabKeydown(event, index)}
						>
							{view.name}
							<span class="sr-only">
								({view.kind === 'board'
									? $t('flow.collection.view.board')
									: $t('flow.collection.view.table')})
							</span>
						</button>
					{:else}
						<span class="text-xs text-slate-500">{$t('flow.collection.view.none')}</span>
					{/each}
				</div>
				<div class="ml-auto flex items-center gap-2">
					<button
						type="button"
						class="min-h-11 rounded border px-3 text-sm font-medium focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 {schemaMode
							? 'border-amber-500 bg-amber-100 text-amber-900 dark:bg-amber-900 dark:text-amber-100'
							: 'border-slate-300 text-slate-700 dark:border-slate-600 dark:text-slate-200'}"
						aria-pressed={schemaMode}
						data-testid="collection-schema-toggle"
						onclick={() => void toggleSchemaMode()}
					>
						{schemaMode ? $t('flow.collection.schema.exit') : $t('flow.collection.schema.enter')}
					</button>
				</div>
			</div>

			{#if $viewState.collection?.fieldSecrecyEnabled}
				<p
					class="border-b border-slate-200 px-4 py-2 text-xs text-slate-500 dark:border-slate-800"
					role="note"
				>
					{$t('flow.collection.secrecyNotice')}
				</p>
			{/if}

			{#if $viewState.notice}
				<div
					class="flex items-center gap-3 border-b border-amber-200 bg-amber-50 px-4 py-2 text-sm text-amber-900 dark:border-amber-900 dark:bg-amber-950 dark:text-amber-100"
					role="status"
					data-notice-code={$viewState.notice.code ?? ''}
				>
					<span class="flex-1">{$t($viewState.notice.key)}</span>
					<button
						type="button"
						class="min-h-11 px-2 font-medium underline"
						onclick={() => controller.dismissNotice()}>{$t('flow.recovery.dismiss')}</button
					>
				</div>
			{/if}

			{#if $viewState.mode === 'schema'}
				<CollectionSchemaEditor {controller} snapshot={$viewState} />
			{/if}

			{#if viewComponent}
				{@const View = viewComponent}
				<View {controller} snapshot={$viewState} {workspaceId} />
			{:else}
				<p class="p-4 text-sm text-slate-500" aria-live="polite">{$t('common.loading')}</p>
			{/if}
		</div>

		{#if $viewState.panel && panelComponent}
			{@const Panel = panelComponent}
			<Panel {controller} snapshot={$viewState} {workspaceId} />
		{/if}
	</div>
{/if}
