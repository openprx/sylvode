<script lang="ts">
	import { onMount, onDestroy, getContext } from 'svelte';
	import { TaskWorkbench, WORKBENCH_CONTEXT } from '#lib/workbench/tabs.js';
	import { projectConflicts, reviewProjectDraft } from '#lib/workbench/project-session.js';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { page } from '$app/state';
	import { projectsApi, type Project, type ProjectType } from '#lib/api/projects.js';
	import { workspacesApi, type Workspace } from '#lib/api/workspaces.js';
	import { workflowsApi, type WorkflowSummary } from '#lib/api/workflows.js';
	import { toast } from '#lib/stores/toast.js';
	import { goto } from '$app/navigation';
	import { CheckCircle2, Code2, TableProperties } from '@lucide/svelte';
	import Modal from '#lib/components/Modal.svelte';
	import Button from '#lib/components/Button.svelte';
	import Input from '#lib/components/Input.svelte';
	import EmptyState from '#lib/components/EmptyState.svelte';
	import { requireRouteParam } from '#lib/utils/route-params.js';
	import { timeAgo } from '#lib/utils/timeago.js';
	import { projectTypeDescription, projectTypeName } from '#lib/utils/project-type-i18n.js';

	const workspaceId = requireRouteParam(page.params.workspaceId, 'workspaceId');

	let workspace = $state<Workspace | null>(null);
	let projects = $state<Project[]>([]);
	let projectTypes = $state<ProjectType[]>([]);
	let filteredProjects = $state<Project[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);

	let searchQuery = $state('');

	let showCreateModal = $state(false);
	let createForm = $state({
		name: '',
		key: '',
		description: '',
		type_key: 'code_project'
	});
	let creating = $state(false);

	let showEditModal = $state(false);
	let editingProject = $state<Project | null>(null);
	let editForm = $state({ name: '', description: '', workflow_id: '' });
	let updating = $state(false);
	let availableWorkflows = $state<WorkflowSummary[]>([]);

	let showDeleteModal = $state(false);
	let deletingProject = $state<Project | null>(null);
	let deleting = $state(false);

	const selectableProjectTypes = $derived.by(() => {
		const preferred = ['code_project', 'custom_form'];
		const ordered = preferred
			.map((key) => projectTypes.find((item) => item.key === key))
			.filter((item): item is ProjectType => Boolean(item));
		return ordered.length > 0 ? ordered : projectTypes;
	});
	const selectedProjectType = $derived(
		projectTypes.find((item) => item.key === createForm.type_key) ??
			selectableProjectTypes[0] ??
			null
	);

	const workbench = getContext<TaskWorkbench | undefined>(WORKBENCH_CONTEXT);
	const taskKey = `projects:${workspaceId}`;
	let unregisterTask: () => void = () => {};
	let remoteProject = $state<Project | null>(null);
	let projectUnavailable = $state(false);
	type ProjectSnapshot = {
		search: string;
		showCreate: boolean;
		create: typeof createForm;
		showEdit: boolean;
		editing: Project | null;
		edit: typeof editForm;
		remote: Project | null;
	};
	function projectDirty() {
		return (
			(showCreateModal && !!(createForm.name || createForm.key || createForm.description)) ||
			(showEditModal &&
				!!editingProject &&
				(editForm.name !== editingProject.name ||
					editForm.description !== (editingProject.description ?? '') ||
					editForm.workflow_id !== ''))
		);
	}
	function captureProject(): ProjectSnapshot {
		return {
			search: searchQuery,
			showCreate: showCreateModal,
			create: $state.snapshot(createForm),
			showEdit: showEditModal,
			editing: $state.snapshot(editingProject),
			edit: $state.snapshot(editForm),
			remote: $state.snapshot(remoteProject)
		};
	}
	async function refreshProjects() {
		if (loading || creating || updating || deleting) return;
		const result = await projectsApi.list(workspaceId);
		if (result.code !== 0 || !result.data) {
			if (result.code === 403 || result.code === 404) projectUnavailable = true;
			return;
		}
		const fresh = result.data.items ?? [];
		if (showEditModal && editingProject) {
			const remote = fresh.find((item) => item.id === editingProject!.id);
			projectUnavailable = !remote;
			if (remote && remote.updated_at !== editingProject.updated_at) {
				remoteProject = remote;
				workbench?.markChanged(taskKey);
			}
		}
		projects = fresh;
	}
	function reviewProject() {
		if (!remoteProject || !editingProject) return;
		editForm = { ...editForm, ...reviewProjectDraft(editingProject, editForm, remoteProject) };
		editingProject = remoteProject;
		remoteProject = null;
		workbench?.markChanged(taskKey, false);
		projectUnavailable = false;
	}
	$effect(() => {
		void editForm.name;
		void editForm.description;
		void editForm.workflow_id;
		void createForm.name;
		void createForm.key;
		void createForm.description;
		workbench?.markDirty(taskKey, projectDirty());
	});
	onMount(async () => {
		await loadData();
		const saved = workbench?.snapshot<ProjectSnapshot>(taskKey);
		if (saved) {
			searchQuery = saved.search;
			createForm = saved.create;
			showCreateModal = saved.showCreate;
			editingProject = saved.editing;
			editForm = saved.edit;
			showEditModal = saved.showEdit;
			remoteProject = saved.remote;
			if (showEditModal && editingProject) {
				const fresh = projects.find((item) => item.id === editingProject!.id);
				if (fresh && fresh.updated_at !== editingProject.updated_at) remoteProject = fresh;
				projectUnavailable = !fresh;
			}
		}
		workbench?.setTitle(taskKey, workspace?.name ?? get(t)('workbench.projects'));
		unregisterTask =
			workbench?.register(taskKey, {
				capture: captureProject,
				dirty: projectDirty,
				refresh: refreshProjects
			}) ?? (() => {});
	});
	onDestroy(() => unregisterTask());

	async function loadData() {
		loading = true;
		error = null;

		const wsResponse = await workspacesApi.get(workspaceId);
		if (wsResponse.code !== 0) {
			error = wsResponse.message;
			toast.error(wsResponse.message);
		} else if (wsResponse.data) {
			workspace = wsResponse.data;
		}

		const response = await projectsApi.list(workspaceId);
		if (response.code !== 0) {
			error = response.message;
			toast.error(response.message);
		} else if (response.data) {
			projects = response.data.items ?? [];
			filteredProjects = projects;
		}

		const typeResponse = await projectsApi.listTypes(workspaceId);
		if (typeResponse.code === 0 && typeResponse.data) {
			projectTypes = typeResponse.data.items ?? [];
			if (!projectTypes.some((item) => item.key === createForm.type_key) && projectTypes[0]) {
				createForm.type_key = projectTypes[0].key;
			}
		}

		loading = false;
	}

	$effect(() => {
		const query = searchQuery.toLowerCase().trim();
		if (!query) {
			filteredProjects = projects;
		} else {
			filteredProjects = projects.filter(
				(project) =>
					project.name.toLowerCase().includes(query) ||
					project.key.toLowerCase().includes(query) ||
					project.description?.toLowerCase().includes(query)
			);
		}
	});

	function isFormsProject(project: Project): boolean {
		return project.type_key === 'custom_form';
	}

	function projectHref(project: Project): string {
		const baseHref = `/workspace/${workspaceId}/projects/${project.id}`;
		return isFormsProject(project) ? `${baseHref}/forms` : baseHref;
	}

	function goToProject(project: Project) {
		goto(projectHref(project));
	}

	function openCreateModal() {
		createForm = {
			name: '',
			key: '',
			description: '',
			type_key: projectTypes.some((item) => item.key === 'code_project')
				? 'code_project'
				: (projectTypes[0]?.key ?? 'code_project')
		};
		showCreateModal = true;
	}

	function selectProjectType(typeKey: string) {
		createForm.type_key = typeKey;
	}

	async function handleCreate() {
		if (!createForm.name.trim()) {
			toast.error(get(t)('project.enterName'));
			return;
		}

		if (!createForm.key.trim()) {
			toast.error(get(t)('project.enterKey'));
			return;
		}

		const keyPattern = /^[A-Z0-9]{2,10}$/;
		if (!keyPattern.test(createForm.key.toUpperCase())) {
			toast.error(get(t)('project.keyValidation'));
			return;
		}

		creating = true;
		const response = await projectsApi.create(workspaceId, {
			name: createForm.name.trim(),
			key: createForm.key.toUpperCase(),
			description: createForm.description.trim() || undefined,
			type_key: createForm.type_key || 'code_project'
		});

		if (response.code !== 0) {
			toast.error(response.message);
		} else if (response.data) {
			toast.success(get(t)('project.createSuccess'));
			showCreateModal = false;
			await loadData();
		}

		creating = false;
	}

	async function openEditModal(project: Project, event: Event) {
		event.stopPropagation();
		editingProject = project;
		remoteProject = null;
		projectUnavailable = false;
		editForm = {
			name: project.name,
			description: project.description || '',
			workflow_id: ''
		};
		// Load available workflows for selection
		const wfResponse = await workflowsApi.listByWorkspace(workspaceId);
		if (wfResponse.code === 0 && wfResponse.data) {
			availableWorkflows = wfResponse.data.items ?? [];
		}
		showEditModal = true;
	}

	async function handleUpdate() {
		if (!editingProject || !editForm.name.trim()) {
			toast.error(get(t)('project.enterName'));
			return;
		}

		updating = true;
		const response = await projectsApi.update(editingProject.id, {
			name: editForm.name.trim(),
			description: editForm.description.trim() || undefined,
			expected_updated_at: editingProject.updated_at
		});

		if (response.code !== 0) {
			updating = false;
			if (response.code === 409 || response.error_code === 'version_conflict') {
				await refreshProjects();
				toast.error(get(t)('workbench.conflictBody'));
			} else toast.error(response.message);
			return;
		}

		// Update project workflow if a workflow was selected
		if (editForm.workflow_id !== '') {
			const wfResponse = await workflowsApi.setProjectWorkflow(
				editingProject.id,
				editForm.workflow_id || null
			);
			if (wfResponse.code !== 0) {
				toast.error(wfResponse.message);
				updating = false;
				return;
			}
		}

		toast.success(get(t)('project.updateSuccess'));
		showEditModal = false;
		await loadData();

		updating = false;
	}

	function openDeleteModal(project: Project, event: Event) {
		event.stopPropagation();
		deletingProject = project;
		showDeleteModal = true;
	}

	async function handleDelete() {
		if (!deletingProject) return;

		deleting = true;
		const response = await projectsApi.delete(deletingProject.id);

		if (response.code !== 0) {
			toast.error(response.message);
		} else {
			toast.success(get(t)('project.deleteSuccess'));
			showDeleteModal = false;
			await loadData();
		}

		deleting = false;
	}

	function formatRelativeTime(dateText: string): string {
		return timeAgo(dateText);
	}

	function getIssueCount(
		project: Project,
		state: 'backlog' | 'todo' | 'in_progress' | 'done'
	): number {
		const counts = project.issue_counts;
		if (!counts) return 0;
		return counts.by_state?.[state] ?? 0;
	}

	function getProjectTypeName(typeKey: string): string {
		return projectTypeName(typeKey, projectTypes, $t);
	}

	function projectTypeIcon(typeKey: string) {
		return typeKey === 'code_project' ? Code2 : TableProperties;
	}
