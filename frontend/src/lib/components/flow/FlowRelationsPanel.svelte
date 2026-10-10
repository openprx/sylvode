<script lang="ts">
	// Relations panel (`ui-surface-v1.md` "后续版本 UI 派生" v0.5). Rows, paging, filtering and the
	// fail-closed integrity rule live in `FlowRelationsController` (`lib/flow/relations-panel.ts`).
	// An unavailable row renders placeholder copy only: no link and no copy action.
	import { t } from 'svelte-i18n';
	import { resolve } from '$app/paths';
	import {
		FlowRelationsController,
		RELATION_DIRECTIONS,
		RELATION_DIRECTION_KEYS,
		relationTarget,
		type RelationsSnapshot
	} from '$lib/flow/relations-panel';
	import type { FlowRelationDirection } from '$lib/api/flow';

	interface Props {
		objectId: string;
		workspaceId: string;
	}

	let { objectId, workspaceId }: Props = $props();

	let snapshot = $state<RelationsSnapshot | null>(null);
	let controller: FlowRelationsController | null = null;

	$effect(() => {
		const next = new FlowRelationsController(objectId, workspaceId, (value) => (snapshot = value));
		controller = next;
		void next.load();
		return () => {
			if (controller === next) controller = null;
		};
	});

	function setDirection(event: Event): void {
		if (!controller || !snapshot) return;
		const direction = (event.currentTarget as HTMLSelectElement).value as FlowRelationDirection;
		void controller.setFilter({ ...snapshot.filter, direction });
	}

	function setType(event: Event): void {
		if (!controller || !snapshot) return;
		const relationType = (event.currentTarget as HTMLSelectElement).value;
		void controller.setFilter({ ...snapshot.filter, relationType });
	}

	const selectClass =
		'min-h-11 w-full rounded-md border border-slate-300 bg-white px-2 text-sm text-slate-900 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100';
	const linkButtonClass =
		'min-h-11 rounded-md px-2 text-xs font-medium text-blue-600 hover:underline focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 disabled:opacity-50 dark:text-blue-400';
</script>

<div class="space-y-3 text-sm" data-testid="flow-relations-panel">
	{#if snapshot}
		<div class="grid grid-cols-2 gap-2">
			<label class="block">
				<span class="mb-1 block text-xs text-slate-500 dark:text-slate-400"
					>{$t('flow.relations.direction.label')}</span
				>
				<select
					aria-label={$t('flow.relations.direction.label')}
					class={selectClass}
					value={snapshot.filter.direction}
					onchange={setDirection}
				>
					{#each RELATION_DIRECTIONS as direction (direction)}
						<option value={direction}>{$t(RELATION_DIRECTION_KEYS[direction])}</option>
					{/each}
				</select>
			</label>
			<label class="block">
				<span class="mb-1 block text-xs text-slate-500 dark:text-slate-400"
					>{$t('flow.relations.type.label')}</span
				>
				<select
					aria-label={$t('flow.relations.type.label')}
					class={selectClass}
					value={snapshot.filter.relationType}
					onchange={setType}
				>
					<option value="">{$t('flow.relations.type.all')}</option>
					{#each snapshot.relationTypes as relationType (relationType)}
						<option value={relationType}>{relationType}</option>
					{/each}
				</select>
			</label>
		</div>

		<div aria-live="polite">
			{#if snapshot.failureKey}
				<p class="text-red-600 dark:text-red-400" role="alert" data-testid="flow-relations-error">
					{$t(snapshot.failureKey)}
				</p>
				<button type="button" class={linkButtonClass} onclick={() => controller?.reload()}>
					{$t('flow.relations.retry')}
				</button>
			{:else if snapshot.loading && snapshot.rows.length === 0}
				<p class="text-slate-500 dark:text-slate-400">{$t('flow.relations.loading')}</p>
			{:else if snapshot.loaded && snapshot.rows.length === 0}
				<p class="text-slate-500 dark:text-slate-400">{$t('flow.relations.empty')}</p>
			{/if}
		</div>

		{#if !snapshot.failureKey && snapshot.rows.length > 0}
			<ul class="space-y-2" data-testid="flow-relations-list">
				{#each snapshot.rows as row, index (index)}
					{@const target = relationTarget(row)}
					{#if row.kind === 'visible' && target}
						<li
							class="border-l-2 border-slate-200 pl-3 dark:border-slate-700"
							data-testid="flow-relation-row"
						>
							<p class="text-xs text-slate-400 dark:text-slate-500">
								{row.direction === 'outgoing'
									? $t('flow.relations.row.outgoing')
									: $t('flow.relations.row.incoming')}
								· {row.relationType}
							</p>
							<a
								href={resolve('/(app)/workspace/[workspaceId]/flow/[objectId]', {
									workspaceId,
									objectId: target
								})}
								class="inline-flex min-h-11 items-center font-medium text-blue-600 hover:underline focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:text-blue-400"
							>
								{row.title}
							</a>
						</li>
					{:else}
						<li
							class="border-l-2 border-dashed border-slate-200 pl-3 text-slate-500 dark:border-slate-700 dark:text-slate-400"
							data-testid="flow-relation-unavailable"
						>
							{$t('flow.relations.unavailable')}
						</li>
					{/if}
				{/each}
			</ul>
			{#if snapshot.hasMore}
				<button
					type="button"
					class={linkButtonClass}
					disabled={snapshot.loading}
					onclick={() => controller?.loadMore()}
				>
					{$t('flow.relations.loadMore')}
				</button>
			{/if}
		{/if}
	{/if}
</div>
