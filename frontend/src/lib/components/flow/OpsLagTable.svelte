<script lang="ts">
	// Workspace lag: the admin aggregate (`GET /admin/workspaces/{id}/flow/lag`) for
	// projection/search/fan-out max+p95, and the policy-filtered `GET .../flow/projection-lag`
	// for the lag badge and the per-object rows (the admin endpoint returns no rows at this
	// baseline). Paging state lives in `ProjectionLagPager`.
	import { t } from 'svelte-i18n';
	import type { FlowAdminLag, FlowProjectionLag } from '$lib/api/flow';

	interface Props {
		lag: FlowAdminLag | null;
		lagErrorKey: string;
		projection: FlowProjectionLag | null;
		projectionErrorKey: string;
		pageNumber: number;
		hasPrevious: boolean;
		hasNext: boolean;
		busy: boolean;
		onprevious: () => void;
		onnext: () => void;
	}

	let {
		lag,
		lagErrorKey,
		projection,
		projectionErrorKey,
		pageNumber,
		hasPrevious,
		hasNext,
		busy,
		onprevious,
		onnext
	}: Props = $props();

	const groups = $derived(
		lag
			? [
					{ id: 'projection', label: 'flow.operations.lag.projection', group: lag.projection },
					{ id: 'search', label: 'flow.operations.lag.search', group: lag.search },
					{ id: 'fanout', label: 'flow.operations.lag.fanout', group: lag.fanout }
				]
			: []
	);
</script>

<section
	class="rounded-lg border border-slate-200 bg-white p-4 shadow-sm sm:p-6 dark:border-slate-700 dark:bg-slate-900"
	aria-labelledby="flow-ops-lag-heading"
	data-testid="flow-ops-lag"
>
	<div class="flex flex-wrap items-center gap-3">
		<h2 id="flow-ops-lag-heading" class="text-lg font-semibold text-slate-900 dark:text-slate-100">
			{$t('flow.operations.lag.heading')}
		</h2>
		{#if projection}
			<span
				class={`rounded-full px-3 py-1 text-xs font-semibold ${projection.max_lag > 0 ? 'bg-amber-100 text-amber-900 dark:bg-amber-950/50 dark:text-amber-200' : 'bg-emerald-100 text-emerald-800 dark:bg-emerald-950/50 dark:text-emerald-300'}`}
				data-testid="flow-ops-lag-badge"
			>
				{projection.max_lag > 0
					? $t('flow.operations.lag.badge', { values: { n: projection.max_lag } })
					: $t('flow.operations.lag.badgeOk')}
			</span>
		{/if}
	</div>

	{#if lagErrorKey}
		<p class="mt-3 text-sm text-red-700 dark:text-red-300" role="alert">
			{$t('flow.operations.lag.loadFailed')}
			{$t(lagErrorKey)}
		</p>
	{/if}

	{#if lag}
		<dl class="mt-4 grid grid-cols-1 gap-4 sm:grid-cols-3" data-testid="flow-ops-lag-groups">
			{#each groups as item (item.id)}
				<div class="rounded-md border border-slate-200 p-3 dark:border-slate-700">
					<dt class="text-sm font-medium text-slate-700 dark:text-slate-300">{$t(item.label)}</dt>
					<dd class="mt-2 grid grid-cols-2 gap-1 text-sm">
						<span class="text-slate-500 dark:text-slate-400">{$t('flow.operations.lag.max')}</span>
						<span class="font-mono" data-testid={`flow-ops-lag-${item.id}-max`}
							>{item.group.max}</span
						>
						<span class="text-slate-500 dark:text-slate-400">{$t('flow.operations.lag.p95')}</span>
						<span class="font-mono">{item.group.p95}</span>
					</dd>
				</div>
			{/each}
		</dl>
	{/if}

	<h3 class="mt-6 text-sm font-semibold text-slate-900 dark:text-slate-100">
		{$t('flow.operations.lag.itemsHeading')}
	</h3>
	{#if projectionErrorKey}
		<p class="mt-2 text-sm text-red-700 dark:text-red-300" role="alert">{$t(projectionErrorKey)}</p>
	{/if}
	{#if projection}
		{#if projection.items.length === 0}
			<p class="mt-2 text-sm text-slate-500 dark:text-slate-400">
				{$t('flow.operations.lag.empty')}
			</p>
		{:else}
			<table class="mt-2 hidden w-full text-left text-sm md:table" data-testid="flow-ops-lag-items">
				<thead class="text-xs uppercase text-slate-500 dark:text-slate-400">
					<tr>
						<th scope="col" class="py-2 pr-4">{$t('flow.operations.lag.objectId')}</th>
						<th scope="col" class="py-2 pr-4">{$t('flow.operations.lag.headSeq')}</th>
						<th scope="col" class="py-2 pr-4">{$t('flow.operations.lag.projectionSeq')}</th>
						<th scope="col" class="py-2">{$t('flow.operations.lag.lagValue')}</th>
					</tr>
				</thead>
				<tbody>
					{#each projection.items as item (item.object_id)}
						<tr class="border-t border-slate-100 dark:border-slate-800">
							<td class="break-all py-2 pr-4 font-mono text-xs">{item.object_id}</td>
							<td class="py-2 pr-4 font-mono">{item.head_seq}</td>
							<td class="py-2 pr-4 font-mono">{item.projection_seq}</td>
							<td class="py-2 font-mono">{item.lag}</td>
						</tr>
					{/each}
				</tbody>
			</table>
			<ul class="mt-2 space-y-2 md:hidden">
				{#each projection.items as item (item.object_id)}
					<li class="rounded-md border border-slate-200 p-3 text-sm dark:border-slate-700">
						<p class="break-all font-mono text-xs">{item.object_id}</p>
						<p class="mt-1">
							{$t('flow.operations.lag.headSeq')}: <span class="font-mono">{item.head_seq}</span> ·
							{$t('flow.operations.lag.projectionSeq')}:
							<span class="font-mono">{item.projection_seq}</span>
							·
							{$t('flow.operations.lag.lagValue')}: <span class="font-mono">{item.lag}</span>
						</p>
					</li>
				{/each}
			</ul>
		{/if}
		<nav class="mt-3 flex items-center gap-2" aria-label={$t('flow.operations.lag.itemsHeading')}>
			<button
				type="button"
				class="min-h-11 rounded-md border border-slate-300 px-4 text-sm focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 disabled:opacity-50 dark:border-slate-600"
				disabled={busy || !hasPrevious}
				onclick={onprevious}
			>
				{$t('flow.operations.lag.previous')}
			</button>
			<span class="text-sm text-slate-600 dark:text-slate-300">
				{$t('flow.operations.lag.page', { values: { n: pageNumber } })}
			</span>
			<button
				type="button"
				class="min-h-11 rounded-md border border-slate-300 px-4 text-sm focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 disabled:opacity-50 dark:border-slate-600"
				disabled={busy || !hasNext}
				onclick={onnext}
			>
				{$t('flow.operations.lag.next')}
			</button>
		</nav>
	{/if}
</section>
