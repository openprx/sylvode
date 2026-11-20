<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { governanceApi, type GovernanceAuditLog } from '#lib/api/governance.js';
	import { toast } from '#lib/stores/toast.js';
	import { projectOptionsStore } from '#lib/stores/project-options.js';
	import { groupProjectOptionsByWorkspace } from '#lib/utils/project-options.js';

	let loading = $state(true);
	const projects = $derived($projectOptionsStore.items);
	const groupedProjects = $derived.by(() => groupProjectOptionsByWorkspace(projects));
	let logs = $state<GovernanceAuditLog[]>([]);
	let selectedProjectId = $state('');
	let action = $state('');
	let resourceType = $state('');
	let startAt = $state('');
	let endAt = $state('');
	let page = $state(1);
	let perPage = $state(20);
	let total = $state(0);
	let totalPages = $state(0);

	onMount(() => {
		void init();
	});

	async function init() {
		loading = true;
		await projectOptionsStore.ensureLoaded();
		if (projects.length > 0) {
			selectedProjectId = projects[0].id;
		}
		await loadLogs();
		loading = false;
	}

	async function loadLogs() {
		const res = await governanceApi.listAuditLogs({
			project_id: selectedProjectId || undefined,
			action: action.trim() || undefined,
			resource_type: resourceType.trim() || undefined,
			start_at: startAt ? new Date(startAt).toISOString() : undefined,
			end_at: endAt ? new Date(endAt).toISOString() : undefined,
			page,
			per_page: perPage
		});
		if (res.code !== 0 || !res.data) {
			toast.error(res.message || get(t)('governanceAuditLogs.loadFailed'));
			logs = [];
			total = 0;
			totalPages = 0;
			return;
		}
		logs = res.data.items ?? [];
		total = res.data.total;
		totalPages = res.data.total_pages;
	}

	async function onSearch() {
		page = 1;
		await loadLogs();
	}

	async function prevPage() {
		if (page <= 1) return;
		page -= 1;
		await loadLogs();
	}

	async function nextPage() {
		if (totalPages > 0 && page >= totalPages) return;
		page += 1;
		await loadLogs();
	}

	function renderJson(value: unknown): string {
		if (value === null || value === undefined) return '-';
		try {
			return JSON.stringify(value);
		} catch {
			return '-';
		}
	}
</script>

<svelte:head>
	<title>{$t('pageTitle.governanceAuditLogs')}</title>
</svelte:head>

