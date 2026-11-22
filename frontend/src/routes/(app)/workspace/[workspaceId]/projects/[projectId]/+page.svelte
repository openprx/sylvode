<script lang="ts">
	import { resolve } from '$app/paths';
	import ProjectPluginSettingsLink from '#lib/components/plugins/ProjectPluginSettingsLink.svelte';
	import ProjectPluginNav from '#lib/components/plugins/ProjectPluginNav.svelte';
	import FlowSharedBlockList from '#lib/components/flow/FlowSharedBlockList.svelte';
	import { FileText } from '@lucide/svelte';
	import { onMount, onDestroy, getContext } from 'svelte';
	import { TaskWorkbench, WORKBENCH_CONTEXT } from '#lib/workbench/tabs.js';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { projectsApi, type Project } from '#lib/api/projects.js';
	import { issuesApi, type Issue } from '#lib/api/issues.js';
	import { importExportApi } from '#lib/api/import-export.js';
	import { toast } from '#lib/stores/toast.js';
	import Button from '#lib/ui/button/Button.svelte';
	import { requireRouteParam } from '#lib/utils/route-params.js';
	import {
		getProjectScenarioTemplate,
		getScenarioFields,
		templateArray
	} from '#lib/utils/scenario-template.js';
	import {
		scenarioTemplateAudience,
		scenarioTemplateIndustry,
		scenarioTemplateName
	} from '#lib/utils/project-type-i18n.js';

	const workspaceId = requireRouteParam(page.params.workspaceId, 'workspaceId');
	const projectId = requireRouteParam(page.params.projectId, 'projectId');

	let project = $state<Project | null>(null);
	let recentIssues = $state<Issue[]>([]);
	let loading = $state(true);
	let exporting = $state(false);
	let importing = $state(false);
	const scenarioTemplate = $derived(getProjectScenarioTemplate(project));
	const scenarioFields = $derived(getScenarioFields(scenarioTemplate));
	const scenarioWorkflowStates = $derived(
		templateArray(scenarioTemplate, 'workflow_template', 'states')
	);

	const workbench = getContext<TaskWorkbench | undefined>(WORKBENCH_CONTEXT);
	const taskKey = `project:${projectId}`;
	let unregisterTask: () => void = () => {};
	async function refreshDetail() {
		if (loading) return;
		const projectResponse = await projectsApi.get(projectId);
		if (projectResponse.code !== 0 || !projectResponse.data) return;
		project = projectResponse.data;
		await loadRecentIssues();
	}

	onDestroy(() => unregisterTask());

	onMount(async () => {
		const loadedProject = await loadProject();
		if (!loadedProject) {
			loading = false;
			return;
		}

		if (loadedProject.type_key === 'custom_form') {
			workbench?.close(taskKey, true);
			await goto(
				resolve('/(app)/workspace/[workspaceId]/projects/[projectId]/forms', {
					workspaceId,
					projectId
				}),
				{ replaceState: true }
			);
			return;
		}

		await loadRecentIssues();
		workbench?.setTitle(taskKey, loadedProject.name);
		unregisterTask =
			workbench?.register(taskKey, {
				capture: () => null,
				dirty: () => false,
				refresh: refreshDetail
			}) ?? (() => {});
		loading = false;
	});

	async function loadProject(): Promise<Project | null> {
		const response = await projectsApi.get(projectId);
		if (response.code !== 0) {
			toast.error(response.message);
		} else if (response.data) {
			project = response.data;
			return response.data;
		}
		return null;
	}

	async function loadRecentIssues() {
		const response = await issuesApi.list(projectId, {
			page: 1,
			per_page: 5,
			sort_by: 'updated_at',
			sort_order: 'desc'
		});
		if (response.data) {
			recentIssues = response.data.items ?? [];
		}
	}

	async function handleExportProject() {
		exporting = true;
		const response = await importExportApi.exportProject(projectId);
		if (response.code !== 0 || !response.data) {
			toast.error(response.message || get(t)('project.exportFailed'));
			exporting = false;
			return;
		}

		const payload = JSON.stringify(response.data, null, 2);
		const blob = new Blob([payload], { type: 'application/json' });
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');
		anchor.href = url;
		anchor.download = `${project?.key ?? 'project'}-export.json`;
		document.body.append(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);

		toast.success(get(t)('project.exportSuccess'));
		exporting = false;
	}

	async function handleImportFile(event: Event) {
		const target = event.currentTarget as HTMLInputElement;
		const file = target.files?.[0];
		if (!file) {
			return;
		}

		importing = true;
		try {
			const content = await file.text();
			const payload = JSON.parse(content) as unknown;
			const response = await importExportApi.importProject(workspaceId, payload);
			if (response.code !== 0) {
				toast.error(response.message);
			} else {
				toast.success(get(t)('project.importSuccess'));
			}
		} catch {
			toast.error(get(t)('project.importInvalidFile'));
		} finally {
			target.value = '';
			importing = false;
		}
	}
