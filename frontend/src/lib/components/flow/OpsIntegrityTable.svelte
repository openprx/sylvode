<script lang="ts">
	// Workspace integrity (`GET /admin/workspaces/{id}/flow/integrity?scope=documents`) with a
	// per-row entry into the verify / compact / rebuild drawer, plus manual id entry. `counts` are
	// workspace-wide (`total` included); `documents[]` is the first page (the panel offers a
	// limit), with failed documents listed and worded by their `failure` code.
	import { t } from 'svelte-i18n';
	import type { FlowAdminIntegrity } from '$lib/api/flow';
	import {
		INTEGRITY_STATUS_KEYS,
		OPS_INTEGRITY_LIMITS,
		integrityRows,
		isUuid,
		statusKey,
		type MaintenanceKind,
		type OpsIntegrityLimit
	} from '$lib/flow/operations-service';

	interface Props {
		integrity: FlowAdminIntegrity | null;
		errorKey: string;
		limit: OpsIntegrityLimit;
		busy: boolean;
		onlimit: (limit: OpsIntegrityLimit) => void;
		onoperate: (kind: MaintenanceKind, targetId: string, expectedHead: number | null) => void;
	}

	let { integrity, errorKey, limit, busy, onlimit, onoperate }: Props = $props();

	let manualDocument = $state('');
	let manualObject = $state('');
	const documentValid = $derived(isUuid(manualDocument));
	const objectValid = $derived(isUuid(manualObject));
	const rows = $derived(integrityRows(integrity));

	function changeLimit(event: Event) {
		const value = Number((event.currentTarget as HTMLSelectElement).value);
		const match = OPS_INTEGRITY_LIMITS.find((candidate) => candidate === value);
		if (match) onlimit(match);
	}

	const actionClass =
		'min-h-11 rounded-md border px-3 text-sm font-medium focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 disabled:opacity-50';
</script>

<section
	class="rounded-lg border border-slate-200 bg-white p-4 shadow-sm sm:p-6 dark:border-slate-700 dark:bg-slate-900"
	aria-labelledby="flow-ops-integrity-heading"
	data-testid="flow-ops-integrity"
