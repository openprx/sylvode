<script lang="ts">
	import { describeResult } from '#lib/i18n/describe.js';
	import { Activity, RefreshCw } from '@lucide/svelte';
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import {
		operationLogsApi,
		type BotOperationLog,
		type BotOperationOutcome
	} from '#lib/api/operation-logs.js';
	import Button from '#lib/ui/button/Button.svelte';
	import Input from '#lib/components/Input.svelte';
	import { toast } from '#lib/stores/toast.js';
	import { requireRouteParam } from '#lib/utils/route-params.js';
	import { locale, t } from 'svelte-i18n';

	const workspaceId = requireRouteParam(page.params.workspaceId, 'workspaceId');
	const pageSize = 50;

	let logs = $state<BotOperationLog[]>([]);
	let loading = $state(true);
	let botIdFilter = $state('');
	let toolNameFilter = $state('');
	let outcomeFilter = $state<BotOperationOutcome | ''>('');
	let cursorHistory = $state<Array<string | null>>([null]);
	let pageIndex = $state(0);
	let nextCursor = $state<string | null>(null);

	onMount(() => {
		void loadLogs();
	});

	async function loadLogs(cursor: string | null = cursorHistory[pageIndex] ?? null) {
		loading = true;
		const response = await operationLogsApi.list(workspaceId, {
			bot_id: botIdFilter.trim() || undefined,
			tool_name: toolNameFilter.trim() || undefined,
			outcome: outcomeFilter || undefined,
			cursor: cursor || undefined,
			limit: pageSize
		});
		if (response.code !== 0 || !response.data) {
			logs = [];
			nextCursor = null;
			toast.error(describeResult(response));
		} else {
			logs = response.data.items;
			nextCursor = response.data.next_cursor;
		}
		loading = false;
	}

	function applyFilters() {
		cursorHistory = [null];
		pageIndex = 0;
		void loadLogs(null);
	}

	function resetFilters() {
		botIdFilter = '';
		toolNameFilter = '';
		outcomeFilter = '';
		applyFilters();
	}

	function previousPage() {
		if (pageIndex === 0) return;
		pageIndex -= 1;
		void loadLogs(cursorHistory[pageIndex] ?? null);
	}

	function nextPage() {
		if (!nextCursor) return;
		cursorHistory = [...cursorHistory.slice(0, pageIndex + 1), nextCursor];
		pageIndex += 1;
		void loadLogs(nextCursor);
	}

	function formatDate(value: string): string {
		return new Date(value).toLocaleString($locale === 'en' ? 'en-US' : 'zh-CN');
	}

	function botLabel(log: BotOperationLog): string {
		return log.bot_name || log.bot_id.slice(0, 8);
	}
</script>

