<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { governanceExtApi, type AuditReport } from '#lib/api/governance-ext.js';
	import { toast } from '#lib/stores/toast.js';

	const projectId = $derived(page.params.projectId || '');
	const workspaceId = $derived(page.params.workspaceId || '');
	const reportId = $derived(page.params.id || '');

	let loading = $state(true);
	let report = $state<AuditReport | null>(null);

	const reportTitle = $derived.by(() => {
		if (report?.id) {
			return $t('pageTitle.auditReportDetailWithId', { values: { id: report.id } });
		}
		return $t('pageTitle.auditReportDetail');
	});

	onMount(() => {
		void load();
	});

	async function load() {
		loading = true;
		const res = await governanceExtApi.getProjectAuditReport(projectId, reportId);
		if (res.code !== 0 || !res.data) {
			toast.error(res.message || get(t)('governanceExt.auditLoadFailed'));
			loading = false;
			return;
		}
		report = res.data;
		loading = false;
	}

	function jsonText(value: unknown): string {
		if (value === null || value === undefined) {
			return '-';
		}
		try {
			return JSON.stringify(value, null, 2);
		} catch {
			return '-';
		}
	}
</script>

<svelte:head>
	<title>{reportTitle}</title>
</svelte:head>

<div class="mx-auto max-w-6xl space-y-4">
	<div
		class="rounded-card border border-line bg-raised p-5"
	>
		<div class="flex items-center justify-between gap-3">
			<div>
				<h1 class="text-xl font-semibold text-fg">
					{$t('governanceExt.auditDetailTitle')}
				</h1>
				<p class="mt-1 text-sm text-fg-muted">{reportId}</p>
			</div>
			<a
				href={`/workspace/${workspaceId}/projects/${projectId}/audit-reports`}
				class="rounded-control border border-fg px-3 py-2 text-xs hover:bg-surface"
			>
				{$t('governanceExt.backToList')}
			</a>
		</div>
	</div>

	{#if loading}
		<div
			class="rounded-card border border-line bg-raised p-6 text-sm text-fg-muted"
		>
			{$t('common.loading')}
		</div>
	{:else if report}
		<div class="grid grid-cols-1 gap-4 md:grid-cols-4">
			<div
				class="rounded-card border border-line bg-raised p-4"
			>
				<p class="text-xs text-fg-muted">{$t('governanceExt.totalDecisions')}</p>
				<p class="mt-1 text-2xl font-semibold text-fg">
					{report.total_proposals}
				</p>
			</div>
			<div
				class="rounded-card border border-line bg-raised p-4"
			>
				<p class="text-xs text-fg-muted">{$t('governance.status.approved')}</p>
				<p class="mt-1 text-2xl font-semibold text-success">{report.approved_proposals}</p>
			</div>
			<div
				class="rounded-card border border-line bg-raised p-4"
			>
				<p class="text-xs text-fg-muted">{$t('governance.status.rejected')}</p>
				<p class="mt-1 text-2xl font-semibold text-danger">{report.rejected_proposals}</p>
			</div>
			<div
				class="rounded-card border border-line bg-raised p-4"
			>
				<p class="text-xs text-fg-muted">{$t('governance.status.vetoed')}</p>
				<p class="mt-1 text-2xl font-semibold text-danger">{report.vetoed_proposals}</p>
			</div>
		</div>

		<div class="grid grid-cols-1 gap-4 lg:grid-cols-2">
			<div
				class="rounded-card border border-line bg-raised p-4"
			>
				<h2 class="mb-2 text-sm font-semibold text-fg">
					{$t('governanceExt.ratingDistribution')}
				</h2>
				<pre class="overflow-auto rounded-card bg-surface p-3 text-xs">{jsonText(
						report.rating_distribution
					)}</pre>
			</div>
			<div
				class="rounded-card border border-line bg-raised p-4"
			>
				<h2 class="mb-2 text-sm font-semibold text-fg">
					{$t('governanceExt.aiParticipation')}
				</h2>
				<pre class="overflow-auto rounded-card bg-surface p-3 text-xs">{jsonText(
						report.ai_participation_stats
					)}</pre>
			</div>
			<div
				class="rounded-card border border-line bg-raised p-4"
			>
				<h2 class="mb-2 text-sm font-semibold text-fg">
					{$t('governanceExt.domainStats')}
				</h2>
				<pre class="overflow-auto rounded-card bg-surface p-3 text-xs">{jsonText(
						report.domain_stats
					)}</pre>
			</div>
			<div
				class="rounded-card border border-line bg-raised p-4"
			>
				<h2 class="mb-2 text-sm font-semibold text-fg">
					{$t('governanceExt.keyInsights')}
				</h2>
				<pre class="overflow-auto rounded-card bg-surface p-3 text-xs">{jsonText(
						report.key_insights
					)}</pre>
			</div>
		</div>
	{/if}
</div>
