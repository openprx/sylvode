<script lang="ts">
	import ProjectPluginSettingsLink from '#lib/components/plugins/ProjectPluginSettingsLink.svelte';
	import ProjectPluginNav from '#lib/components/plugins/ProjectPluginNav.svelte';
	import FlowSharedBlockList from '#lib/components/flow/FlowSharedBlockList.svelte';
	import { FileText, Power } from '@lucide/svelte';
	import { onMount, onDestroy, getContext } from 'svelte';
	import { TaskWorkbench, WORKBENCH_CONTEXT } from '#lib/workbench/tabs.js';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import {
		projectsApi,
		type Project,
		type ProjectResource,
		type ProjectResourceKind
	} from '#lib/api/projects.js';
	import { issuesApi, type Issue } from '#lib/api/issues.js';
	import { aiAgentsApi, type AiAgent } from '#lib/api/ai-agents.js';
	import { importExportApi } from '#lib/api/import-export.js';
	import { toast } from '#lib/stores/toast.js';
	import Button from '#lib/components/Button.svelte';
	import { requireRouteParam } from '#lib/utils/route-params.js';
	import {
		getProjectScenarioTemplate,
		getScenarioFields,
		scenarioItemLabel,
		templateArray
	} from '#lib/utils/scenario-template.js';
	import {
		scenarioTemplateAudience,
		scenarioTemplateIndustry,
		scenarioTemplateName
	} from '#lib/utils/project-type-i18n.js';

	const workspaceId = requireRouteParam(page.params.workspaceId, 'workspaceId');
	const projectId = requireRouteParam(page.params.projectId, 'projectId');
	const locatorPlaceholder = 'Locator, e.g. {"url":"https://..."} or /path/to/directory';

	let project = $state<Project | null>(null);
	let resources = $state<ProjectResource[]>([]);
	let recentIssues = $state<Issue[]>([]);
	let loading = $state(true);
	let exporting = $state(false);
	let importing = $state(false);
	let resourceSubmitting = $state(false);
	let aiAgents = $state<AiAgent[]>([]);
	let assistantConfigDrafts = $state<
		Record<string, { provider: string; model: string; api_endpoint: string }>
	>({});
	let updatingAssistantId = $state<string | null>(null);
	let deletingResourceId = $state<string | null>(null);
	let editingResourceId = $state<string | null>(null);
	let resourceEditDrafts = $state<
		Record<
			string,
			{ kind: ProjectResourceKind; name: string; locator: string; sync_status: string }
		>
	>({});
	let resourceForm = $state({
		kind: 'directory' as ProjectResourceKind,
		name: '',
		locator: ''
	});

	const scenarioTemplate = $derived(getProjectScenarioTemplate(project));
	const scenarioFields = $derived(getScenarioFields(scenarioTemplate));
	const scenarioWorkflowStates = $derived(
		templateArray(scenarioTemplate, 'workflow_template', 'states')
	);
	const scenarioRequiredResources = $derived(
		templateArray(scenarioTemplate, 'resource_schema', 'resources')
	);

	const workbench = getContext<TaskWorkbench | undefined>(WORKBENCH_CONTEXT);
	const taskKey = `project:${projectId}`;
	let unregisterTask: () => void = () => {};
	let remoteDetail = $state(false);
	let resourceBases = $state<Record<string, ProjectResource>>({});
	let assistantBases = $state<Record<string, AiAgent>>({});
	let assistantBaseDrafts = $state<
		Record<string, { provider: string; model: string; api_endpoint: string }>
	>({});
	let resourceConflicts = $state<Record<string, ProjectResource>>({});
	let assistantConflicts = $state<Record<string, AiAgent>>({});
	type DetailSnapshot = {
		assistant: typeof assistantConfigDrafts;
		assistantBase: typeof assistantBases;
		assistantUiBase: typeof assistantBaseDrafts;
		resourceBase: typeof resourceBases;
		resourceEdit: typeof resourceEditDrafts;
		resourceForm: typeof resourceForm;
		editing: string | null;
	};
	function detailDirty() {
		return (
			!!editingResourceId ||
			!!resourceForm.name ||
			!!resourceForm.locator ||
			Object.entries(assistantConfigDrafts).some(
				([id, draft]) => JSON.stringify(draft) !== JSON.stringify(assistantBaseDrafts[id])
			)
		);
	}
	function captureDetail(): DetailSnapshot {
		return {
			assistant: $state.snapshot(assistantConfigDrafts),
			assistantBase: $state.snapshot(assistantBases),
			assistantUiBase: $state.snapshot(assistantBaseDrafts),
			resourceBase: $state.snapshot(resourceBases),
			resourceEdit: $state.snapshot(resourceEditDrafts),
			resourceForm: $state.snapshot(resourceForm),
			editing: editingResourceId
		};
	}
	async function refreshDetail(force = false) {
		if (!force && (loading || resourceSubmitting || updatingAssistantId)) return;
		const [projectResponse, resourceResponse, agentResponse] = await Promise.all([
			projectsApi.get(projectId),
			projectsApi.listResources(projectId),
			aiAgentsApi.list(projectId)
		]);
		if (projectResponse.code !== 0 || !projectResponse.data) return;
		project = projectResponse.data;
		if (resourceResponse.code === 0 && resourceResponse.data) {
			const fresh = resourceResponse.data.items ?? [];
			for (const [id, base] of Object.entries(resourceBases)) {
				const remote = fresh.find((row) => row.id === id);
				if (remote && resourceEditDrafts[id] && remote.updated_at !== base.updated_at)
					resourceConflicts = { ...resourceConflicts, [id]: remote };
			}
			resources = fresh;
		}
		if (agentResponse.code === 0 && agentResponse.data) {
			const fresh = agentResponse.data.items ?? [];
			for (const [id, base] of Object.entries(assistantBases)) {
				const remote = fresh.find((row) => row.id === id);
				if (
					remote &&
					assistantConfigDrafts[id] &&
					remote.config_updated_at !== base.config_updated_at
				)
					assistantConflicts = { ...assistantConflicts, [id]: remote };
			}
			aiAgents = fresh;
		}
		remoteDetail =
			Object.keys(resourceConflicts).length > 0 || Object.keys(assistantConflicts).length > 0;
		if (remoteDetail) workbench?.markChanged(taskKey);
		await loadRecentIssues();
	}
	function reviewResource(id: string) {
		const base = resourceBases[id],
			remote = resourceConflicts[id],
			local = resourceEditDrafts[id];
		if (!base || !remote || !local) return;
		resourceEditDrafts = {
			...resourceEditDrafts,
			[id]: {
				kind: local.kind !== base.kind ? local.kind : remote.kind,
				name: local.name !== base.name ? local.name : remote.name,
				locator:
					local.locator !== formatLocator(base.locator)
						? local.locator
						: formatLocator(remote.locator),
				sync_status: local.sync_status !== base.sync_status ? local.sync_status : remote.sync_status
			}
		};
		resourceBases = { ...resourceBases, [id]: remote };
		const next = { ...resourceConflicts };
		delete next[id];
		resourceConflicts = next;
		remoteDetail = Object.keys(next).length > 0 || Object.keys(assistantConflicts).length > 0;
		workbench?.markChanged(taskKey, remoteDetail);
	}
	function reviewAssistant(id: string) {
		const remote = assistantConflicts[id],
			base = assistantBaseDrafts[id],
			local = assistantConfigDrafts[id];
		if (!remote || !base || !local) return;
		const current = {
			provider: remote.provider,
			model: remote.model,
			api_endpoint: remote.api_endpoint ?? ''
		};
		const next = { ...local };
		for (const field of ['provider', 'model', 'api_endpoint'] as const)
			if (local[field] === base[field]) next[field] = current[field];
		assistantConfigDrafts = { ...assistantConfigDrafts, [id]: next };
		assistantBases = { ...assistantBases, [id]: remote };
		assistantBaseDrafts = { ...assistantBaseDrafts, [id]: current };
		const conflicts = { ...assistantConflicts };
		delete conflicts[id];
		assistantConflicts = conflicts;
		remoteDetail = Object.keys(conflicts).length > 0 || Object.keys(resourceConflicts).length > 0;
		workbench?.markChanged(taskKey, remoteDetail);
	}
	$effect(() => {
		void resourceForm.name;
		void resourceForm.locator;
		void editingResourceId;
		void assistantConfigDrafts;
		workbench?.markDirty(taskKey, detailDirty());
	});

	onDestroy(() => unregisterTask());

	onMount(async () => {
		const loadedProject = await loadProject();
		if (!loadedProject) {
			loading = false;
			return;
		}

		if (loadedProject.type_key === 'custom_form') {
			workbench?.close(taskKey, true);
			await goto(`/workspace/${workspaceId}/projects/${projectId}/forms`, { replaceState: true });
			return;
		}

		await Promise.all([loadRecentIssues(), loadResources(), loadAiAgents()]);
		const saved = workbench?.snapshot<DetailSnapshot>(taskKey);
		if (saved) {
			assistantConfigDrafts = saved.assistant;
			assistantBases = saved.assistantBase;
			assistantBaseDrafts = saved.assistantUiBase;
			resourceBases = saved.resourceBase;
			resourceEditDrafts = saved.resourceEdit;
			resourceForm = saved.resourceForm;
			editingResourceId = saved.editing;
		}
		workbench?.setTitle(taskKey, loadedProject.name);
		unregisterTask =
			workbench?.register(taskKey, {
				capture: captureDetail,
				dirty: detailDirty,
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

	async function loadResources() {
		const response = await projectsApi.listResources(projectId);
		if (response.code === 0 && response.data) {
			resources = response.data.items ?? [];
		}
	}

	async function loadAiAgents() {
		const response = await aiAgentsApi.list(projectId);
		if (response.code === 0 && response.data) {
			aiAgents = response.data.items ?? [];
		}
	}

	function isRecord(value: unknown): value is Record<string, unknown> {
		return typeof value === 'object' && value !== null && !Array.isArray(value);
	}

	function scenarioValue(item: unknown, key: string, fallback = ''): string {
		if (!isRecord(item)) return fallback;
		const value = item[key];
		return typeof value === 'string' && value.trim() ? value.trim() : fallback;
	}

	function scenarioArrayValue(item: unknown, key: string): string[] {
		if (!isRecord(item) || !Array.isArray(item[key])) return [];
		return item[key].filter(
			(value): value is string => typeof value === 'string' && value.trim().length > 0
		);
	}

	function scenarioBooleanValue(item: unknown, key: string): boolean {
		return isRecord(item) ? Boolean(item[key]) : false;
	}

	function eventInputValue(event: Event): string {
		return event.currentTarget instanceof HTMLInputElement ? event.currentTarget.value : '';
	}

	function assistantRoles(): unknown[] {
		return scenarioTemplate?.ai_roles ?? [];
	}

	function assistantForRole(role: unknown): AiAgent | undefined {
		const roleKey = scenarioValue(role, 'key');
		const roleLabel = scenarioItemLabel(role, roleKey);
		return aiAgents.find((agent) => {
			const roleMeta = agent.domain_overrides?.role;
			const metaKey = isRecord(roleMeta) && typeof roleMeta.key === 'string' ? roleMeta.key : '';
			return metaKey === roleKey || agent.name === roleLabel || agent.id.endsWith(roleKey);
		});
	}

	function assistantProviderForRole(role: unknown): string {
		return scenarioValue(role, 'provider', scenarioValue(role, 'agent_type', 'assistant'));
	}

	function assistantModelForRole(role: unknown): string {
		return scenarioValue(role, 'model', 'configured-by-operator');
	}

	function assistantDraft(assistant: AiAgent, role: unknown) {
		return (
			assistantConfigDrafts[assistant.id] ?? {
				provider: assistant.provider || assistantProviderForRole(role),
				model:
					assistant.model === 'template-placeholder'
						? assistantModelForRole(role)
						: assistant.model || assistantModelForRole(role),
				api_endpoint: assistant.api_endpoint ?? scenarioValue(role, 'api_endpoint')
			}
		);
	}

	function setAssistantDraft(
		assistant: AiAgent,
		field: 'provider' | 'model' | 'api_endpoint',
		value: string,
		role: unknown
	) {
		if (!assistantBases[assistant.id]) {
			assistantBases = { ...assistantBases, [assistant.id]: $state.snapshot(assistant) };
			assistantBaseDrafts = {
				...assistantBaseDrafts,
				[assistant.id]: assistantDraft(assistant, role)
			};
		}
		assistantConfigDrafts = {
			...assistantConfigDrafts,
			[assistant.id]: {
				...assistantDraft(assistant, role),
				[field]: value
			}
		};
	}

	async function saveAssistantConfig(role: unknown) {
		const assistant = assistantForRole(role);
		if (!assistant) return;
		const draft = assistantDraft(assistant, role);
		const provider = draft.provider.trim();
		const model = draft.model.trim();
		if (!provider || !model) {
			toast.error(get(t)('project.assistantConfigRequired'));
			return;
		}
		updatingAssistantId = assistant.id;
		const response = await aiAgentsApi.update(projectId, assistant.id, {
			provider,
			model,
			api_endpoint: draft.api_endpoint.trim() || undefined,
			expected_config_updated_at:
				assistantBases[assistant.id]?.config_updated_at ?? assistant.config_updated_at
		});
		updatingAssistantId = null;
		if (response.code !== 0) {
			if (response.code === 409) {
				await refreshDetail(true);
				toast.error(get(t)('workbench.conflictBody'));
			} else toast.error(response.message);
			return;
		}
		const clean = { ...assistantConfigDrafts };
		delete clean[assistant.id];
		assistantConfigDrafts = clean;
		const bases = { ...assistantBases };
		delete bases[assistant.id];
		assistantBases = bases;

		toast.success(get(t)('project.assistantSaved'));
		await loadAiAgents();
	}

	function parseLocator(raw: string): Record<string, unknown> {
		const trimmed = raw.trim();
		if (!trimmed) {
			return {};
		}
		try {
			const parsed = JSON.parse(trimmed) as unknown;
			if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
				return parsed as Record<string, unknown>;
			}
			throw new Error('locator must be an object');
		} catch {
			return { value: trimmed };
		}
	}

	function formatLocator(locator: Record<string, unknown>): string {
		const keys = Object.keys(locator ?? {});
		if (keys.length === 0) {
			return $t('resources.noLocator');
		}
		if (typeof locator.url === 'string') return locator.url;
		if (typeof locator.path === 'string') return locator.path;
		if (typeof locator.value === 'string') return locator.value;
		return JSON.stringify(locator);
	}

	function resourceKindLabel(kind: ProjectResourceKind): string {
		const labels: Record<ProjectResourceKind, string> = {
			directory: $t('resources.kindDirectory'),
			custom: $t('resources.kindCustom')
		};
		return labels[kind] ?? kind;
	}

	function resourceDraft(resource: ProjectResource) {
		return (
			resourceEditDrafts[resource.id] ?? {
				kind: resource.kind,
				name: resource.name,
				locator: formatLocator(resource.locator),
				sync_status: resource.sync_status
			}
		);
	}

	function setResourceDraft(
		resource: ProjectResource,
		field: 'kind' | 'name' | 'locator' | 'sync_status',
		value: string
	) {
		resourceEditDrafts = {
			...resourceEditDrafts,
			[resource.id]: {
				...resourceDraft(resource),
				[field]: field === 'kind' ? (value as ProjectResourceKind) : value
			}
		};
	}

	function startEditResource(resource: ProjectResource) {
		resourceBases = { ...resourceBases, [resource.id]: $state.snapshot(resource) };
		editingResourceId = resource.id;
		resourceEditDrafts = {
			...resourceEditDrafts,
			[resource.id]: {
				kind: resource.kind,
				name: resource.name,
				locator: formatLocator(resource.locator),
				sync_status: resource.sync_status
			}
		};
	}

	async function handleUpdateResource(resource: ProjectResource) {
		const draft = resourceDraft(resource);
		if (!draft.name.trim()) {
			toast.error('Resource name is required');
			return;
		}
		resourceSubmitting = true;
		const response = await projectsApi.updateResource(projectId, resource.id, {
			kind: draft.kind,
			name: draft.name.trim(),
			locator: parseLocator(draft.locator),
			sync_status: draft.sync_status.trim() || 'manual',
			expected_updated_at: resourceBases[resource.id]?.updated_at ?? resource.updated_at
		});
		resourceSubmitting = false;
		if (response.code !== 0) {
			if (response.code === 409) {
				await refreshDetail(true);
				toast.error(get(t)('workbench.conflictBody'));
			} else toast.error(response.message);
			return;
		}

		if (response.data) {
			resources = resources.map((item) => (item.id === response.data?.id ? response.data : item));
		}
		editingResourceId = null;
		const bases = { ...resourceBases };
		delete bases[resource.id];
		resourceBases = bases;
		toast.success('Project resource updated');
	}

	async function handleCreateResource() {
		if (!resourceForm.name.trim()) {
			toast.error('Resource name is required');
			return;
		}

		resourceSubmitting = true;
		const response = await projectsApi.createResource(projectId, {
			kind: resourceForm.kind,
			name: resourceForm.name.trim(),
			locator: parseLocator(resourceForm.locator)
		});
		resourceSubmitting = false;

		if (response.code !== 0) {
			toast.error(response.message);
			return;
		}

		resourceForm = { kind: 'directory', name: '', locator: '' };
		toast.success('Project resource added');
		await loadResources();
	}

	async function handleDeleteResource(resource: ProjectResource) {
		deletingResourceId = resource.id;
		const response = await projectsApi.deleteResource(projectId, resource.id);
		deletingResourceId = null;

		if (response.code !== 0) {
			toast.error(response.message);
			return;
		}

		resources = resources.filter((item) => item.id !== resource.id);
		toast.success('Project resource deleted');
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

{#each Object.values(resourceConflicts) as remote (remote.id)}<section
		class="rounded border p-3"
		role="alert"
		data-testid="task-conflict"
	>
		<p>{$t('workbench.conflictBody')}</p>
		<p>{$t('workbench.remoteValue')} {remote.name} · {formatLocator(remote.locator)}</p>
		<button type="button" class="min-h-11 underline" onclick={() => reviewResource(remote.id)}
			>{$t('workbench.reviewRetry')}</button
		>
	</section>{/each}
{#each Object.values(assistantConflicts) as remote (remote.id)}<section
		class="rounded border p-3"
		role="alert"
		data-testid="task-conflict"
	>
		<p>{$t('workbench.conflictBody')}</p>
		<p>{$t('workbench.remoteValue')} {remote.provider} · {remote.model}</p>
		<button type="button" class="min-h-11 underline" onclick={() => reviewAssistant(remote.id)}
			>{$t('workbench.reviewRetry')}</button
		>
	</section>{/each}

{#if remoteDetail}<p
		class="rounded border bg-warning-soft p-3 text-sm"
		role="status"
		data-testid="task-remote-change"
	>
		{$t('workbench.conflictBody')}
	</p>{/if}

<div class="max-w-7xl mx-auto">
	{#if loading}
		<div class="flex items-center justify-center py-12">
			<div class="text-fg-muted">{$t('common.loading')}</div>
		</div>
	{:else if project}
		<div class="mb-6 flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
			<div>
				<div class="mb-2 flex items-center gap-3">
					<span
						class="rounded bg-surface px-2 py-1 text-sm text-fg-muted"
						>{project.key}</span
					>
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
			<section
				class="rounded-card border border-line bg-raised p-5"
			>
				<div class="mb-4">
					<h2 class="text-lg font-semibold text-fg">
						{$t('project.projectManagement')}
					</h2>
					<p class="mt-1 text-sm text-fg-muted">
						{$t('project.projectManagementDesc')}
					</p>
				</div>
				<div class="grid gap-3 md:grid-cols-3">
					<a
						href={`/workspace/${workspaceId}/projects/${projectId}/issues`}
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
						href={`/workspace/${workspaceId}/projects/${projectId}/board`}
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
						href={`/workspace/${workspaceId}/projects/${projectId}/cycles`}
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

			<section
				class="rounded-card border border-line bg-raised p-5"
			>
				<div class="mb-4">
					<h2 class="text-lg font-semibold text-fg">
						{$t('project.businessData')}
					</h2>
					<p class="mt-1 text-sm text-fg-muted">
						{$t('project.businessDataDesc')}
					</p>
				</div>
				<a
					href={`/workspace/${workspaceId}/projects/${projectId}/forms`}
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
			<div
				class="mb-8 rounded-card border border-line bg-raised p-6"
			>
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
						href={`/workspace/${workspaceId}/projects/${projectId}/issues`}
						class="inline-flex items-center justify-center rounded-control border border-line px-3 py-2 text-sm font-medium text-fg transition-colors hover:border-line-strong hover:bg-surface"
					>
						{$t('project.openScenarioWork')}
					</a>
				</div>

				<div class="grid gap-3 md:grid-cols-4">
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
					<div class="rounded-card bg-surface p-4">
						<p class="text-xs font-medium uppercase text-fg-muted">
							{$t('project.resources')}
						</p>
						<p class="mt-2 text-2xl font-semibold text-fg">
							{scenarioRequiredResources.length}
						</p>
					</div>
					<div class="rounded-card bg-surface p-4">
						<p class="text-xs font-medium uppercase text-fg-muted">
							{$t('project.assistants')}
						</p>
						<p class="mt-2 text-2xl font-semibold text-fg">
							{scenarioTemplate.ai_roles.length}
						</p>
					</div>
				</div>

				<div class="mt-5 grid gap-5 lg:grid-cols-3">
					<div>
						<h3 class="mb-2 text-sm font-semibold text-fg">
							{$t('project.scenarioFields')}
						</h3>
						<div class="space-y-2">
							{#each scenarioFields.slice(0, 5) as field}
								<div
									class="flex items-center justify-between rounded-control border border-line px-3 py-2 text-sm"
								>
									<span class="text-fg">{field.label}</span>
									<span class="text-xs text-fg-muted">{field.type}</span>
								</div>
							{/each}
						</div>
					</div>
					<div>
						<h3 class="mb-2 text-sm font-semibold text-fg">
							{$t('project.requiredResources')}
						</h3>
						<div class="space-y-2">
							{#each scenarioRequiredResources.slice(0, 5) as item, index}
								<div
									class="rounded-control border border-line px-3 py-2 text-sm text-fg"
								>
									{scenarioItemLabel(item, `Resource ${index + 1}`)}
								</div>
							{/each}
						</div>
					</div>
				</div>
			</div>

			<div
				class="mb-8 rounded-card border border-line bg-raised p-6"
			>
				<div class="mb-5 flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
					<div>
						<h2 class="text-lg font-semibold text-fg">
							{$t('project.scenarioSetup')}
						</h2>
						<p class="mt-1 text-sm text-fg-muted">
							{scenarioTemplateAudience(scenarioTemplate.audience_label, scenarioTemplate.key, $t)}
						</p>
					</div>
				</div>

				<div class="grid gap-5">
					<div>
						<h3 class="mb-3 text-sm font-semibold text-fg">
							{$t('project.assistantSetup')}
						</h3>
						{#if assistantRoles().length === 0}
							<p
								class="rounded-control border border-dashed border-input p-4 text-sm text-fg-muted"
							>
								{$t('project.noAssistantRoles')}
							</p>
						{:else}
							<div class="space-y-3">
								{#each assistantRoles() as role (scenarioValue(role, 'key', scenarioItemLabel(role, 'assistant')))}
									{@const assistant = assistantForRole(role)}
									<div class="rounded-control border border-line p-4">
										<div class="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
											<div class="min-w-0">
												<div class="flex flex-wrap items-center gap-2">
													<h4 class="text-sm font-semibold text-fg">
														{scenarioItemLabel(role, $t('project.assistant'))}
													</h4>
													<span
														class="rounded bg-surface px-2 py-0.5 text-xs text-fg-muted"
														>{scenarioValue(role, 'agent_type', 'assistant')}</span
													>
													<span
														class="rounded px-2 py-0.5 text-xs font-medium {assistant?.is_active
															? 'bg-success-soft text-success'
															: 'bg-warning-soft text-warning'}"
													>
														{assistant?.is_active ? $t('project.active') : $t('project.needsSetup')}
													</span>
												</div>
												<div class="mt-2 flex flex-wrap gap-1.5">
													{#each scenarioArrayValue(role, 'capabilities') as capability}
														<span
															class="rounded border border-line bg-surface px-1.5 py-0.5 font-mono text-xs text-fg-muted"
															>{capability}</span
														>
													{/each}
												</div>
												{#if scenarioBooleanValue(role, 'writes_require_approval')}
													<p class="mt-2 text-xs text-fg-muted">
														{$t('project.writesRequireApproval')}
													</p>
												{/if}
												{#if assistant}
													{@const draft = assistantDraft(assistant, role)}
													<div class="mt-3 grid gap-2 sm:grid-cols-3">
														<label
															class="block text-xs font-medium text-fg-muted"
														>
															{$t('project.provider')}
															<input
																class="mt-1 block w-full rounded-control border border-input bg-field px-2 py-1.5 text-sm text-fg"
																value={draft.provider}
																placeholder={assistantProviderForRole(role)}
																oninput={(event) =>
																	setAssistantDraft(
																		assistant,
																		'provider',
																		eventInputValue(event),
																		role
																	)}
															/>
														</label>
														<label
															class="block text-xs font-medium text-fg-muted"
														>
															{$t('project.model')}
															<input
																class="mt-1 block w-full rounded-control border border-input bg-field px-2 py-1.5 text-sm text-fg"
																value={draft.model}
																placeholder={assistantModelForRole(role)}
																oninput={(event) =>
																	setAssistantDraft(
																		assistant,
																		'model',
																		eventInputValue(event),
																		role
																	)}
															/>
														</label>
														<label
															class="block text-xs font-medium text-fg-muted"
														>
															{$t('project.apiEndpoint')}
															<input
																class="mt-1 block w-full rounded-control border border-input bg-field px-2 py-1.5 text-sm text-fg"
																value={draft.api_endpoint}
																placeholder="https://..."
																oninput={(event) =>
																	setAssistantDraft(
																		assistant,
																		'api_endpoint',
																		eventInputValue(event),
																		role
																	)}
															/>
														</label>
													</div>
												{/if}
											</div>
											<Button
												size="sm"
												variant={assistant?.is_active ? 'secondary' : 'primary'}
												disabled={!assistant || updatingAssistantId === assistant.id}
												onclick={() => saveAssistantConfig(role)}
											>
												<Power class="mr-2 h-4 w-4" aria-hidden="true" />
												{assistant?.is_active
													? $t('project.saveAssistant')
													: $t('project.enableAssistant')}
											</Button>
										</div>
									</div>
								{/each}
							</div>
						{/if}
					</div>
				</div>
			</div>
		{/if}

		<div
			class="mb-8 rounded-card border border-line bg-raised p-6"
		>
			<div class="mb-4 flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
				<div>
					<h2 class="text-lg font-semibold text-fg">
						{$t('resources.title')}
					</h2>
					<p class="text-sm text-fg-muted">
						{$t('resources.description')}
					</p>
				</div>
			</div>

			<div class="mb-5 grid grid-cols-1 gap-3 md:grid-cols-[160px_1fr_1.3fr_auto]">
				<select
					bind:value={resourceForm.kind}
					class="rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
					aria-label={$t('resources.resourceKind')}
				>
					<option value="directory">{$t('resources.kindDirectory')}</option>
					<option value="custom">{$t('resources.kindCustom')}</option>
				</select>
				<input
					bind:value={resourceForm.name}
					placeholder={$t('resources.name')}
					class="rounded-control border border-input bg-field px-3 py-2 text-sm text-fg placeholder:text-fg-muted"
				/>
				<input
					bind:value={resourceForm.locator}
					placeholder={locatorPlaceholder}
					class="rounded-control border border-input bg-field px-3 py-2 text-sm text-fg placeholder:text-fg-muted"
				/>
				<Button loading={resourceSubmitting} onclick={handleCreateResource}
					>{$t('resources.add')}</Button
				>
			</div>

			{#if resources.length === 0}
				<p
					class="rounded-control border border-dashed border-input p-4 text-sm text-fg-muted"
				>
					{$t('resources.empty')}
				</p>
			{:else}
				<div class="overflow-hidden rounded-control border border-line">
					<table class="min-w-full divide-y divide-line">
						<thead class="bg-nav">
							<tr>
								<th
									class="px-4 py-2 text-left text-xs font-medium uppercase text-fg-muted"
									>{$t('resources.kind')}</th
								>
								<th
									class="px-4 py-2 text-left text-xs font-medium uppercase text-fg-muted"
									>{$t('resources.name')}</th
								>
								<th
									class="px-4 py-2 text-left text-xs font-medium uppercase text-fg-muted"
									>{$t('resources.locator')}</th
								>
								<th
									class="px-4 py-2 text-right text-xs font-medium uppercase text-fg-muted"
									>{$t('common.actions')}</th
								>
							</tr>
						</thead>
						<tbody class="divide-y divide-line">
							{#each resources as resource (resource.id)}
								{@const editingResource = editingResourceId === resource.id}
								{@const draft = resourceDraft(resource)}
								<tr>
									<td class="px-4 py-3 text-sm text-fg-muted">
										{#if editingResource}
											<select
												value={draft.kind}
												onchange={(event) =>
													setResourceDraft(resource, 'kind', event.currentTarget.value)}
												class="w-full rounded-control border border-input bg-field px-2 py-1.5 text-sm text-fg"
											>
												<option value="directory">{$t('resources.kindDirectory')}</option>
												<option value="custom">{$t('resources.kindCustom')}</option>
											</select>
										{:else}
											{resourceKindLabel(resource.kind)}
										{/if}
									</td>
									<td class="px-4 py-3 text-sm font-medium text-fg">
										{#if editingResource}
											<input
												value={draft.name}
												oninput={(event) =>
													setResourceDraft(resource, 'name', event.currentTarget.value)}
												class="w-full rounded-control border border-input bg-field px-2 py-1.5 text-sm text-fg"
											/>
										{:else}
											{resource.name}
										{/if}
									</td>
									<td class="max-w-md px-4 py-3 text-sm text-fg-muted">
										{#if editingResource}
											<input
												value={draft.locator}
												oninput={(event) =>
													setResourceDraft(resource, 'locator', event.currentTarget.value)}
												class="w-full rounded-control border border-input bg-field px-2 py-1.5 text-sm text-fg"
											/>
										{:else}
											<span class="block truncate">{formatLocator(resource.locator)}</span>
										{/if}
									</td>
									<td class="px-4 py-3 text-right">
										{#if editingResource}
											<div class="flex justify-end gap-2">
												<Button
													size="sm"
													variant="secondary"
													onclick={() => (editingResourceId = null)}>{$t('common.cancel')}</Button
												>
												<Button
													size="sm"
													loading={resourceSubmitting}
													disabled={!!resourceConflicts[resource.id]}
													onclick={() => handleUpdateResource(resource)}>{$t('common.save')}</Button
												>
											</div>
										{:else}
											<div class="flex justify-end gap-3">
												<button
													type="button"
													onclick={() => startEditResource(resource)}
													class="text-sm text-fg underline"
												>
													{$t('common.edit')}
												</button>
												<button
													type="button"
													disabled={deletingResourceId === resource.id}
													onclick={() => handleDeleteResource(resource)}
													class="text-sm text-danger disabled:cursor-not-allowed disabled:text-control"
												>
													{$t('common.delete')}
												</button>
											</div>
										{/if}
									</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			{/if}
		</div>

		<div
			class="rounded-card border border-line bg-raised p-6"
		>
			<h2 class="mb-4 text-lg font-semibold text-fg">
				{$t('project.recentUpdates')}
			</h2>
			{#if recentIssues.length === 0}
				<p class="text-fg-muted">{$t('project.noRecentWorkItems')}</p>
			{:else}
				<div class="space-y-3">
					{#each recentIssues as issue}
						<a
							href={`/workspace/${workspaceId}/projects/${projectId}/issues/${issue.id}`}
							class="block rounded-control p-3 hover:bg-surface"
						>
							<div class="space-y-1.5">
								<div class="flex items-center gap-2">
									<span class="font-mono text-sm text-fg-muted"
										>{issue.key}</span
									>

									<span class="text-sm font-medium text-fg"
										>{issue.title}</span
									>

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
										{#each issue.labels as label}
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