>
	<div class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
		<div class="flex flex-wrap items-center gap-3">
			<h2
				id="flow-ops-integrity-heading"
				class="text-lg font-semibold text-slate-900 dark:text-slate-100"
			>
				{$t('flow.operations.integrity.heading')}
			</h2>
			{#if integrity}
				<span
					class={`rounded-full px-3 py-1 text-xs font-semibold ${integrity.status === 'healthy' ? 'bg-emerald-100 text-emerald-800 dark:bg-emerald-950/50 dark:text-emerald-300' : 'bg-red-100 text-red-800 dark:bg-red-950/50 dark:text-red-300'}`}
					data-testid="flow-ops-integrity-status"
				>
					{$t(statusKey(INTEGRITY_STATUS_KEYS, integrity.status))}
				</span>
			{/if}
		</div>
		<label class="flex items-center gap-2 text-sm text-slate-700 dark:text-slate-300">
			<span>{$t('flow.operations.integrity.limit')}</span>
			<select
				class="min-h-11 rounded-md border border-slate-300 bg-white px-3 text-sm focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:border-slate-600 dark:bg-slate-800"
				value={String(limit)}
				disabled={busy}
				onchange={changeLimit}
			>
				{#each OPS_INTEGRITY_LIMITS as option (option)}
					<option value={String(option)}>{option}</option>
				{/each}
			</select>
		</label>
	</div>

	{#if errorKey}
		<p class="mt-3 text-sm text-red-700 dark:text-red-300" role="alert">
			{$t('flow.operations.integrity.loadFailed')}
			{$t(errorKey)}
		</p>
	{/if}

	{#if integrity}
		<dl
			class="mt-4 grid grid-cols-2 gap-4 text-sm sm:grid-cols-4"
			data-testid="flow-ops-integrity-counts"
		>
			<div>
				<dt class="text-slate-500 dark:text-slate-400">
					{$t('flow.operations.integrity.total')}
				</dt>
				<dd class="mt-1 font-mono" data-testid="flow-ops-integrity-total">
					{integrity.counts.total}
				</dd>
			</div>
			<div>
				<dt class="text-slate-500 dark:text-slate-400">
					{$t('flow.operations.integrity.checked')}
				</dt>
				<dd class="mt-1 font-mono" data-testid="flow-ops-integrity-checked">
					{integrity.counts.checked}
				</dd>
			</div>
			<div>
				<dt class="text-slate-500 dark:text-slate-400">
					{$t('flow.operations.integrity.healthy')}
				</dt>
				<dd class="mt-1 font-mono">{integrity.counts.healthy}</dd>
			</div>
			<div>
				<dt class="text-slate-500 dark:text-slate-400">{$t('flow.operations.integrity.failed')}</dt>
				<dd
					class={`mt-1 font-mono ${integrity.counts.failed > 0 ? 'text-red-700 dark:text-red-300' : ''}`}
					data-testid="flow-ops-integrity-failed"
				>
					{integrity.counts.failed}
				</dd>
			</div>
		</dl>
		<p class="mt-2 text-xs text-slate-500 dark:text-slate-400">
			{$t('flow.operations.integrity.checkedAt', {
				values: { time: new Date(integrity.checked_at).toLocaleString() }
			})}
			{$t('flow.operations.integrity.limitNote', { values: { n: limit } })}
		</p>

		{#if rows.length === 0}
			<p class="mt-4 text-sm text-slate-500 dark:text-slate-400">
				{$t('flow.operations.integrity.empty')}
			</p>
		{:else}
			<table
				class="mt-4 hidden w-full text-left text-sm md:table"
				data-testid="flow-ops-integrity-table"
			>
				<thead class="text-xs uppercase text-slate-500 dark:text-slate-400">
					<tr>
						<th scope="col" class="py-2 pr-3">{$t('flow.operations.integrity.documentId')}</th>
						<th scope="col" class="py-2 pr-3">{$t('flow.operations.integrity.rowStatus')}</th>
						<th scope="col" class="py-2 pr-3">{$t('flow.operations.integrity.headSeq')}</th>
						<th scope="col" class="py-2 pr-3">{$t('flow.operations.integrity.projectionSeq')}</th>
						<th scope="col" class="py-2 pr-3">{$t('flow.operations.integrity.semanticHash')}</th>
						<th scope="col" class="py-2">{$t('flow.operations.integrity.actions')}</th>
					</tr>
				</thead>
				<tbody>
					{#each rows as row (row.documentId)}
						{@const head = row.status === 'ok' ? row.headSeq : null}
						<tr
							class="border-t border-slate-100 align-top dark:border-slate-800"
							data-document-id={row.documentId}
							data-status={row.status}
						>
							<td class="py-2 pr-3">
								<p class="break-all font-mono text-xs">{row.documentId}</p>
								<p class="mt-1 break-all font-mono text-xs text-slate-500 dark:text-slate-400">
									{$t('flow.operations.integrity.objectId')}: {row.objectId}
								</p>
							</td>
							<td class="py-2 pr-3" data-testid="flow-ops-integrity-row-status">
								{#if row.status === 'ok'}
									<span class="text-emerald-700 dark:text-emerald-300"
										>{$t('flow.operations.integrity.rowOk')}</span
									>
								{:else}
									<p class="text-red-700 dark:text-red-300">{$t(row.failureKey)}</p>
									<p class="mt-1 text-xs text-slate-500 dark:text-slate-400">{$t(row.errorKey)}</p>
								{/if}
							</td>
							{#if row.status === 'ok'}
								<td class="py-2 pr-3 font-mono">{row.headSeq}</td>
								<td class="py-2 pr-3 font-mono">{row.projectionSeq}</td>
								<td
									class="max-w-[10rem] truncate py-2 pr-3 font-mono text-xs"
									title={row.semanticHash}>{row.semanticHash}</td
								>
							{:else}
								<td class="py-2 pr-3 font-mono">-</td>
								<td class="py-2 pr-3 font-mono">-</td>
								<td class="py-2 pr-3 font-mono">-</td>
							{/if}
							<td class="py-2">
								<div class="flex flex-wrap gap-2">
									<button
										type="button"
										class={`${actionClass} border-slate-300 dark:border-slate-600`}
										disabled={busy}
										onclick={() => onoperate('verify', row.documentId, head)}
									>
										{$t('flow.operations.integrity.verify')}
									</button>
									<button
										type="button"
										class={`${actionClass} border-amber-400 text-amber-900 dark:border-amber-700 dark:text-amber-200`}
										disabled={busy}
										onclick={() => onoperate('compact', row.documentId, head)}
									>
										{$t('flow.operations.integrity.compact')}
									</button>
									<button
										type="button"
										class={`${actionClass} border-amber-400 text-amber-900 dark:border-amber-700 dark:text-amber-200`}
										disabled={busy}
										onclick={() => onoperate('rebuild', row.objectId, head)}
									>
										{$t('flow.operations.integrity.rebuild')}
									</button>
								</div>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
			<ul class="mt-4 space-y-3 md:hidden">
				{#each rows as row (row.documentId)}
					{@const head = row.status === 'ok' ? row.headSeq : null}
					<li
						class="rounded-md border border-slate-200 p-3 text-sm dark:border-slate-700"
						data-status={row.status}
					>
						<p class="break-all font-mono text-xs">{row.documentId}</p>
						<p class="mt-1 break-all font-mono text-xs text-slate-500 dark:text-slate-400">
							{$t('flow.operations.integrity.objectId')}: {row.objectId}
						</p>
						{#if row.status === 'ok'}
							<p class="mt-1">
								{$t('flow.operations.integrity.headSeq')}:
								<span class="font-mono">{row.headSeq}</span>
								·
								{$t('flow.operations.integrity.projectionSeq')}:
								<span class="font-mono">{row.projectionSeq}</span>
							</p>
						{:else}
							<p class="mt-1 text-red-700 dark:text-red-300">{$t(row.failureKey)}</p>
							<p class="mt-1 text-xs text-slate-500 dark:text-slate-400">{$t(row.errorKey)}</p>
						{/if}
						<div class="mt-2 flex flex-wrap gap-2">
							<button
								type="button"
								class={`${actionClass} border-slate-300 dark:border-slate-600`}
								disabled={busy}
								onclick={() => onoperate('verify', row.documentId, head)}
							>
								{$t('flow.operations.integrity.verify')}
							</button>
							<button
								type="button"
								class={`${actionClass} border-amber-400 text-amber-900 dark:border-amber-700 dark:text-amber-200`}
								disabled={busy}
								onclick={() => onoperate('compact', row.documentId, head)}
							>
								{$t('flow.operations.integrity.compact')}
							</button>
							<button
								type="button"
								class={`${actionClass} border-amber-400 text-amber-900 dark:border-amber-700 dark:text-amber-200`}
								disabled={busy}
								onclick={() => onoperate('rebuild', row.objectId, head)}
							>
								{$t('flow.operations.integrity.rebuild')}
							</button>
						</div>
					</li>
				{/each}
			</ul>
		{/if}
	{/if}

	<div class="mt-6 border-t border-slate-200 pt-4 dark:border-slate-700">
		<h3 class="text-sm font-semibold text-slate-900 dark:text-slate-100">
			{$t('flow.operations.integrity.manual.heading')}
		</h3>
		<p class="mt-1 text-xs text-slate-500 dark:text-slate-400">
			{$t('flow.operations.integrity.manual.description')}
		</p>
		<div class="mt-3 grid grid-cols-1 gap-4 md:grid-cols-2">
			<div>
				<label
					for="flow-ops-manual-document"
					class="block text-sm font-medium text-slate-700 dark:text-slate-300"
				>
					{$t('flow.operations.integrity.manual.documentId')}
				</label>
				<input
					id="flow-ops-manual-document"
					class="mt-1 block min-h-11 w-full rounded-md border border-slate-300 px-3 font-mono text-sm focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:border-slate-600 dark:bg-slate-800"
					autocomplete="off"
					spellcheck="false"
					bind:value={manualDocument}
					aria-invalid={manualDocument !== '' && !documentValid}
				/>
				{#if manualDocument !== '' && !documentValid}
					<p class="mt-1 text-xs text-red-700 dark:text-red-300">
						{$t('flow.operations.integrity.manual.invalid')}
					</p>
				{/if}
				<div class="mt-2 flex flex-wrap gap-2">
					<button
						type="button"
						class={`${actionClass} border-slate-300 dark:border-slate-600`}
						disabled={busy || !documentValid}
						onclick={() => onoperate('verify', manualDocument.trim(), null)}
					>
						{$t('flow.operations.integrity.verify')}
					</button>
					<button
						type="button"
						class={`${actionClass} border-amber-400 text-amber-900 dark:border-amber-700 dark:text-amber-200`}
						disabled={busy || !documentValid}
						onclick={() => onoperate('compact', manualDocument.trim(), null)}
					>
						{$t('flow.operations.integrity.compact')}
					</button>
				</div>
			</div>
			<div>
				<label
					for="flow-ops-manual-object"
					class="block text-sm font-medium text-slate-700 dark:text-slate-300"
				>
					{$t('flow.operations.integrity.manual.objectId')}
				</label>
				<input
					id="flow-ops-manual-object"
					class="mt-1 block min-h-11 w-full rounded-md border border-slate-300 px-3 font-mono text-sm focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:border-slate-600 dark:bg-slate-800"
					autocomplete="off"
					spellcheck="false"
					bind:value={manualObject}
					aria-invalid={manualObject !== '' && !objectValid}
				/>
				{#if manualObject !== '' && !objectValid}
					<p class="mt-1 text-xs text-red-700 dark:text-red-300">
						{$t('flow.operations.integrity.manual.invalid')}
					</p>
				{/if}
				<div class="mt-2 flex flex-wrap gap-2">
					<button
						type="button"
						class={`${actionClass} border-amber-400 text-amber-900 dark:border-amber-700 dark:text-amber-200`}
						disabled={busy || !objectValid}
						onclick={() => onoperate('rebuild', manualObject.trim(), null)}
					>
						{$t('flow.operations.integrity.rebuild')}
					</button>
				</div>
			</div>
		</div>
	</div>
</section>
