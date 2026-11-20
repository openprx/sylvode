<script lang="ts">
	import { getContext, onMount } from 'svelte';
	import { flowApi } from '#lib/api/flow.js';
	import { personalPackageReadiness } from '#lib/flow/personal-package.js';
	import { t } from 'svelte-i18n';
	import { FLOW_SCOPE_CONTEXT } from '#lib/flow/context-keys.js';
	import PackageImportWizard from '#lib/components/flow/PackageImportWizard.svelte';
	import PackageExportCard from '#lib/components/flow/PackageExportCard.svelte';
	const scopeId = getContext<() => string>(FLOW_SCOPE_CONTEXT)();
	let readiness = $state(personalPackageReadiness(null));
	let loading = $state(true),
		failed = $state(false);
	async function refreshReadiness() {
		loading = true;
		failed = false;
		try {
			const result = await flowApi.getCapabilities();
			readiness = personalPackageReadiness(result.code === 0 ? result.data : null);
			failed = result.code !== 0;
		} catch {
			readiness = personalPackageReadiness(null);
			failed = true;
		} finally {
			loading = false;
		}
	}
	onMount(() => {
		void refreshReadiness();
	});
</script>

<div class="flex-1 overflow-y-auto p-4 md:p-6">
	<div class="mx-auto max-w-3xl space-y-6">
		<h1 class="text-xl font-semibold">{$t('flow.personal.packages')}</h1>
		<p class="text-sm text-fg-muted">{$t('flow.personal.packageNote')}</p>
		{#if loading}<p class="text-sm text-fg-muted" role="status">{$t('common.loading')}</p>{/if}
		{#if failed}<p class="text-sm text-danger" role="alert">
				{$t('flow.personal.packageAvailabilityFailed')}
			</p>{/if}
		{#if readiness.exports}
			<PackageExportCard
				{scopeId}
				workspaceLabel={$t('flow.personal.personalFlow')}
				projects={[]}
				allowHistory={readiness.historyExports}
			/>
		{:else if !loading}<p class="text-sm text-fg-muted">
				{$t('flow.personal.packageExportUnavailable')}
			</p>{/if}
		{#if readiness.imports}
			{#if readiness.historyImports}<p class="text-sm text-fg-muted">
					{$t('flow.personal.packageHistoryReplay')}
				</p>{/if}
			<PackageImportWizard {scopeId} projects={[]} allowHistory={readiness.historyImports} />
		{:else if !loading}<p class="text-sm text-fg-muted">
				{$t('flow.personal.packageImportUnavailable')}
			</p>{/if}
		<button
			type="button"
			class="min-h-11 rounded-control border border-fg bg-raised px-3 text-sm disabled:bg-surface disabled:text-control disabled:border-line"
			onclick={refreshReadiness}
			disabled={loading}>{$t('flow.personal.packageCheckAvailability')}</button
		>
	</div>
</div>
