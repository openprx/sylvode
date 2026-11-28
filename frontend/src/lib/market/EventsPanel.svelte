<svelte:options runes />

<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { locale, t } from 'svelte-i18n';
	import { describeResult } from '#lib/i18n/describe.js';
	import { toast } from '#lib/stores/toast.js';
	import Pagination from '#lib/ui/pagination/Pagination.svelte';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import StatusChip from '#lib/ui/status-chip/StatusChip.svelte';
	import { indexApi, type IndexEvent } from './index-api';
	import { shortDate } from './index-logic';

	/**
	 * The audit log of the public index for this workspace (imports, refusals, trusted keys,
	 * revocations applied, refresh failures), newest first. For owners and administrators.
	 */
	let { workspaceId }: { workspaceId: string } = $props();

	const PAGE_SIZE = 20;
	let page = $state(1);
	let total = $state(0);
	let rows = $state.raw<IndexEvent[]>([]);
	let loading = $state(true);
	let failed = $state(false);

	async function load() {
		loading = true;
		const result = await indexApi.events(workspaceId, page);
		loading = false;
		failed = result.code !== 0;
		if (failed) {
			toast.error(describeResult(result, get(t)('market.index.eventsFailed')));
			return;
		}
		rows = result.data?.items ?? [];
		total = result.data?.total ?? 0;
	}

	onMount(() => void load());

	function actionName(action: string): string {
		const key = `market.index.action.${action}`;
		const text = get(t)(key);
		return text === key ? action : text;
	}

	function tone(action: string): 'neutral' | 'success' | 'warning' | 'danger' {
		if (action === 'import' || action === 'trust_key') return 'success';
		if (action === 'import_refused' || action === 'refresh_failed') return 'warning';
		if (action.endsWith('_sync')) return 'danger';
		return 'neutral';
	}

	/** The facts of an event worth a line: the plugin, version, key and reason when present. */
	function summary(details: IndexEvent['details']): string {
		if (!details) return '';
		const parts: string[] = [];
		for (const field of [
			'plugin_id',
			'version',
			'publisher',
			'key_id',
			'reason',
			'error',
			'code'
		]) {
			const value = details[field];
			if (typeof value === 'string' || typeof value === 'number') parts.push(String(value));
		}
		return parts.join(' · ');
	}
</script>

<section class="panel" aria-label={$t('market.index.events')} data-market-index-events>
	{#if loading && rows.length === 0}
		<StateBlock kind="loading" />
	{:else if failed}
		<StateBlock
			kind="loadFailed"
			thing={$t('market.index.events')}
			action={{ run: () => void load() }}
		/>
	{:else if rows.length === 0}
		<p class="empty">{$t('market.index.eventsEmpty')}</p>
	{:else}
		<table>
			<thead>
				<tr>
					<th>{$t('market.index.colWhen')}</th>
					<th>{$t('market.index.colAction')}</th>
					<th>{$t('market.index.colSource')}</th>
					<th>{$t('market.index.colDetails')}</th>
				</tr>
			</thead>
			<tbody>
				{#each rows as row (row.id)}
					<tr>
						<td class="when">{shortDate(row.created_at, $locale ?? 'en')}</td>
						<td><StatusChip tone={tone(row.action)}>{actionName(row.action)}</StatusChip></td>
						<td>{row.source}</td>
						<td class="details">{summary(row.details)}</td>
					</tr>
				{/each}
			</tbody>
		</table>
		<Pagination
			{page}
			pageSize={PAGE_SIZE}
			{total}
			{loading}
			pageSizes={[PAGE_SIZE]}
			onpage={(next) => {
				page = next;
				void load();
			}}
			label={$t('market.index.events')}
		/>
	{/if}
</section>

<style>
	.panel {
		border: var(--rf-border-width) solid var(--rf-line);
		border-radius: var(--rf-radius-card);
		padding: var(--rf-space-5);
		font-family: var(--rf-font-sans-stack);
		overflow-x: auto;
	}
	table {
		width: 100%;
		border-collapse: collapse;
		font-size: var(--rf-size-secondary);
	}
	th {
		text-align: left;
		font-weight: var(--rf-weight-medium);
		font-size: var(--rf-size-meta);
		color: var(--rf-secondary);
		padding: var(--rf-space-3) var(--rf-space-4);
	}
	td {
		padding: var(--rf-space-3) var(--rf-space-4);
		border-top: var(--rf-border-width) solid var(--rf-line);
		vertical-align: top;
	}
	.when {
		white-space: nowrap;
		font-variant-numeric: tabular-nums;
	}
	.details {
		overflow-wrap: anywhere;
		color: var(--rf-secondary);
	}
	.empty {
		margin: 0;
		color: var(--rf-secondary);
	}
</style>
