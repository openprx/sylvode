<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { page } from '$app/state';
	import { projectsApi, type Project } from '#lib/api/projects.js';
	import { membersApi } from '#lib/api/members.js';
	import { issuesApi, type Issue, type IssuePriority, type IssueStatus } from '#lib/api/issues.js';
	import { workflowsApi, type WorkflowStateDef } from '#lib/api/workflows.js';
	import { sprintsApi, type Sprint } from '#lib/api/sprints.js';
	import { toast } from '#lib/stores/toast.js';
	import { requireRouteParam } from '#lib/utils/route-params.js';
	import Input from '#lib/components/Input.svelte';
	import Button from '#lib/components/Button.svelte';
	import Badge from '#lib/components/Badge.svelte';
	import Avatar from '#lib/components/Avatar.svelte';
	import BotIcon from '#lib/components/BotIcon.svelte';
	import IssueCreateModal from '#lib/components/IssueCreateModal.svelte';
	import { getProjectScenarioTemplate } from '#lib/utils/scenario-template.js';

	const workspaceId = $derived(requireRouteParam(page.params.workspaceId, 'workspaceId'));
	const projectId = $derived(requireRouteParam(page.params.projectId, 'projectId'));

	let workflowStates = $state<WorkflowStateDef[]>([]);
	let project = $state<Project | null>(null);
	const columns = $derived.by(() => workflowStates.map((state) => state.key as IssueStatus));

	function getColumnName(status: IssueStatus): string {
		const stateDef = workflowStates.find((s) => s.key === status);
		if (stateDef?.display_name) return stateDef.display_name;
		const keys: Record<string, string> = {
			backlog: 'issue.backlog',
			todo: 'issue.todo',
			in_progress: 'issue.inProgress',
			done: 'issue.done'
		};
		return keys[status] ? get(t)(keys[status]) : status;
	}

	let issues = $state<Issue[]>([]);
	let sprints = $state<Sprint[]>([]);
	let memberNameMap = $state<Record<string, string>>({});
	let memberEntityTypeMap = $state<Record<string, string>>({});
	let loading = $state(true);
	let creatingQuick = $state(false);
	let updatingIssueId = $state<string | null>(null);

	let keyword = $state('');
	let priorityFilter = $state<'all' | IssuePriority>('all');
	let assigneeFilter = $state('all');
	const sprintFilterId = $derived(page.url.searchParams.get('sprintId') ?? '');

	let quickTitle = $state('');
	let showDetailedCreate = $state(false);

	let draggingIssueId = $state<string | null>(null);
	let dropTargetStatus = $state<IssueStatus | null>(null);

	onMount(async () => {
		await Promise.all([loadProject(), loadWorkflow(), loadIssues(), loadSprints()]);
	});

	async function loadProject() {
		const response = await projectsApi.get(projectId);
		if (response.code === 0 && response.data) {
			project = response.data;
		}
	}

	const scenarioTemplate = $derived(getProjectScenarioTemplate(project));

	async function loadWorkflow() {
		const response = await workflowsApi.getEffectiveByProject(projectId);
		if (response.code === 0 && response.data?.states?.length) {
			workflowStates = response.data.states;
		} else {
			workflowStates = [
				{
					key: 'backlog',
					display_name: get(t)('issue.backlog'),
					category: 'active',
					position: 1,
					is_initial: true,
					is_terminal: false
				},
				{
					key: 'todo',
					display_name: get(t)('issue.todo'),
					category: 'active',
					position: 2,
					is_initial: false,
					is_terminal: false
				},
				{
					key: 'in_progress',
					display_name: get(t)('issue.inProgress'),
					category: 'active',
					position: 3,
					is_initial: false,
					is_terminal: false
				},
				{
					key: 'done',
					display_name: get(t)('issue.done'),
					category: 'done',
					position: 4,
					is_initial: false,
					is_terminal: true
				}
			];
		}
	}

	async function loadIssues() {
		loading = true;
		const response = await issuesApi.list(projectId, { page: 1, per_page: 500 });

		if (response.code !== 0) {
			toast.error(response.message);
		} else if (response.data) {
			issues = response.data.items ?? [];
		}

		loading = false;

		membersApi
			.list(workspaceId)
			.then((res) => {
				if (res.code === 0 && res.data) {
					const nameMap: Record<string, string> = {};
					const entityMap: Record<string, string> = {};
					for (const m of (res.data.items ?? []) as any[]) {
						const memberId = m.user_id || m.id;
						nameMap[memberId] = m.name || m.email || memberId.substring(0, 8);
						entityMap[memberId] = m.entity_type || 'human';
					}
					memberNameMap = nameMap;
					memberEntityTypeMap = entityMap;
				}
			})
			.catch(() => {});
	}

	async function loadSprints() {
		const response = await sprintsApi.list(projectId);
		if (response.code !== 0) {
			toast.error(response.message);
			return;
		}
		if (response.data) {
			sprints = response.data.items ?? [];
		}
	}

	const assigneeOptions = $derived.by(() => {
		const values = issues
			.map((issue) => issue.assignee_id)
			.filter((assigneeId): assigneeId is string => Boolean(assigneeId));
		return Array.from(new Set(values));
	});

	const filteredIssues = $derived.by(() => {
		const query = keyword.trim().toLowerCase();
		return issues.filter((issue) => {
			const keywordMatched =
				query.length === 0 ||
				issue.title.toLowerCase().includes(query) ||
				issue.description?.toLowerCase().includes(query) ||
				issue.id.toLowerCase().includes(query) ||
				issue.key?.toLowerCase().includes(query);

			const priorityMatched = priorityFilter === 'all' || issue.priority === priorityFilter;
			const assigneeMatched = assigneeFilter === 'all' || issue.assignee_id === assigneeFilter;
			const sprintMatched = !sprintFilterId || issue.sprint_id === sprintFilterId;

			return keywordMatched && priorityMatched && assigneeMatched && sprintMatched;
		});
	});

	const sprintNameMap = $derived.by(() => {
		const map: Record<string, string> = {};
		for (const sprint of sprints) {
			map[sprint.id] = sprint.name;
		}
		return map;
	});

	function getIssuesByStatus(status: IssueStatus): Issue[] {
		return filteredIssues.filter((issue) => issue.status === status);
	}

	function getIssueCode(issue: Issue): string {
		if (issue.key?.trim()) {
			return issue.key;
		}
		if (/^\d+$/.test(issue.id)) {
			return `#${issue.id}`;
		}
		return `#${issue.id.slice(0, 8)}`;
	}

	function getPriorityBarClass(priority: IssuePriority): string {
		const classes: Record<IssuePriority, string> = {
			low: 'bg-control',
			medium: 'bg-fg-muted',
			high: 'bg-warning',
			urgent: 'bg-danger'
		};
		return classes[priority];
	}

	function isBotAssignee(assigneeId: string): boolean {
		return memberEntityTypeMap[assigneeId] === 'bot';
	}

	function getAssigneeLabel(assigneeId: string): string {
		const name = memberNameMap[assigneeId] || assigneeId.substring(0, 8);
		return isBotAssignee(assigneeId) ? `[Bot] ${name}` : name;
	}

	async function handleQuickCreate() {
		if (creatingQuick) return;

		const title = quickTitle.trim();
		if (!title) {
			toast.error(get(t)('issue.enterTitle'));
			return;
		}

		creatingQuick = true;
		const response = await issuesApi.create(projectId, {
			title,
			status: (workflowStates.find((s) => s.is_initial)?.key ||
				workflowStates[0]?.key ||
				'backlog') as IssueStatus,
			priority: 'medium',
			sprint_id: sprintFilterId || undefined
		});

		if (response.code !== 0) {
			toast.error(response.message);
		} else if (response.data) {
			issues = [response.data, ...issues];
			quickTitle = '';
			toast.success(get(t)('issue.createSuccess'));
		}

		creatingQuick = false;
	}

	function handleDetailedCreated(issue: Issue) {
		issues = [issue, ...issues];
	}

	function handleDragStart(issueId: string) {
		draggingIssueId = issueId;
	}

	function handleDragEnd() {
		draggingIssueId = null;
		dropTargetStatus = null;
	}

	function handleDragOver(event: DragEvent) {
		event.preventDefault();
	}

	function handleDragEnter(status: IssueStatus) {
		if (draggingIssueId) {
			dropTargetStatus = status;
		}
	}

	function handleDragLeave(event: DragEvent, status: IssueStatus) {
		const current = event.currentTarget;
		const related = event.relatedTarget;

		if (current instanceof HTMLElement && related instanceof Node && current.contains(related)) {
			return;
		}

		if (dropTargetStatus === status) {
			dropTargetStatus = null;
		}
	}

	async function handleDrop(event: DragEvent, targetStatus: IssueStatus) {
		event.preventDefault();
		dropTargetStatus = null;

		if (!draggingIssueId) {
			return;
		}

		const issueId = draggingIssueId;
		draggingIssueId = null;

		const currentIssue = issues.find((issue) => issue.id === issueId);
		if (!currentIssue) {
			return;
		}
		if (currentIssue.status === targetStatus) {
			return;
		}

		const previousIssues = issues;
		issues = issues.map((issue) =>
			issue.id === issueId
				? { ...issue, status: targetStatus, updated_at: new Date().toISOString() }
				: issue
		);
		updatingIssueId = issueId;

		const response = await issuesApi.update(issueId, { status: targetStatus });
		updatingIssueId = null;

		if (response.code !== 0) {
			issues = previousIssues;
			toast.error(get(t)('issue.statusUpdateFailed', { values: { message: response.message } }));
			return;
		}

		if (response.data) {
			const updatedIssue = response.data;
			issues = issues.map((issue) => (issue.id === issueId ? updatedIssue : issue));
		}
	}
