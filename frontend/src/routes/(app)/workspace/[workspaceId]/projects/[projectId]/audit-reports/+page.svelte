<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { projectsApi } from '#lib/api/projects.js';
	import { governanceExtApi, type AuditReport } from '#lib/api/governance-ext.js';
	import { toast } from '#lib/stores/toast.js';

	const projectId = $derived(page.params.projectId || '');
	const workspaceId = $derived(page.params.workspaceId || '');

	let loading = $state(true);
	let generating = $state(false);
	let projectName = $state('');
	let reports = $state<AuditReport[]>([]);
	let periodStart = $state('');
	let periodEnd = $state('');
	let pageNum = $state(1);
	let perPage = $state(20);
	let total = $state(0);
	let totalPages = $state(0);

	onMount(() => {
		void init();
	});

	async function init() {
		loading = true;
		const projectRes = await projectsApi.get(projectId);
		if (projectRes.code === 0 && projectRes.data) {
			projectName = projectRes.data.name;
		}
		await loadReports();
		loading = false;
	}

	async function loadReports() {
		const res = await governanceExtApi.listProjectAuditReports(projectId, {
			page: pageNum,
			per_page: perPage
		});
		if (res.code !== 0 || !res.data) {
			toast.error(res.message || get(t)('governanceExt.auditLoadFailed'));
			reports = [];
			total = 0;
			totalPages = 0;
			return;
		}
		reports = res.data.items;
		total = res.data.total;
		totalPages = res.data.total_pages;
	}

	async function generateReport() {
		generating = true;
		const res = await governanceExtApi.createProjectAuditReport(projectId, {
			period_start: periodStart || undefined,
			period_end: periodEnd || undefined
		});
		generating = false;
		if (res.code !== 0 || !res.data) {
			toast.error(res.message || get(t)('governanceExt.auditGenerateFailed'));
			return;
		}
		toast.success(get(t)('governanceExt.auditGenerateSuccess'));
		pageNum = 1;
		await loadReports();
	}

	async function goPage(next: number) {
		pageNum = Math.max(1, Math.min(next, totalPages || 1));
		await loadReports();
	}
</script>

<svelte:head>
	<title>{$t('pageTitle.auditReports')}</title>
</svelte:head>

<div class="mx-auto max-w-7xl space-y-4">
	<div class="rounded-card border border-line bg-raised p-5">
		<h1 class="text-xl font-semibold text-fg">
			{$t('governanceExt.auditTitle')}
		</h1>
		<p class="mt-1 text-sm text-fg-muted">
			{$t('governanceExt.auditSubtitle', { values: { project: projectName || projectId } })}
		</p>
	</div>

	<div class="grid grid-cols-1 gap-3 rounded-card border border-line bg-raised p-4 md:grid-cols-4">
		<div>
			<label for="audit-period-start" class="mb-1 block text-sm text-fg-muted"
				>{$t('governanceExt.periodStart')}</label
			>
			<input
				id="audit-period-start"
				bind:value={periodStart}
				type="date"
				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm"
			/>
		</div>
		<div>
			<label for="audit-period-end" class="mb-1 block text-sm text-fg-muted"
				>{$t('governanceExt.periodEnd')}</label
			>
			<input
				id="audit-period-end"
				bind:value={periodEnd}
				type="date"
				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm"
			/>
		</div>
		<div class="md:col-span-2 flex items-end">
			<button
				type="button"
				onclick={generateReport}
				disabled={generating}
				class="w-full rounded-control bg-primary px-3 py-2 text-sm text-on-primary disabled:bg-surface disabled:text-fg-muted disabled:cursor-not-allowed"
			>
				{generating ? $t('common.processing') : $t('governanceExt.generateReport')}
			</button>
		</div>
	</div>

	<div class="rounded-card border border-line bg-raised p-4">
		{#if loading}
			<p class="text-sm text-fg-muted">{$t('common.loading')}</p>
		{:else if reports.length === 0}
			<p class="text-sm text-fg-muted">{$t('common.noData')}</p>
		{:else}
			<div class="overflow-x-auto">
				<table class="min-w-full text-left text-sm">
					<thead>
						<tr class="border-b border-line text-xs text-fg-muted">
							<th class="px-2 py-2">{$t('common.id')}</th>
							<th class="px-2 py-2">{$t('governanceExt.period')}</th>
							<th class="px-2 py-2">{$t('governanceExt.totalDecisions')}</th>
							<th class="px-2 py-2">{$t('governanceExt.passRate')}</th>
							<th class="px-2 py-2">{$t('common.createdAt')}</th>
							<th class="px-2 py-2">{$t('common.actions')}</th>
						</tr>
					</thead>
					<tbody>
						{#each reports as report}
							<tr class="border-b border-line">
								<td class="px-2 py-2 text-xs text-fg-muted">{report.id}</td>
								<td class="px-2 py-2 text-xs">
									{report.period_start} ~ {report.period_end}
								</td>
								<td class="px-2 py-2 text-xs">{report.total_proposals}</td>
								<td class="px-2 py-2 text-xs">
									{report.total_proposals > 0
										? `${((report.approved_proposals / report.total_proposals) * 100).toFixed(1)}%`
										: '0.0%'}
								</td>
								<td class="px-2 py-2 text-xs text-fg-muted"
									>{new Date(report.generated_at).toLocaleString()}</td
								>
								<td class="px-2 py-2 text-xs">
									<a
										href={`/workspace/${workspaceId}/projects/${projectId}/audit-reports/${report.id}`}
										class="text-fg underline"
									>
										{$t('governanceExt.viewDetail')}
									</a>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}

		<div class="mt-4 flex items-center justify-between border-t border-line pt-3 text-sm">
			<p class="text-fg-muted">
				{$t('governance.pagination', { values: { total, page: pageNum, pages: totalPages || 1 } })}
			</p>
			<div class="flex gap-2">
				<button
					type="button"
					onclick={() => goPage(pageNum - 1)}
					disabled={pageNum <= 1}
					class="rounded border border-fg px-3 py-1.5 text-xs hover:bg-surface disabled:bg-surface disabled:text-fg-muted disabled:cursor-not-allowed"
				>
					{$t('governance.prevPage')}
				</button>
				<button
					type="button"
					onclick={() => goPage(pageNum + 1)}
					disabled={totalPages > 0 && pageNum >= totalPages}
					class="rounded border border-fg px-3 py-1.5 text-xs hover:bg-surface disabled:bg-surface disabled:text-fg-muted disabled:cursor-not-allowed"
				>
					{$t('governance.nextPage')}
				</button>
			</div>
		</div>
	</div>
</div>