<div class="mx-auto max-w-7xl space-y-6">
	<header class="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
		<div>
			<div class="flex items-center gap-2">
				<Activity class="h-6 w-6 text-fg-muted" aria-hidden="true" />
				<h1 class="text-2xl font-semibold text-fg">
					{$t('connections.title')}
				</h1>
			</div>
			<p class="mt-1 text-sm text-fg-muted">{$t('connections.subtitle')}</p>
		</div>
		<Button variant="secondary" onclick={() => loadLogs()} disabled={loading}>
			<RefreshCw class="mr-2 h-4 w-4" aria-hidden="true" />
			{$t('connections.refresh')}
		</Button>
	</header>

	<section class="rounded-card border border-line bg-raised p-4">
		<div class="grid gap-3 md:grid-cols-[1fr_1fr_180px_auto] md:items-end">
			<div>
				<label for="operation-bot-filter" class="mb-1 block text-sm font-medium text-fg"
					>{$t('connections.bot')}</label
				>
				<Input
					id="operation-bot-filter"
					bind:value={botIdFilter}
					placeholder={$t('connections.botFilterPlaceholder')}
				/>
			</div>
			<div>
				<label for="operation-tool-filter" class="mb-1 block text-sm font-medium text-fg"
					>{$t('connections.tool')}</label
				>
				<Input
					id="operation-tool-filter"
					bind:value={toolNameFilter}
					placeholder="form_records.list"
				/>
			</div>
			<div>
				<label for="operation-outcome-filter" class="mb-1 block text-sm font-medium text-fg"
					>{$t('connections.result')}</label
				>
				<select
					id="operation-outcome-filter"
					bind:value={outcomeFilter}
					class="h-10 w-full rounded-control border border-input bg-field px-3 text-sm text-fg"
				>
					<option value="">{$t('connections.allResults')}</option>
					<option value="ok">{$t('connections.resultOk')}</option>
					<option value="error">{$t('connections.resultError')}</option>
				</select>
			</div>
			<div class="flex gap-2">
				<Button variant="primary" onclick={applyFilters} disabled={loading}
					>{$t('connections.applyFilters')}</Button
				>
				<Button variant="secondary" onclick={resetFilters} disabled={loading}
					>{$t('connections.resetFilters')}</Button
				>
			</div>
		</div>
	</section>

	<section class="overflow-hidden rounded-card border border-line bg-raised">
		{#if loading}
			<div class="p-8 text-center text-sm text-fg-muted">
				{$t('common.loading')}
			</div>
		{:else if logs.length === 0}
			<div class="p-8 text-center text-sm text-fg-muted">
				{$t('connections.noOperationLogs')}
			</div>
		{:else}
			<div class="overflow-x-auto">
				<table class="min-w-full divide-y divide-line text-sm">
					<thead
						class="bg-surface text-left text-xs font-semibold uppercase tracking-wide text-fg-muted"
					>
						<tr>
							<th class="px-4 py-3">{$t('connections.time')}</th>
							<th class="px-4 py-3">{$t('connections.bot')}</th>
							<th class="px-4 py-3">{$t('connections.tool')}</th>
							<th class="px-4 py-3">{$t('connections.surface')}</th>
							<th class="px-4 py-3">{$t('connections.result')}</th>
							<th class="px-4 py-3 text-right">{$t('connections.duration')}</th>
						</tr>
					</thead>
					<tbody class="divide-y divide-line">
						{#each logs as log (log.id)}
							<tr class="align-top hover:bg-surface">
								<td class="whitespace-nowrap px-4 py-3 text-fg-muted"
									>{formatDate(log.created_at)}</td
								>
								<td class="px-4 py-3">
									<div class="font-medium text-fg">{botLabel(log)}</div>
									<div class="font-mono text-xs text-fg-muted">{log.bot_id.slice(0, 8)}</div>
								</td>
								<td class="px-4 py-3">
									<div class="font-mono text-xs font-medium text-fg">
										{log.tool_name || $t('connections.directRest')}
									</div>
									<div class="mt-1 max-w-md truncate font-mono text-xs text-fg-muted">
										{log.method}
										{log.path}
									</div>
								</td>

								<td class="px-4 py-3">
									<span class="rounded bg-surface px-2 py-1 font-mono text-xs text-fg"
										>{log.surface}</span
									>
								</td>

								<td class="px-4 py-3">
									<span
										class="inline-flex rounded-full px-2 py-0.5 text-xs font-medium ring-1 {log.outcome ===
										'ok'
											? 'bg-success-soft text-success'
											: 'bg-danger-soft text-danger'}"
									>
										{log.outcome === 'ok'
											? $t('connections.resultOk')
											: $t('connections.resultError')}
									</span>
									{#if log.business_code !== 0}<div class="mt-1 text-xs text-fg-muted">
											{log.business_code} · {log.error_message || '-'}
										</div>{/if}
								</td>
								<td class="whitespace-nowrap px-4 py-3 text-right tabular-nums text-fg"
									>{log.duration_ms} ms</td
								>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
	</section>

	<div class="flex items-center justify-between">
		<span class="text-sm text-fg-muted"
			>{$t('connections.page', { values: { page: pageIndex + 1 } })}</span
		>
		<div class="flex gap-2">
			<Button variant="secondary" onclick={previousPage} disabled={loading || pageIndex === 0}
				>{$t('connections.previous')}</Button
			>
			<Button variant="secondary" onclick={nextPage} disabled={loading || !nextCursor}
				>{$t('connections.next')}</Button
			>
		</div>
	</div>
</div>