</script>

<div class="mx-auto max-w-7xl space-y-6">
	<div class="rounded-card border border-line bg-raised p-4 sm:p-5">
		<div class="mb-4 flex flex-col gap-3 lg:flex-row lg:items-center lg:justify-between">
			<div class="text-sm text-fg-muted">
				<a href="/workspace" class="hover:underline">{$t('nav.myWorkspace')}</a>
				<span class="mx-1 text-fg-muted">/</span>
				<a href="/workspace/{workspaceId}/projects" class="hover:underline">{$t('project.title')}</a
				>
				<span class="mx-1 text-fg-muted">/</span>
				<a href="/workspace/{workspaceId}/projects/{projectId}" class="hover:underline"
					>{$t('project.detail')}</a
				>
				<span class="mx-1 text-fg-muted">/</span>
				<span class="font-medium text-fg">{$t('board.title')}</span>
			</div>
		</div>
		{#if sprintFilterId}
			<div
				class="mb-4 flex items-center justify-between rounded-control border bg-surface px-3 py-2 text-sm"
			>
				<span class="text-fg"
					>{$t('board.filterBySprint', {
						values: { sprint: sprintNameMap[sprintFilterId] ?? sprintFilterId }
					})}</span
				>
				<a href="/workspace/{workspaceId}/projects/{projectId}/board" class="text-fg underline">
					{$t('board.clearFilter')}
				</a>
			</div>
		{/if}

		<div class="grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-5">
			<div class="xl:col-span-2">
				<Input
					label={$t('common.search')}
					placeholder={$t('issue.searchPlaceholder')}
					bind:value={keyword}
					type="search"
				/>
			</div>
			<div>
				<label class="mb-1 block text-sm font-medium text-fg" for="priorityFilter"
					>{$t('common.priority')}</label
				>
				<select
					id="priorityFilter"
					bind:value={priorityFilter}
					class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
				>
					<option value="all">{$t('issue.all')}</option>
					<option value="low">{$t('issue.low')}</option>
					<option value="medium">{$t('issue.medium')}</option>
					<option value="high">{$t('issue.high')}</option>
					<option value="urgent">{$t('issue.urgent')}</option>
				</select>
			</div>
			<div>
				<label class="mb-1 block text-sm font-medium text-fg" for="assigneeFilter"
					>{$t('common.assignee')}</label
				>
				<select
					id="assigneeFilter"
					bind:value={assigneeFilter}
					class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
				>
					<option value="all">{$t('issue.all')}</option>
					{#each assigneeOptions as assigneeId}
						<option value={assigneeId}>{getAssigneeLabel(assigneeId)}</option>
					{/each}
				</select>
			</div>
			<div class="flex items-end gap-2">
				<Button variant="secondary" onclick={() => (showDetailedCreate = true)}
					>{$t('issue.detailedCreate')}</Button
				>
			</div>
		</div>

		<div class="mt-4 flex flex-col gap-2 sm:flex-row">
			<input
				type="text"
				bind:value={quickTitle}
				placeholder={$t('issue.quickCreatePlaceholder')}
				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
			/>
			<Button loading={creatingQuick} onclick={handleQuickCreate}>+ {$t('issue.create')}</Button>
		</div>
	</div>

	{#if loading}
		<div class="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-4">
			{#each columns as column}
				<div class="rounded-card border border-line bg-surface p-3">
					<div class="mb-3 h-5 w-24 animate-pulse rounded bg-surface"></div>
					<div class="space-y-3">
						{#each Array(2) as _, index (`${column}-${index}`)}
							<div class="h-24 animate-pulse rounded-control border border-line bg-raised"></div>
						{/each}
					</div>
				</div>
			{/each}
		</div>
	{:else}
		<div class="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-4">
			{#each columns as column}
				<div
					class={`rounded-card border p-3 transition-colors ${
						dropTargetStatus === column ? 'border-fg bg-surface' : 'border-line bg-surface'
					}`}
					role="region"
					ondragover={handleDragOver}
					ondragenter={() => handleDragEnter(column)}
					ondragleave={(event) => handleDragLeave(event, column)}
					ondrop={(event) => handleDrop(event, column)}
				>
					<div class="mb-3 flex items-center justify-between">
						<h2 class="text-sm font-semibold text-fg">
							{getColumnName(column)}
						</h2>
						<span class="rounded-full bg-surface px-2 py-1 text-xs text-fg-muted">
							{getIssuesByStatus(column).length}
						</span>
					</div>

					<div class="space-y-3">
						{#each getIssuesByStatus(column) as issue (issue.id)}
							<div
								draggable="true"
								role="article"
								ondragstart={() => handleDragStart(issue.id)}
								ondragend={handleDragEnd}
								class={`group relative overflow-hidden rounded-control border bg-raised p-3 transition hover:border-line-strong ${draggingIssueId === issue.id ? 'opacity-50' : ''}`}
							>
								<div
									class={`absolute inset-y-0 left-0 w-1 ${getPriorityBarClass(issue.priority)}`}
								></div>
								<div class="ml-2 space-y-2">
									<div class="flex items-start justify-between gap-2">
										<a
											href="/workspace/{workspaceId}/projects/{projectId}/issues/{issue.id}"
											class="line-clamp-2 text-sm font-medium text-fg hover:underline"
										>
											{issue.title}
										</a>
										{#if updatingIssueId === issue.id}
											<span class="text-[10px] text-fg-muted">{$t('common.updating')}</span>
										{/if}
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
									{#if issue.sprint_id}
										<span
											class="inline-flex w-fit items-center rounded-full border bg-surface px-2 py-0.5 text-xs font-medium text-fg"
										>
											<svg
												class="mr-1 h-3.5 w-3.5"
												fill="none"
												stroke="currentColor"
												viewBox="0 0 24 24"
											>
												<path
													stroke-linecap="round"
													stroke-linejoin="round"
													stroke-width="2"
													d="M5 4v16m0-14h10l-2 4 2 4H5"
												></path>
											</svg>
											{sprintNameMap[issue.sprint_id] ?? $t('issue.sprint')}
										</span>
									{/if}
									<div class="flex items-center justify-between gap-2">
										<div class="flex items-center gap-2">
											<Badge kind="priority" value={issue.priority} />
											<span class="font-mono text-xs text-fg-muted">{getIssueCode(issue)}</span>
										</div>
										{#if issue.assignee_id}
											<div class="flex items-center gap-1.5">
												<Avatar userId={issue.assignee_id} />
												{#if isBotAssignee(issue.assignee_id)}
													<BotIcon class="h-3.5 w-3.5 text-fg-muted" title={$t('admin.botBadge')} />
												{/if}
											</div>
										{/if}
									</div>
								</div>
							</div>
						{/each}

						{#if getIssuesByStatus(column).length === 0}
							<div
								class="rounded-control border border-dashed border-input bg-raised p-4 text-center text-sm text-fg-muted"
							>
								{$t('issue.none')}
							</div>
						{/if}
					</div>
				</div>
			{/each}
		</div>
	{/if}
</div>

<IssueCreateModal
	bind:open={showDetailedCreate}
	{workspaceId}
	{projectId}
	initialStatus={(workflowStates.find((s) => s.is_initial)?.key ||
		workflowStates[0]?.key ||
		'backlog') as IssueStatus}
	initialSprintId={sprintFilterId}
	{workflowStates}
	{scenarioTemplate}
	onCreated={handleDetailedCreated}
/>