<div class="mx-auto max-w-7xl space-y-4">
	<div class="rounded-card border border-line bg-raised p-5">
		<h1 class="text-xl font-semibold text-fg">
			{$t('governanceAuditLogs.title')}
		</h1>
		<p class="mt-1 text-sm text-fg-muted">{$t('governanceAuditLogs.subtitle')}</p>
	</div>

	<div class="grid grid-cols-1 gap-3 rounded-card border border-line bg-raised p-4 md:grid-cols-6">
		<div class="md:col-span-2">
			<label for="audit-log-project" class="mb-1 block text-sm text-fg-muted"
				>{$t('impactReview.project')}</label
			>
			<select
				id="audit-log-project"
				bind:value={selectedProjectId}
				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm"
			>
				<option value="">{`- ${$t('impactReview.allProjects')} -`}</option>
				{#each groupedProjects as group}
					<optgroup label={group.workspaceName}>
						{#each group.items as project}
							<option value={project.id}>{project.name}</option>
						{/each}
					</optgroup>
				{/each}
			</select>
		</div>
		<div>
			<label for="audit-log-action" class="mb-1 block text-sm text-fg-muted"
				>{$t('governanceAuditLogs.action')}</label
			>
			<input
				id="audit-log-action"
				bind:value={action}
				type="text"
				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm"
			/>
		</div>
		<div>
			<label for="audit-log-resource-type" class="mb-1 block text-sm text-fg-muted"
				>{$t('governanceAuditLogs.resourceType')}</label
			>
			<input
				id="audit-log-resource-type"
				bind:value={resourceType}
				type="text"
				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm"
			/>
		</div>
		<div>
			<label for="audit-log-start-at" class="mb-1 block text-sm text-fg-muted"
				>{$t('governanceAuditLogs.startAt')}</label
			>
			<input
				id="audit-log-start-at"
				bind:value={startAt}
				type="datetime-local"
				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm"
			/>
		</div>
		<div>
			<label for="audit-log-end-at" class="mb-1 block text-sm text-fg-muted"
				>{$t('governanceAuditLogs.endAt')}</label
			>
			<input
				id="audit-log-end-at"
				bind:value={endAt}
				type="datetime-local"
				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm"
			/>
		</div>
		<div class="md:col-span-6">
			<button
				type="button"
				onclick={onSearch}
				class="rounded-control rf-filled bg-primary px-4 py-2 text-sm text-on-primary"
			>
				{$t('common.search')}
			</button>
		</div>
	</div>

	<div class="rounded-card border border-line bg-raised p-4">
		{#if loading}
			<p class="text-sm text-fg-muted">{$t('common.loading')}</p>
		{:else if logs.length === 0}
			<p class="text-sm text-fg-muted">{$t('governanceAuditLogs.empty')}</p>
		{:else}
			<div class="overflow-x-auto">
				<table class="min-w-full text-left text-sm">
					<thead>
						<tr class="border-b border-line text-xs text-fg-muted">
							<th class="px-2 py-2">ID</th>
							<th class="px-2 py-2">{$t('common.createdAt')}</th>
							<th class="px-2 py-2">{$t('governanceAuditLogs.action')}</th>
							<th class="px-2 py-2">{$t('governanceAuditLogs.resourceType')}</th>
							<th class="px-2 py-2">{$t('governanceAuditLogs.actor')}</th>
							<th class="px-2 py-2">{$t('governanceAuditLogs.resourceId')}</th>
							<th class="px-2 py-2">{$t('governanceAuditLogs.changes')}</th>
						</tr>
					</thead>
					<tbody>
						{#each logs as item}
							<tr class="border-b border-line align-top">
								<td class="px-2 py-2 text-xs text-fg-muted">{item.id}</td>
								<td class="px-2 py-2 text-xs text-fg-muted"
									>{new Date(item.created_at).toLocaleString()}</td
								>
								<td class="px-2 py-2 text-xs">{item.action}</td>
								<td class="px-2 py-2 text-xs">{item.resource_type}</td>
								<td class="px-2 py-2 text-xs">{item.actor_id || '-'}</td>
								<td class="px-2 py-2 text-xs">{item.resource_id || '-'}</td>
								<td class="max-w-xl px-2 py-2 text-xs text-fg-muted">
									<div class="space-y-1">
										<p>
											<span class="font-medium">{$t('governanceAuditLogs.old')}</span>
											{renderJson(item.old_value)}
										</p>
										<p>
											<span class="font-medium">{$t('governanceAuditLogs.new')}</span>
											{renderJson(item.new_value)}
										</p>
									</div>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}

		<div class="mt-4 flex items-center justify-between border-t border-line pt-3 text-sm">
			<p class="text-fg-muted">
				{$t('governance.pagination', { values: { total, page, pages: totalPages || 1 } })}
			</p>
			<div class="flex gap-2">
				<button
					type="button"
					onclick={prevPage}
					class="rounded border border-fg bg-raised px-3 py-1.5 text-xs hover:bg-surface"
					disabled={page <= 1}
				>
					{$t('governance.prevPage')}
				</button>
				<button
					type="button"
					onclick={nextPage}
					class="rounded border border-fg bg-raised px-3 py-1.5 text-xs hover:bg-surface"
					disabled={totalPages > 0 && page >= totalPages}
				>
					{$t('governance.nextPage')}
				</button>
			</div>
		</div>
	</div>
</div>
