<script lang="ts">
	import { describeResult } from '#lib/i18n/describe.js';
	import { resolve } from '$app/paths';
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { directoryName, someoneText } from '#lib/people/display.js';
	import { locale, t } from 'svelte-i18n';
	import { page } from '$app/state';
	import { membersApi, type WorkspaceMember } from '#lib/api/members.js';
	import { issuesApi, type Issue, type IssuePriority, type IssueStatus } from '#lib/api/issues.js';
	import { workflowsApi, type WorkflowStateDef } from '#lib/api/workflows.js';
	import { labelsApi, type Label } from '#lib/api/labels.js';
	import { sprintsApi, type Sprint } from '#lib/api/sprints.js';
	import { milestonesApi, type Milestone } from '#lib/api/milestones.js';
	import { loadAllMilestones, sortMilestones } from '#lib/milestones/rules.js';
	import { toast } from '#lib/stores/toast.js';
	import { workflowStateLabel } from '#lib/utils/workflow-state.js';
	import { requireRouteParam } from '#lib/utils/route-params.js';
	import Badge from '#lib/components/Badge.svelte';
	import Avatar from '#lib/components/Avatar.svelte';
	import BotIcon from '#lib/components/BotIcon.svelte';
	import IssueCreateModal from '#lib/components/IssueCreateModal.svelte';

	const workspaceId = $derived(requireRouteParam(page.params.workspaceId, 'workspaceId'));
	const projectId = $derived(requireRouteParam(page.params.projectId, 'projectId'));

	let issues = $state<Issue[]>([]);
	let sprints = $state<Sprint[]>([]);
	let milestones = $state<Milestone[]>([]);
	// `all`, `none` (items in no milestone) or a milestone id.
	let milestoneFilter = $state('all');
	let loading = $state(true);
	let keyword = $state('');
	let workflowStates = $state<WorkflowStateDef[]>([]);
	let statusFilter = $state<'all' | IssueStatus>('all');
	let priorityFilter = $state<'all' | IssuePriority>('all');
	let assigneeFilter = $state('all');
	let workspaceLabels = $state<Label[]>([]);
	let selectedLabelIds = $state<string[]>([]);
	let showLabelFilterDropdown = $state(false);
	let labelFilterContainer: HTMLDivElement | null = null;
	let memberNameMap = $state<Record<string, string>>({});
	let memberEntityTypeMap = $state<Record<string, string>>({});
	let sortBy = $state<'updated_at' | 'created_at' | 'priority' | 'title'>('updated_at');
	let sortOrder = $state<'asc' | 'desc'>('desc');
	let showCreateModal = $state(false);
	let currentPage = $state(1);
	let totalPages = $state(1);
	let total = $state(0);
	const perPage = 15;
	const labelFilterLabelId = 'issue-label-filter-label';

	onMount(() => {
		const handleDocumentClick = (event: MouseEvent) => {
			const target = event.target;
			if (!(target instanceof Node)) return;
			if (labelFilterContainer?.contains(target)) return;
			showLabelFilterDropdown = false;
		};

		document.addEventListener('click', handleDocumentClick);
		void (async () => {
			await Promise.all([
				loadWorkflow(),
				loadSprints(),
				loadMilestones(),
				loadMembers(),
				loadLabels()
			]);
			await loadIssues(1);
		})();

		return () => {
			document.removeEventListener('click', handleDocumentClick);
		};
	});

	async function loadIssues(pageToLoad: number = currentPage) {
		loading = true;
		const response = await issuesApi.list(projectId, {
			page: pageToLoad,
			per_page: perPage,
			status: statusFilter === 'all' ? undefined : statusFilter,
			priority: priorityFilter === 'all' ? undefined : priorityFilter,
			assignee_id: assigneeFilter === 'all' ? undefined : assigneeFilter,
			milestone_id: milestoneFilter === 'all' ? undefined : milestoneFilter,
			label_ids: selectedLabelIds.length ? selectedLabelIds.join(',') : undefined,
			search: keyword.trim() || undefined,
			sort_by: sortBy,
			sort_order: sortOrder
		});

		if (response.code !== 0) {
			toast.error(describeResult(response));
			issues = [];
			total = 0;
			totalPages = 1;
		} else if (response.data) {
			issues = response.data.items ?? [];
			total = response.data.total ?? 0;
			const computedTotalPages = response.data.total_pages ?? Math.ceil(total / perPage);
			totalPages = Math.max(1, computedTotalPages);
			currentPage = Math.min(Math.max(1, pageToLoad), totalPages);
		}

		loading = false;
	}

	async function loadMembers() {
		const res = await membersApi.list(workspaceId);
		if (res.code === 0 && res.data) {
			const nameMap: Record<string, string> = {};
			const entityMap: Record<string, string> = {};
			for (const m of (res.data.items ?? []) as Array<WorkspaceMember & { id: string }>) {
				const memberId = m.user_id || m.id;
				nameMap[memberId] = directoryName(m, get(t));
				entityMap[memberId] = m.entity_type || 'human';
			}
			memberNameMap = nameMap;
			memberEntityTypeMap = entityMap;
		}
	}

	async function loadSprints() {
		const response = await sprintsApi.list(projectId);
		if (response.code !== 0) {
			toast.error(describeResult(response));
			return;
		}
		if (response.data) {
			sprints = response.data.items ?? [];
		}
	}

	async function loadMilestones() {
		const response = await loadAllMilestones(milestonesApi.list, workspaceId);
		if (response.code !== 0) {
			toast.error(get(t)('milestones.field.loadFailed'));
			return;
		}
		milestones = response.data ?? [];
	}

	const milestoneFilterOptions = $derived(sortMilestones(milestones, $locale ?? undefined));

	async function loadLabels() {
		const res = await labelsApi.list(workspaceId);
		if (res.code !== 0) {
			toast.error(describeResult(res));
			workspaceLabels = [];
			return;
		}
		workspaceLabels = res.data?.items ?? [];
	}

	async function loadWorkflow() {
		const response = await workflowsApi.getEffectiveByProject(projectId);
		if (response.code === 0 && response.data?.states?.length) {
			workflowStates = response.data.states;
		}
	}

	const assigneeOptions = $derived.by(() => {
		return Object.keys(memberNameMap);
	});

	const sprintNameMap = $derived.by(() => {
		const map: Record<string, string> = {};
		for (const sprint of sprints) {
			map[sprint.id] = sprint.name;
		}
		return map;
	});

	function issueHref(issueId: string): string {
		return `/workspace/${workspaceId}/projects/${projectId}/issues/${issueId}`;
	}

	function goToIssue(issueId: string) {
		window.open(issueHref(issueId), '_blank');
	}

	// The title link opens the issue itself (keyboard, middle click); the row click is a pointer
	// convenience and must not fire a second open when the click came from a link inside the row.
	function onRowClick(event: MouseEvent, issueId: string) {
		if ((event.target as HTMLElement | null)?.closest('a, button')) return;
		goToIssue(issueId);
	}

	function formatIssueCode(issue: Issue): string {
		return issue.identifier;
	}

	function formatDate(dateText: string): string {
		return new Date(dateText).toLocaleString(get(locale) === 'en' ? 'en-US' : 'zh-CN', {
			year: 'numeric',
			month: '2-digit',
			day: '2-digit',
			hour: '2-digit',
			minute: '2-digit'
		});
	}

	function isBotAssignee(assigneeId: string): boolean {
		return memberEntityTypeMap[assigneeId] === 'bot';
	}

	function getAssigneeLabel(assigneeId: string): string {
		const name = memberNameMap[assigneeId] || someoneText(get(t));
		return isBotAssignee(assigneeId) ? `[Bot] ${name}` : name;
	}

	function toggleLabelSelection(labelId: string) {
		if (selectedLabelIds.includes(labelId)) {
			selectedLabelIds = selectedLabelIds.filter((id) => id !== labelId);
		} else {
			selectedLabelIds = [...selectedLabelIds, labelId];
		}
		resetPageAndLoad();
	}

	function getLabelFilterText(): string {
		if (selectedLabelIds.length === 0) {
			return get(t)('issue.all');
		}
		if (selectedLabelIds.length === 1) {
			const selected = workspaceLabels.find((label) => label.id === selectedLabelIds[0]);
			return selected?.name ?? get(t)('common.label');
		}
		return `${get(t)('common.label')} (${selectedLabelIds.length})`;
	}

	function handleIssueCreated(issue: Issue) {
		void issue;
		currentPage = 1;
		void loadIssues(1);
	}

	function getPageNumbers(current: number, totalCount: number): Array<number | '...'> {
		if (totalCount <= 7) {
			return Array.from({ length: totalCount }, (_, i) => i + 1);
		}

		// eslint-disable-next-line svelte/prefer-svelte-reactivity -- local scratch collection, not reactive state
		const pages = new Set<number>([1, totalCount, current - 1, current, current + 1]);
		if (current <= 3) {
			pages.add(2);
			pages.add(3);
		}
		if (current >= totalCount - 2) {
			pages.add(totalCount - 1);
			pages.add(totalCount - 2);
		}

		const sorted = [...pages].filter((p) => p >= 1 && p <= totalCount).sort((a, b) => a - b);
		const result: Array<number | '...'> = [];
		for (let i = 0; i < sorted.length; i += 1) {
			const pageNum = sorted[i];
			const prev = sorted[i - 1];
			if (i > 0 && pageNum - prev > 1) {
				result.push('...');
			}
			result.push(pageNum);
		}
		return result;
	}

	function goToPage(pageNum: number) {
		if (pageNum < 1 || pageNum > totalPages || pageNum === currentPage) return;
		currentPage = pageNum;
		void loadIssues(pageNum);
	}

	function resetPageAndLoad() {
		currentPage = 1;
		void loadIssues(1);
	}
