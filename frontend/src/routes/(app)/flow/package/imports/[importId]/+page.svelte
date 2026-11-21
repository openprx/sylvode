<script lang="ts">
	// Import report deep link: polls `GET .../flow/imports/{import_id}` until a terminal status.
	// Reloading the page simply resumes polling. Missing and foreign imports share one safe state.
	import { getContext } from 'svelte';
	import { FLOW_SCOPE_CONTEXT } from '#lib/flow/context-keys.js';
	import { onDestroy, onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import { page } from '$app/state';
	import { resolve } from '$app/paths';
	import EmptyState from '#lib/components/EmptyState.svelte';
	import ImportReportView from '#lib/components/flow/ImportReportView.svelte';
	import { ImportReportPoller, type ImportReportState } from '#lib/flow/import-report.js';
	import { requireRouteParam } from '#lib/utils/route-params.js';

	const scopeId = getContext<() => string>(FLOW_SCOPE_CONTEXT)();
	const importId = requireRouteParam(page.params.importId, 'importId');

	let reportState = $state.raw<ImportReportState>({ status: 'loading' });
	const poller = new ImportReportPoller(scopeId, importId, (next) => {
		reportState = next;
	});
	onMount(() => {
		void poller.run();
	});
	onDestroy(() => poller.dispose());
</script>

<div class="mx-auto max-w-4xl space-y-6">
	<div>
		<a
			href={resolve('/(app)/flow/package')}
			class="inline-flex min-h-11 items-center text-sm text-fg underline"
		>
			{$t('flow.import.report.back')}
		</a>
		<h1 class="text-2xl font-semibold text-fg">
			{$t('flow.import.report.title')}
		</h1>
		<p class="mt-1 break-all font-mono text-xs text-fg-muted">{importId}</p>
	</div>

	{#if reportState.status === 'loading' || reportState.status === 'pending'}
		<div
			class="rounded-card border border-line bg-raised p-6 text-fg-muted"
			role="status"
			aria-live="polite"
			data-testid="flow-import-report-pending"
		>
			{reportState.status === 'loading'
				? $t('flow.import.report.loading')
				: $t('flow.import.report.pending')}
		</div>
	{:else if reportState.status === 'not_found'}
		<div data-testid="flow-import-report-not-found" role="alert">
			<EmptyState
				icon="search"
				title={$t('flow.import.report.notFoundTitle')}
				description={$t('flow.import.report.notFound')}
			/>
		</div>
	{:else if reportState.status === 'failed'}
		<div
			class="rounded-card border border-line bg-danger-soft p-6 text-sm text-danger"
			role="alert"
		>
			{$t(reportState.failure.messageKey, { values: reportState.failure.values })}
		</div>
	{:else}
		<p class="sr-only" role="status" aria-live="polite">{$t('flow.import.report.ready')}</p>
		<ImportReportView report={reportState.report} />
	{/if}
</div>
