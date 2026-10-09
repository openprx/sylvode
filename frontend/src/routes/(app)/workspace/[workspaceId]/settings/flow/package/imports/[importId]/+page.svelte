<script lang="ts">
	// Import report deep link: polls `GET .../flow/imports/{import_id}` until a terminal status.
	// Reloading the page simply resumes polling. Missing and foreign imports share one safe state.
	import { onDestroy, onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import { page } from '$app/stores';
	import { resolve } from '$app/paths';
	import EmptyState from '$lib/components/EmptyState.svelte';
	import ImportReportView from '$lib/components/flow/ImportReportView.svelte';
	import { ImportReportPoller, type ImportReportState } from '$lib/flow/import-report';
	import { requireRouteParam } from '$lib/utils/route-params';

	const workspaceId = requireRouteParam($page.params.workspaceId, 'workspaceId');
	const importId = requireRouteParam($page.params.importId, 'importId');

	let reportState = $state.raw<ImportReportState>({ status: 'loading' });
	const poller = new ImportReportPoller(workspaceId, importId, (next) => {
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
			href={resolve('/(app)/workspace/[workspaceId]/settings/flow/package', { workspaceId })}
			class="inline-flex min-h-11 items-center text-sm text-blue-700 hover:underline focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:text-blue-300"
		>
			{$t('flow.import.report.back')}
		</a>
		<h1 class="text-2xl font-bold text-slate-900 dark:text-slate-100">
			{$t('flow.import.report.title')}
		</h1>
		<p class="mt-1 break-all font-mono text-xs text-slate-500 dark:text-slate-400">{importId}</p>
	</div>

	{#if reportState.status === 'loading' || reportState.status === 'pending'}
		<div
			class="rounded-lg border border-slate-200 bg-white p-6 text-slate-600 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-300"
			role="status"
			aria-live="polite"
			data-testid="flow-import-report-pending"
		>
			{reportState.status === 'loading'
				? $t('flow.import.report.loading')
				: $t('flow.import.report.pending')}
		</div>
	{:else if reportState.status === 'not_found'}
		<div data-testid="flow-import-report-not-found" role="status" aria-live="polite">
			<EmptyState
				icon="search"
				title={$t('flow.import.report.notFoundTitle')}
				description={$t('flow.import.report.notFound')}
			/>
		</div>
	{:else if reportState.status === 'failed'}
		<div
			class="rounded-lg border border-red-200 bg-red-50 p-6 text-sm text-red-700 dark:border-red-900 dark:bg-red-950/30 dark:text-red-300"
			role="alert"
		>
			{$t(reportState.failure.messageKey, { values: reportState.failure.values })}
		</div>
	{:else}
		<p class="sr-only" role="status" aria-live="polite">{$t('flow.import.report.ready')}</p>
		<ImportReportView report={reportState.report} />
	{/if}
</div>