</script>

<div class="max-w-7xl mx-auto">
	{#if loading}
		<div class="flex items-center justify-center py-12">
			<div class="text-fg-muted">{$t('common.loading')}</div>
		</div>
	{:else if project}
		<div class="mb-6 flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
			<div>
				<div class="mb-2 flex items-center gap-3">
					<span class="rounded bg-surface px-2 py-1 text-sm text-fg-muted">{project.key}</span>
					<h1 class="text-2xl font-semibold text-fg">{project.name}</h1>
				</div>
				{#if project.description}
					<p class="text-fg-muted">{project.description}</p>
				{/if}
			</div>
			<div class="flex flex-wrap gap-2">
				<ProjectPluginSettingsLink {workspaceId} {projectId} />
				<Button variant="secondary" loading={exporting} onclick={handleExportProject}
					>{$t('project.exportJson')}</Button
				>
				<label
					class="inline-flex cursor-pointer items-center justify-center rounded-control bg-primary px-4 py-2 text-sm font-medium text-on-primary"
				>
					{importing ? $t('project.importingJson') : $t('project.importJson')}
					<input
						type="file"
						accept="application/json"
						class="hidden"
						disabled={importing}
						onchange={handleImportFile}
					/>
				</label>
			</div>
		</div>

		<div class="mb-8 grid gap-5 lg:grid-cols-[minmax(0,2fr)_minmax(280px,1fr)]">
			<section class="rounded-card border border-line bg-raised p-5">
				<div class="mb-4">
					<h2 class="text-lg font-semibold text-fg">
						{$t('project.projectManagement')}
					</h2>
					<p class="mt-1 text-sm text-fg-muted">
						{$t('project.projectManagementDesc')}
					</p>
				</div>
				<div class="grid gap-3 md:grid-cols-2">
					<a
						href={resolve('/(app)/workspace/[workspaceId]/projects/[projectId]/issues', {
							workspaceId,
							projectId
						})}
						class="rounded-control border border-line p-4 transition-colors hover:border-line-strong hover:bg-surface"
					>
						<h3 class="text-sm font-semibold text-fg">
							{$t('project.workItems')}
						</h3>
						<p class="mt-1 text-sm text-fg-muted">
							{$t('project.workItemsDesc')}
						</p>
					</a>
					<a
						href={resolve('/(app)/workspace/[workspaceId]/projects/[projectId]/board', {
							workspaceId,
							projectId
						})}
						class="rounded-control border border-line p-4 transition-colors hover:border-line-strong hover:bg-surface"
					>
						<h3 class="text-sm font-semibold text-fg">
							{$t('project.boardView')}
						</h3>
						<p class="mt-1 text-sm text-fg-muted">
							{$t('project.boardViewDesc')}
						</p>
					</a>
					<a
						href={resolve('/(app)/workspace/[workspaceId]/projects/[projectId]/cycles', {
							workspaceId,
							projectId
						})}
						class="rounded-control border border-line p-4 transition-colors hover:border-line-strong hover:bg-surface"
					>
						<h3 class="text-sm font-semibold text-fg">
							{$t('project.sprintPlan')}
						</h3>
						<p class="mt-1 text-sm text-fg-muted">
							{$t('project.sprintPlanDesc')}
						</p>
					</a>
				</div>
			</section>

			<section class="rounded-card border border-line bg-raised p-5">
				<div class="mb-4">
					<h2 class="text-lg font-semibold text-fg">
						{$t('project.businessData')}
					</h2>
					<p class="mt-1 text-sm text-fg-muted">
						{$t('project.businessDataDesc')}
					</p>
				</div>
				<a
					href={resolve('/(app)/workspace/[workspaceId]/projects/[projectId]/forms', {
						workspaceId,
						projectId
					})}
					class="flex items-start gap-3 rounded-control border border-line p-4 transition-colors hover:border-line-strong hover:bg-surface"
				>
					<FileText class="mt-0.5 h-5 w-5 text-fg-muted" aria-hidden="true" />
					<span>
						<span class="block text-sm font-semibold text-fg">
							{$t('project.forms')}
						</span>
						<span class="mt-1 block text-sm text-fg-muted">
							{$t('project.formsDesc')}
						</span>
					</span>
				</a>
			</section>
		</div>

		<ProjectPluginNav {workspaceId} {projectId} />

		{#if scenarioTemplate}
			<div class="mb-8 rounded-card border border-line bg-raised p-6">
				<div class="mb-5 flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
					<div>
						<p class="text-xs font-medium uppercase tracking-wide text-fg-muted">
							{scenarioTemplateIndustry(scenarioTemplate.industry, $t)}
						</p>
						<h2 class="mt-1 text-lg font-semibold text-fg">
							{scenarioTemplateName(scenarioTemplate, $t)}
						</h2>
						<p class="mt-1 text-sm text-fg-muted">
							{scenarioTemplateAudience(scenarioTemplate.audience_label, scenarioTemplate.key, $t)}
						</p>
					</div>
					<a
						href={resolve('/(app)/workspace/[workspaceId]/projects/[projectId]/issues', {
							workspaceId,
							projectId
						})}
						class="inline-flex items-center justify-center rounded-control border border-line px-3 py-2 text-sm font-medium text-fg transition-colors hover:border-line-strong hover:bg-surface"
					>
						{$t('project.openScenarioWork')}
					</a>
				</div>

				<div class="grid gap-3 md:grid-cols-3">
					<div class="rounded-card bg-surface p-4">
						<p class="text-xs font-medium uppercase text-fg-muted">
							{$t('project.workflow')}
						</p>
						<p class="mt-2 text-2xl font-semibold text-fg">
							{scenarioWorkflowStates.length}
						</p>
					</div>
					<div class="rounded-card bg-surface p-4">
						<p class="text-xs font-medium uppercase text-fg-muted">
							{$t('project.fields')}
						</p>
						<p class="mt-2 text-2xl font-semibold text-fg">
							{scenarioFields.length}
						</p>
					</div>
				</div>

				<div class="mt-5 grid gap-5">
					<div>
						<h3 class="mb-2 text-sm font-semibold text-fg">
							{$t('project.scenarioFields')}
						</h3>
						<div class="space-y-2">
							{#each scenarioFields.slice(0, 5) as field (field.key)}
								<div
									class="flex items-center justify-between rounded-control border border-line px-3 py-2 text-sm"
								>
									<span class="text-fg">{field.label}</span>
									<span class="text-xs text-fg-muted">{field.type}</span>
								</div>
							{/each}
						</div>
					</div>
				</div>
			</div>
		{/if}

		<div class="rounded-card border border-line bg-raised p-6">
			<h2 class="mb-4 text-lg font-semibold text-fg">
				{$t('project.recentUpdates')}
			</h2>
			{#if recentIssues.length === 0}
				<p class="text-fg-muted">{$t('project.noRecentWorkItems')}</p>
			{:else}
				<div class="space-y-3">
					{#each recentIssues as issue (issue.id)}
						<a
							href={resolve(
								'/(app)/workspace/[workspaceId]/projects/[projectId]/issues/[issueId]',
								{ workspaceId, projectId, issueId: issue.id }
							)}
							class="block rounded-control p-3 hover:bg-surface"
						>
							<div class="space-y-1.5">
								<div class="flex items-center gap-2">
									<span class="font-mono text-sm text-fg-muted">{issue.key}</span>

									<span class="text-sm font-medium text-fg">{issue.title}</span>

									<span
										class="ml-auto rounded px-2 py-1 text-xs {issue.status === 'done'
											? 'bg-success-soft text-success'
											: 'bg-surface text-fg-muted'}"
									>
										{issue.status}
									</span>
								</div>
								{#if issue.labels?.length}
									<div class="flex flex-wrap gap-1">
										{#each issue.labels as label, index (index)}
											<span
												class="inline-flex items-center rounded-full px-2 py-0.5 text-xs font-medium"
												style={`background-color: ${label.color}20; color: ${label.color}; border: 1px solid ${label.color}40;`}
											>
												{label.name}
											</span>
										{/each}
									</div>
								{/if}
							</div>
						</a>
					{/each}
				</div>
			{/if}
		</div>
	{/if}
	{#if project && project.workspace_id === workspaceId}<FlowSharedBlockList
			{workspaceId}
			{projectId}
		/>{/if}
</div>