</script>

<div class="mx-auto max-w-7xl">
	<div class="mb-6">
		<nav class="mb-4 flex items-center gap-2 text-sm text-fg-muted">
			<a href="/workspace" class="hover:underline">{$t('nav.myWorkspace')}</a>
			<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
				<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7"
				></path>
			</svg>
			<span class="font-medium text-fg"
				>{workspace?.name || $t('common.loading')}</span
			>
		</nav>

		<div class="flex items-center justify-between">
			<div>
				<h1 class="text-2xl font-semibold text-fg">{$t('project.title')}</h1>
				<p class="mt-1 text-sm text-fg-muted">
					{$t('project.count', { values: { count: filteredProjects.length } })}
				</p>
			</div>
			<Button onclick={openCreateModal}>
				<svg class="mr-2 h-5 w-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
					<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4v16m8-8H4"
					></path>
				</svg>
				{$t('project.create')}
			</Button>
		</div>
	</div>

	<div class="mb-6 flex items-center gap-4">
		<div class="relative flex-1">
			<svg
				class="absolute left-3 top-1/2 h-5 w-5 -translate-y-1/2 text-fg-muted"
				fill="none"
				stroke="currentColor"
				viewBox="0 0 24 24"
			>
				<path
					stroke-linecap="round"
					stroke-linejoin="round"
					stroke-width="2"
					d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z"
				></path>
			</svg>
			<input
				type="search"
				placeholder={$t('project.searchPlaceholder')}
				bind:value={searchQuery}
				class="w-full rounded-control border border-input bg-field py-2 pl-10 pr-4 text-fg placeholder:text-fg-muted"
			/>
		</div>
	</div>

	{#if loading}
		<div class="flex items-center justify-center py-12">
			<div class="text-fg-muted">{$t('common.loading')}</div>
		</div>
	{:else if error}
		<div class="rounded-control border bg-danger-soft p-4">
			<p class="text-danger">{error}</p>
		</div>
	{:else if projects.length === 0}
		<EmptyState icon="box" title={$t('project.none')} description={$t('project.noneDesc')}>
			{#snippet action()}
				<Button onclick={openCreateModal}>{$t('project.create')}</Button>
			{/snippet}
		</EmptyState>
	{:else if filteredProjects.length === 0}
		<EmptyState
			icon="search"
			title={$t('project.notFound')}
			description={$t('project.notFoundDesc')}
		/>
	{:else}
		<div class="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-4">
			{#each filteredProjects as project}
				<article
					class="group rounded-card border border-line bg-raised p-5 transition-all hover:-translate-y-0.5 hover:border-line-strong"
				>
					<div class="mb-3 flex items-start justify-between gap-3">
						<div class="min-w-0">
							<button
								type="button"
								onclick={() => goToProject(project)}
								class="truncate text-left text-lg font-semibold text-fg transition-colors hover:underline"
							>
								{project.name}
							</button>
							<div class="mt-1">
								<span
									class="inline-flex items-center rounded-control bg-surface px-2 py-0.5 text-xs font-semibold text-fg"
								>
									{project.key}
								</span>
							</div>
						</div>
						<div class="flex items-center gap-1">
							<button
								onclick={(e) => openEditModal(project, e)}
								class="rounded-control p-1 text-fg-muted transition-colors hover:bg-surface hover:text-fg"
								aria-label={$t('common.edit')}
							>
								<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
									<path
										stroke-linecap="round"
										stroke-linejoin="round"
										stroke-width="2"
										d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z"
									></path>
								</svg>
							</button>
							<button
								onclick={(e) => openDeleteModal(project, e)}
								class="rounded-control p-1 text-fg-muted transition-colors hover:bg-surface hover:text-danger"
								aria-label={$t('common.delete')}
							>
								<svg class="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
									<path
										stroke-linecap="round"
										stroke-linejoin="round"
										stroke-width="2"
										d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16"
									></path>
								</svg>
							</button>
						</div>
					</div>

					<p class="mb-4 min-h-10 text-sm text-fg-muted line-clamp-2">
						{project.description || $t('project.noDescription')}
					</p>

					{#if isFormsProject(project)}
						<div
							class="mb-4 flex min-h-16 items-center gap-3 rounded-card border bg-surface px-3 py-3 text-fg"
						>
							<TableProperties
								class="h-5 w-5 shrink-0 text-fg-muted"
								aria-hidden="true"
							/>
							<div class="min-w-0">
								<p class="text-sm font-semibold">{$t('project.formsProjectHome')}</p>
								<p class="mt-0.5 line-clamp-2 text-xs leading-5 text-fg">
									{$t('project.formsProjectHomeDesc')}
								</p>
							</div>
						</div>
					{:else}
						<div class="mb-4 grid grid-cols-4 gap-2">
							<div
								class="rounded-card bg-surface px-2 py-2 text-center text-fg-muted"
							>
								<div class="text-base font-semibold">{getIssueCount(project, 'backlog')}</div>
								<div class="text-[11px] font-medium">{$t('workflow.backlog')}</div>
							</div>
							<div class="rounded-card bg-surface px-2 py-2 text-center text-fg">
								<div class="text-base font-semibold">{getIssueCount(project, 'todo')}</div>
								<div class="text-[11px] font-medium">{$t('workflow.todo')}</div>
							</div>
							<div class="rounded-card bg-warning-soft px-2 py-2 text-center text-warning">
								<div class="text-base font-semibold">{getIssueCount(project, 'in_progress')}</div>
								<div class="text-[11px] font-medium">{$t('workflow.inProgress')}</div>
							</div>
							<div class="rounded-card bg-success-soft px-2 py-2 text-center text-success">
								<div class="text-base font-semibold">{getIssueCount(project, 'done')}</div>
								<div class="text-[11px] font-medium">{$t('workflow.done')}</div>
							</div>
						</div>
					{/if}

					<div class="flex items-center justify-between gap-3 border-t border-line pt-3">
						<a
							href={projectHref(project)}
							class="inline-flex items-center rounded-control border border-line px-2.5 py-1 text-xs font-medium text-fg transition-colors hover:border-line-strong hover:bg-surface"
						>
							{isFormsProject(project) ? $t('project.openForms') : $t('project.openProject')}
						</a>
						<span class="text-xs text-fg-muted"
							>{$t('project.updatedAt')} {formatRelativeTime(project.updated_at)}</span
						>
					</div>
				</article>
			{/each}
		</div>
	{/if}
</div>

<Modal
	taskScoped
	bind:open={showCreateModal}
	title={$t('project.create')}
	maxWidthClass="max-w-4xl"
>
	<form
		onsubmit={(e) => {
			e.preventDefault();
			handleCreate();
		}}
		class="space-y-5"
	>
		<div class="space-y-2">
			<p class="text-sm font-medium text-fg">
				{$t('project.projectType')}
			</p>
			<div class="grid gap-3 md:grid-cols-2">
				{#each selectableProjectTypes as item (item.key)}
					{@const Icon = projectTypeIcon(item.key)}
					<button
						type="button"
						onclick={() => selectProjectType(item.key)}
						class="min-h-32 rounded-card border p-4 text-left transition-colors {createForm.type_key ===
						item.key
							? 'border-fg bg-surface'
							: 'border-line bg-raised hover:border-line-strong hover:bg-surface'}"
					>
						<div class="flex items-start justify-between gap-3">
							<div class="min-w-0">
								<div class="flex items-center gap-2">
									<Icon class="h-5 w-5 text-fg-muted" />
									<p class="truncate text-sm font-semibold text-fg">
										{getProjectTypeName(item.key)}
									</p>
								</div>
								<p class="mt-3 line-clamp-3 text-xs leading-5 text-fg-muted">
									{projectTypeDescription(item, $t)}
								</p>
							</div>
							{#if createForm.type_key === item.key}
								<CheckCircle2 class="h-5 w-5 shrink-0 text-fg-muted" />
							{/if}
						</div>
					</button>
				{/each}
			</div>
			{#if selectedProjectType}
				<p class="text-xs text-fg-muted">
					{$t('project.projectTypeBoundary')}
				</p>
			{/if}
		</div>

		<Input
			label={$t('project.name')}
			placeholder={$t('project.namePlaceholder')}
			bind:value={createForm.name}
			required
			maxlength={100}
		/>
		<Input
			label={$t('project.key')}
			placeholder={$t('project.keyPlaceholder')}
			bind:value={createForm.key}
			oninput={(e) => {
				const t = e.currentTarget;
				t.value = t.value.toUpperCase();
				createForm.key = t.value;
			}}
			required
			maxlength={10}
			hint={$t('project.keyHint')}
		/>
		<div class="space-y-1">
			<label
				class="block text-sm font-medium text-fg"
				for="create-project-description">{$t('project.description')}</label
			>
			<textarea
				id="create-project-description"
				class="block w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg placeholder:text-fg-muted resize-y"
				placeholder={$t('project.descriptionPlaceholder')}
				bind:value={createForm.description}
				maxlength={350}
				rows="3"></textarea>
			<p class="text-xs text-fg-muted text-right">{createForm.description.length}/350</p>
		</div>
	</form>

	{#snippet footer()}
		<Button variant="secondary" onclick={() => (showCreateModal = false)}
			>{$t('common.cancel')}</Button
		>

		<Button type="submit" loading={creating} onclick={handleCreate}>{$t('common.create')}</Button>
	{/snippet}
</Modal>

<Modal taskScoped bind:open={showEditModal} title={$t('project.editProject')}>
	<form
		onsubmit={(e) => {
			e.preventDefault();
			handleUpdate();
		}}
		class="space-y-4"
	>
		{#if remoteProject && editingProject}
			<section
				class="rounded border bg-warning-soft p-3 text-sm"
				data-testid="task-conflict"
				role="alert"
			>
				<p>{$t('workbench.conflictBody')}</p>
				{#each projectConflicts(editingProject, editForm, remoteProject) as field (field)}<p>
						{field}: {$t('workbench.localValue')}
						{editForm[field]} · {$t('workbench.remoteValue')}
						{remoteProject[field] ?? ''}
					</p>{/each}
				<button
					type="button"
					class="mt-2 min-h-11 underline"
					data-testid="task-conflict-review"
					onclick={reviewProject}>{$t('workbench.reviewRetry')}</button
				>
			</section>
		{/if}
		{#if projectUnavailable}<p role="alert" data-testid="task-resource-unavailable">
				{$t('workbench.resourceUnavailable')}
			</p>{/if}
		<Input
			label={$t('project.name')}
			placeholder={$t('project.namePlaceholder')}
			bind:value={editForm.name}
			required
			maxlength={100}
		/>
		<div class="space-y-1">
			<label
				class="block text-sm font-medium text-fg"
				for="edit-project-description">{$t('project.description')}</label
			>
			<textarea
				id="edit-project-description"
				class="block w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg placeholder:text-fg-muted resize-y"
				placeholder={$t('project.descriptionPlaceholder')}
				bind:value={editForm.description}
				maxlength={350}
				rows="3"></textarea>
			<p class="text-xs text-fg-muted text-right">{editForm.description.length}/350</p>
		</div>
		<div class="space-y-1">
			<label
				class="block text-sm font-medium text-fg"
				for="edit-project-workflow"
			>
				Workflow
			</label>
			<select
				id="edit-project-workflow"
				bind:value={editForm.workflow_id}
				class="block w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
			>
				<option value="">{$t('workflow.useWorkspaceDefault')}</option>
				{#each availableWorkflows as wf (wf.id)}
					<option value={wf.id}>{wf.name}{wf.is_system_default ? ' (System)' : ''}</option>
				{/each}
			</select>
		</div>
		<div class="text-sm text-fg-muted">
			<strong>{$t('project.note')}:</strong>
			{$t('project.keyImmutable', { values: { key: editingProject?.key } })}
		</div>
	</form>

	{#snippet footer()}
		<Button variant="secondary" onclick={() => (showEditModal = false)}
			>{$t('common.cancel')}</Button
		>

		<Button
			type="submit"
			loading={updating}
			disabled={!!remoteProject || projectUnavailable}
			onclick={handleUpdate}>{$t('common.save')}</Button
		>
	{/snippet}
</Modal>

<Modal bind:open={showDeleteModal} title={$t('project.deleteProject')}>
	<div class="space-y-4">
		<p class="text-fg">
			{$t('project.deleteConfirm', {
				values: { name: deletingProject?.name, key: deletingProject?.key }
			})}
		</p>
		<div class="rounded-control border bg-danger-soft p-4">
			<p class="inline-flex items-start gap-2 text-sm text-danger">
				<svg class="mt-0.5 h-4 w-4 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
					<path
						stroke-linecap="round"
						stroke-linejoin="round"
						stroke-width="2"
						d="M12 9v3m0 4h.01M4.93 19h14.14a2 2 0 001.73-3L13.73 3a2 2 0 00-3.46 0L3.2 16a2 2 0 001.73 3z"
					></path>
				</svg>
				<span>{$t('project.deleteWarning')}</span>
			</p>
		</div>
	</div>

	{#snippet footer()}
		<Button variant="secondary" onclick={() => (showDeleteModal = false)}
			>{$t('common.cancel')}</Button
		>

		<Button variant="danger" loading={deleting} onclick={handleDelete}
			>{$t('workspace.confirmDelete')}</Button
		>
	{/snippet}
</Modal>
