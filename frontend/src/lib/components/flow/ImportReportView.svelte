<script lang="ts">
	// Renders a finished `ImportReport` (`contracts/export-package-v1.md`). Only report fields are
	// shown; there are no CRDT bytes in a report and none are requested.
	import { t } from 'svelte-i18n';
	import Card from '#lib/components/Card.svelte';
	import {
		IMPORT_COUNT_LABEL_KEYS,
		IMPORT_REPORT_COUNT_KEYS,
		IMPORT_STATUS_KEYS,
		type FlowPackageImportReport
	} from '#lib/flow/import-report.js';
	import { CONFLICT_POLICY_KEYS } from '#lib/flow/package-wizard.js';

	interface Props {
		report: FlowPackageImportReport;
	}

	let { report }: Props = $props();

	const objectRows = $derived(Object.entries(report.object_mapping));

	function formatTime(value: string): string {
		const date = new Date(value);
		return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
	}

	function statusLabel(status: string): string {
		const key = IMPORT_STATUS_KEYS[status];
		return key ? $t(key) : $t('flow.import.report.status.unknown', { values: { status } });
	}
</script>

<div class="space-y-6" data-testid="flow-import-report">
	<Card>
		<dl class="grid grid-cols-1 gap-4 text-sm md:grid-cols-2">
			<div>
				<dt class="text-fg-muted">{$t('flow.import.report.statusLabel')}</dt>
				<dd
					class="mt-1 font-medium text-fg"
					data-testid="flow-import-report-status"
				>
					{statusLabel(report.status)}
				</dd>
			</div>
			<div>
				<dt class="text-fg-muted">
					{$t('flow.import.report.conflictPolicy')}
				</dt>
				<dd class="mt-1 text-fg">
					{report.conflict_policy === 'reject_existing' ||
					report.conflict_policy === 'reuse_import_lineage'
						? $t(CONFLICT_POLICY_KEYS[report.conflict_policy])
						: report.conflict_policy}
				</dd>
			</div>
			<div class="md:col-span-2">
				<dt class="text-fg-muted">{$t('flow.package.sourceProvenance')}</dt>
				<dd class="mt-1 break-all font-mono text-xs text-fg">
					{#if report.source_scope_id}<span class="block"
							>{$t('flow.package.sourceScope')}: {report.source_scope_id}</span
						>{/if}
					{#if report.source_workspace_id}<span class="block"
							>{$t('flow.package.sourceWorkspace')}: {report.source_workspace_id}</span
						>{/if}
					<span class="block">{$t('flow.package.targetScope')}: {report.target_scope_id}</span>
					{#if report.target_workspace_id}<span class="block"
							>{$t('flow.package.targetWorkspace')}: {report.target_workspace_id}</span
						>{/if}
				</dd>
			</div>
			<div class="md:col-span-2">
				<dt class="text-fg-muted">{$t('flow.import.packageSha256')}</dt>
				<dd class="mt-1 break-all font-mono text-xs text-fg">
					{report.package_sha256}
				</dd>
			</div>
			<div class="md:col-span-2">
				<dt class="text-fg-muted">{$t('flow.import.mappingHash')}</dt>
				<dd class="mt-1 break-all font-mono text-xs text-fg">
					{report.mapping_hash}
				</dd>
			</div>
			<div>
				<dt class="text-fg-muted">{$t('flow.import.report.startedAt')}</dt>
				<dd class="mt-1 text-fg">{formatTime(report.started_at)}</dd>
			</div>
			<div>
				<dt class="text-fg-muted">{$t('flow.import.report.finishedAt')}</dt>
				<dd class="mt-1 text-fg">{formatTime(report.finished_at)}</dd>
			</div>
			<div>
				<dt class="text-fg-muted">{$t('flow.import.report.actor')}</dt>
				<dd class="mt-1 break-all font-mono text-xs text-fg">
					{report.actor}
				</dd>
			</div>
			<div>
				<dt class="text-fg-muted">{$t('flow.import.report.auditEvent')}</dt>
				<dd class="mt-1 break-all font-mono text-xs text-fg">
					{report.audit_event_id}
				</dd>
			</div>
		</dl>
	</Card>

	<Card>
		<h2 class="text-lg font-semibold text-fg">
			{$t('flow.import.report.countsHeading')}
		</h2>
		<dl
			class="mt-3 grid grid-cols-2 gap-3 text-sm md:grid-cols-5"
			data-testid="flow-import-report-counts"
		>
			{#each IMPORT_REPORT_COUNT_KEYS as key (key)}
				<div class="rounded-md border border-line p-3">
					<dt class="text-fg-muted">{$t(IMPORT_COUNT_LABEL_KEYS[key])}</dt>
					<dd
						class="mt-1 text-xl font-semibold text-fg"
						data-testid={`flow-import-report-count-${key}`}
					>
						{report.counts[key] ?? 0}
					</dd>
				</div>
			{/each}
		</dl>
	</Card>

	<Card>
		<h2 class="text-lg font-semibold text-fg">
			{$t('flow.import.report.objectMappingHeading')}
		</h2>
		{#if objectRows.length === 0}
			<p class="mt-2 text-sm text-fg-muted">
				{$t('flow.import.preview.none')}
			</p>
		{:else}
			<table class="mt-3 hidden w-full text-left text-sm md:table">
				<thead>
					<tr class="border-b border-line">
						<th scope="col" class="py-2 pr-4 font-medium text-fg-muted">
							{$t('flow.import.report.sourceObject')}
						</th>
						<th scope="col" class="py-2 font-medium text-fg-muted">
							{$t('flow.import.report.targetObject')}
						</th>
					</tr>
				</thead>
				<tbody>
					{#each objectRows as [source, target] (source)}
						<tr class="border-b border-line">
							<td class="break-all py-2 pr-4 font-mono text-xs">{source}</td>
							<td class="break-all py-2 font-mono text-xs">{target}</td>
						</tr>
					{/each}
				</tbody>
			</table>
			<ul class="mt-3 space-y-2 md:hidden">
				{#each objectRows as [source, target] (source)}
					<li class="rounded-md border border-line p-3 text-xs">
						<p class="text-fg-muted">
							{$t('flow.import.report.sourceObject')}
						</p>
						<p class="break-all font-mono">{source}</p>
						<p class="mt-2 text-fg-muted">
							{$t('flow.import.report.targetObject')}
						</p>
						<p class="break-all font-mono">{target}</p>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>

	<Card>
		<h2 class="text-lg font-semibold text-fg">
			{$t('flow.import.report.detachedHeading')}
		</h2>
		{#if report.detached_references.length === 0}
			<p class="mt-2 text-sm text-fg-muted">
				{$t('flow.import.preview.none')}
			</p>
		{:else}
			<ul class="mt-2 space-y-1 break-all font-mono text-xs">
				{#each report.detached_references as reference (reference)}<li>{reference}</li>{/each}
			</ul>
		{/if}
		<h2 class="mt-6 text-lg font-semibold text-fg">
			{$t('flow.import.preview.warnings')}
		</h2>
		{#if report.warnings.length === 0}
			<p class="mt-2 text-sm text-fg-muted">
				{$t('flow.import.preview.none')}
			</p>
		{:else}
			<ul class="mt-2 list-disc space-y-1 pl-5 text-sm">
				{#each report.warnings as warning (warning)}<li>{warning}</li>{/each}
			</ul>
		{/if}
	</Card>
</div>