</script>

<div class="mx-auto max-w-7xl space-y-6">
	<div class="rounded-card border border-line bg-raised p-4 sm:p-5">
		<div class="mb-4 flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
			<div class="text-sm text-fg-muted">
				<a href={resolve('/(app)/workspace')} class="hover:underline">{$t('nav.myWorkspace')}</a>
				<span class="mx-1 text-fg-muted">/</span>
				<a
					href={resolve('/(app)/workspace/[workspaceId]/projects', { workspaceId })}
					class="hover:underline">{$t('project.title')}</a
				>
				<span class="mx-1 text-fg-muted">/</span>
				<a
					href={resolve('/(app)/workspace/[workspaceId]/projects/[projectId]', {
						workspaceId,
						projectId
					})}
					class="hover:underline">{$t('project.detail')}</a
				>
				<span class="mx-1 text-fg-muted">/</span>
				<span class="font-medium text-fg">{$t('issue.list')}</span>
			</div>
			<button
				type="button"
				class="inline-flex items-center justify-center rounded-control rf-filled bg-primary px-4 py-2 text-sm font-medium text-on-primary transition"
				onclick={() => (showCreateModal = true)}>+ {$t('issue.create')}</button
			>
		</div>

		<div class="grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-8">
			<div class="xl:col-span-2">
				<label class="mb-1 block text-sm font-medium text-fg" for="keyword"
					>{$t('common.search')}</label
				>
				<input
					id="keyword"
					type="search"
					bind:value={keyword}
					oninput={resetPageAndLoad}
					placeholder={$t('issue.searchPlaceholder')}
					class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
				/>
			</div>
			<div>
				<label class="mb-1 block text-sm font-medium text-fg" for="statusFilter"
					>{$t('common.status')}</label
				>
				<select
					id="statusFilter"
					bind:value={statusFilter}
					onchange={resetPageAndLoad}
					class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
				>
					<option value="all">{$t('issue.all')}</option>
					{#if workflowStates.length > 0}
						{#each workflowStates as state (state.key)}
							<option value={state.key}>{workflowStateLabel(state, $t)}</option>
						{/each}
					{:else}
						<option value="backlog">{$t('issue.backlog')}</option>
						<option value="todo">{$t('issue.todo')}</option>
						<option value="in_progress">{$t('issue.inProgress')}</option>
						<option value="done">{$t('issue.done')}</option>
					{/if}
				</select>
			</div>
			<div>
				<label class="mb-1 block text-sm font-medium text-fg" for="priorityFilter"
					>{$t('common.priority')}</label
				>
				<select
					id="priorityFilter"
					bind:value={priorityFilter}
					onchange={resetPageAndLoad}
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
					onchange={resetPageAndLoad}
					class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
				>
					<option value="all">{$t('issue.all')}</option>
					{#each assigneeOptions as assigneeId, idx (idx)}
						<option value={assigneeId}>{getAssigneeLabel(assigneeId)}</option>
					{/each}
				</select>
			</div>
			<div>
				<label class="mb-1 block text-sm font-medium text-fg" for="milestoneFilter"
					>{$t('milestones.filterItems.label')}</label
				>
				<select
					id="milestoneFilter"
					bind:value={milestoneFilter}
					onchange={resetPageAndLoad}
					class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
				>
					<option value="all">{$t('milestones.filterItems.all')}</option>
					<option value="none">{$t('milestones.filterItems.none')}</option>
					{#each milestoneFilterOptions as milestone (milestone.id)}
						<option value={milestone.id}
							>{milestone.status === 'closed'
								? `${milestone.name} (${$t('milestones.field.closed')})`
								: milestone.name}</option
						>
					{/each}
				</select>
			</div>
			<div class="relative" bind:this={labelFilterContainer}>
				<span id={labelFilterLabelId} class="mb-1 block text-sm font-medium text-fg"
					>{$t('common.label')}</span
				>
				<button
					type="button"
					aria-expanded={showLabelFilterDropdown}
					aria-haspopup="listbox"
					aria-labelledby={labelFilterLabelId}
					class="flex w-full items-center justify-between rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
					onclick={(event) => {
						event.stopPropagation();
						showLabelFilterDropdown = !showLabelFilterDropdown;
					}}
				>
					<span class="truncate text-left">{getLabelFilterText()}</span>
					<svg
						class="ml-2 h-4 w-4 shrink-0 text-fg-muted"
						fill="none"
						stroke="currentColor"
						viewBox="0 0 24 24"
					>
						<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7"
						></path>
					</svg>
				</button>
				{#if showLabelFilterDropdown}
					<div
						class="absolute z-20 mt-1 w-full rounded-float border border-line bg-overlay p-2 shadow-overlay"
					>
						<p class="mb-2 px-2 text-xs font-medium text-fg-muted">
							{$t('common.label')}
						</p>
						{#if workspaceLabels.length === 0}
							<p class="px-2 py-1 text-xs text-fg-muted">
								{$t('issue.noWorkspaceLabels')}
							</p>
						{:else}
							<div class="max-h-52 space-y-1 overflow-y-auto">
								{#each workspaceLabels as label (label.id)}
									<button
										type="button"
										class="flex w-full items-center justify-between rounded px-2 py-1.5 text-left text-sm hover:bg-surface"
										onclick={() => toggleLabelSelection(label.id)}
									>
										<div class="flex min-w-0 items-center gap-2">
											<input
												type="checkbox"
												checked={selectedLabelIds.includes(label.id)}
												readonly
											/>
											<span
												class="inline-flex items-center rounded-full px-2 py-0.5 text-xs font-medium"
												style={`background-color: ${label.color}20; color: ${label.color}; border: 1px solid ${label.color}40;`}
											>
												{label.name}
											</span>
										</div>
										{#if selectedLabelIds.includes(label.id)}
											<span class="text-xs text-fg">✓</span>
										{/if}
									</button>
								{/each}
							</div>
						{/if}
					</div>
				{/if}
			</div>
			<div>
				<label class="mb-1 block text-sm font-medium text-fg" for="sortBy"
					>{$t('issue.sortBy')}</label
				>
				<div class="flex gap-2">
					<select
						id="sortBy"
						bind:value={sortBy}
						onchange={resetPageAndLoad}
						class="w-full rounded-control border border-input bg-field px-2 py-2 text-sm text-fg"
					>
						<option value="updated_at">{$t('common.updatedAt')}</option>
						<option value="created_at">{$t('common.createdAt')}</option>
						<option value="priority">{$t('common.priority')}</option>
						<option value="title">{$t('issue.title')}</option>
					</select>
					<select
						bind:value={sortOrder}
						onchange={resetPageAndLoad}
						class="rounded-control border border-input bg-field px-2 py-2 text-sm text-fg"
					>
						<option value="desc">{$t('common.desc')}</option>
						<option value="asc">{$t('common.asc')}</option>
					</select>
				</div>
			</div>
		</div>
	</div>

	{#if loading}
		<div class="rounded-card border border-line bg-raised p-4">
			<div class="space-y-3">
				{#each Array(5)}
					<div class="h-12 animate-pulse rounded bg-surface"></div>
				{/each}
			</div>
		</div>
	{:else if issues.length === 0}
		<div class="rounded-card border border-line bg-raised p-12 text-center text-fg-muted">
			{$t('issue.noMatched')}
		</div>
	{:else}
		<div class="overflow-hidden rounded-card border border-line bg-raised">
			<div class="hidden overflow-x-auto md:block">
				<table class="min-w-full divide-y divide-line">
					<thead class="bg-nav">
						<tr>
							<th
								class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-fg-muted"
								>{$t('issue.title')}</th
							>
							<th
								class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-fg-muted"
								>{$t('common.status')}</th
							>
							<th
								class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-fg-muted"
								>{$t('common.priority')}</th
							>
							<th
								class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-fg-muted"
								>{$t('common.assignee')}</th
							>
							<th
								class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-fg-muted"
								>{$t('common.updatedAt')}</th
							>
						</tr>
					</thead>
					<tbody class="divide-y divide-line bg-raised">
						{#each issues as issue (issue.id)}
							<tr
								class="cursor-pointer hover:bg-surface"
								onclick={(event) => onRowClick(event, issue.id)}
							>
								<td class="px-4 py-3">
									<div class="space-y-1">
										<p class="text-sm font-medium text-fg">
											<a
												href={issueHref(issue.id)}
												target="_blank"
												rel="noopener noreferrer"
												class="rounded-control hover:underline"
											>
												{issue.title}
											</a>
										</p>
										<p class="font-mono text-xs text-fg-muted">
											{formatIssueCode(issue)}
										</p>
										{#if issue.labels?.length}
											<div class="mt-1 flex flex-wrap gap-1">
												{#each issue.labels as label, idx (idx)}
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
									</div>
								</td>
								<td class="px-4 py-3"><Badge value={issue.status} /></td>
								<td class="px-4 py-3"><Badge kind="priority" value={issue.priority} /></td>
								<td class="px-4 py-3">
									{#if issue.assignee_id}
										<div class="flex items-center gap-2">
											<Avatar name={memberNameMap[issue.assignee_id]} />
											{#if isBotAssignee(issue.assignee_id)}
												<BotIcon class="h-3.5 w-3.5 text-fg-muted" title={$t('admin.botBadge')} />
											{/if}
											<span class="text-xs text-fg-muted"
												>{memberNameMap[issue.assignee_id] || $t('people.someone')}</span
											>
										</div>
									{:else}
										<span class="text-xs text-fg-muted">{$t('common.unassigned')}</span>
									{/if}
								</td>
								<td class="px-4 py-3 text-sm text-fg-muted">{formatDate(issue.updated_at)}</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>

			<div class="divide-y divide-line md:hidden">
				{#each issues as issue (issue.id)}
					<button
						class="w-full space-y-2 p-4 text-left hover:bg-surface"
						onclick={() => goToIssue(issue.id)}
					>
						<div class="flex items-start justify-between gap-3">
							<div>
								<p class="text-sm font-medium text-fg">{issue.title}</p>
								<p class="font-mono text-xs text-fg-muted">
									{formatIssueCode(issue)}
								</p>
								{#if issue.labels?.length}
									<div class="mt-1 flex flex-wrap gap-1">
										{#each issue.labels as label, idx (idx)}
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
							</div>
							<Badge kind="priority" value={issue.priority} />
						</div>
						<div class="flex items-center justify-between">
							<Badge value={issue.status} />
							{#if issue.assignee_id}
								<div class="flex items-center gap-2">
									<Avatar name={memberNameMap[issue.assignee_id]} />
									{#if isBotAssignee(issue.assignee_id)}
										<BotIcon class="h-3.5 w-3.5 text-fg-muted" title={$t('admin.botBadge')} />
									{/if}
									<span class="text-xs text-fg-muted"
										>{memberNameMap[issue.assignee_id] || $t('people.someone')}</span
									>
								</div>
							{:else}
								<span class="text-xs text-fg-muted">{$t('common.unassigned')}</span>
							{/if}
						</div>
						<p class="text-xs text-fg-muted">
							{$t('project.updatedAt')}
							{formatDate(issue.updated_at)}
						</p>
					</button>
				{/each}
			</div>
		</div>

		{#if totalPages > 1}
			<div class="mt-6 flex items-center justify-between px-2">
				<span class="text-sm text-fg-muted"
					>{$t('issue.pagination', {
						values: { total, current: currentPage, pages: totalPages }
					})}</span
				>
				<div class="flex items-center gap-1">
					<button
						class="rounded border px-3 py-1 text-sm {currentPage === 1
							? 'cursor-not-allowed bg-surface text-fg-muted'
							: 'hover:bg-surface'}"
						onclick={() => goToPage(currentPage - 1)}
						disabled={currentPage === 1}
					>
						{$t('issue.prevPage')}
					</button>
					{#each getPageNumbers(currentPage, totalPages) as p, idx (idx)}
						{#if p === '...'}
							<span class="px-2 text-fg-muted">...</span>
						{:else}
							<button
								class="rounded px-3 py-1 text-sm {p === currentPage
									? 'bg-selected text-fg'
									: 'border hover:bg-surface'}"
								onclick={() => goToPage(p)}
							>
								{p}
							</button>
						{/if}
					{/each}
					<button
						class="rounded border px-3 py-1 text-sm {currentPage === totalPages
							? 'cursor-not-allowed bg-surface text-fg-muted'
							: 'hover:bg-surface'}"
						onclick={() => goToPage(currentPage + 1)}
						disabled={currentPage === totalPages}
					>
						{$t('issue.nextPage')}
					</button>
				</div>
			</div>
		{/if}
	{/if}
</div>

<IssueCreateModal
	bind:open={showCreateModal}
	{workspaceId}
	{projectId}
	initialStatus={(workflowStates.find((s) => s.is_initial)?.key ||
		workflowStates[0]?.key ||
		'backlog') as IssueStatus}
	{workflowStates}
	onCreated={handleIssueCreated}
/>
