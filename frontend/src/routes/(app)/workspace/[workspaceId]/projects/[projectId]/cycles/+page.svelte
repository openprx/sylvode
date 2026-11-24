<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { SvelteSet } from 'svelte/reactivity';
	import { get } from 'svelte/store';
	import { locale, t } from 'svelte-i18n';
	import { issuesApi, type Issue } from '#lib/api/issues.js';
	import { workspacesApi } from '#lib/api/workspaces.js';
	import { authStore } from '#lib/stores/auth.js';
	import {
		canManageSprints,
		pastSprints,
		rolloverMessage,
		rolloverTargets,
		sprintFailureMessage
	} from '#lib/sprints/rules.js';
	import {
		emptyIssueList,
		hasMoreIssues,
		nextIssuePage,
		sprintProgressOf,
		withIssuePage,
		type SprintIssueList
	} from '#lib/sprints/issue-pages.js';
	import type { WorkspaceRole } from '#lib/workspace/home.js';
	import { WorkspaceRoleTracker } from '#lib/workspace/role.js';
	import {
		sprintsApi,
		type CreateSprintData,
		type Sprint,
		type SprintStatus,
		type UpdateSprintData
	} from '#lib/api/sprints.js';
	import ProgressBar from '#lib/components/ProgressBar.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import { isMissingResult } from '#lib/utils/api-helpers.js';
	import { toast } from '#lib/stores/toast.js';
	import { confirmAction } from '#lib/ui/dialog/confirm.js';
	import { requireRouteParam } from '#lib/utils/route-params.js';

	const workspaceId = requireRouteParam(page.params.workspaceId, 'workspaceId');
	const projectId = requireRouteParam(page.params.projectId, 'projectId');

	let loading = $state(true);
	let saving = $state(false);
	let sprints = $state<Sprint[]>([]);
	// Expanded sprints and the pages read for them.
	let issueLists = $state<Record<string, SprintIssueList>>({});
	const expanded = new SvelteSet<string>();
	let errorMessage = $state('');
	let projectMissing = $state(false);

	// Starting, completing, cancelling and deleting a sprint are for owners and admins: the buttons
	// are not shown to anyone else (the server refuses the calls too).
	let role = $state<WorkspaceRole | null>(null);
	const tracker = new WorkspaceRoleTracker(
		(id) => workspacesApi.getMembers(id),
		(next) => {
			role = next;
		}
	);
	const userId = $derived($authStore.user?.id ?? null);
	$effect(() => {
		void tracker.select(workspaceId, userId);
	});
	const canManage = $derived(canManageSprints(role));

	// Completing or cancelling asks where the unfinished issues go.
	let endDialog = $state<{ sprint: Sprint; status: 'completed' | 'cancelled' } | null>(null);
	let showEndDialog = $state(false);
	let rolloverChoice = $state('');
	const endTargets = $derived(endDialog ? rolloverTargets(sprints, endDialog.sprint.id) : []);

	let showCreateModal = $state(false);
	let showEditModal = $state(false);
	let editingSprint = $state<Sprint | null>(null);

	type SprintForm = {
		name: string;
		description: string;
		start_date: string;
		end_date: string;
	};

	const emptyForm: SprintForm = {
		name: '',
		description: '',
		start_date: '',
		end_date: ''
	};

	let createForm = $state<SprintForm>({ ...emptyForm });
	// A refused name is shown under the field: a toast would sit behind the open dialog.
	let createNameError = $state('');
	let createNameInput = $state<HTMLInputElement | null>(null);
	let editForm = $state<SprintForm>({ ...emptyForm });

	onMount(async () => {
		await loadData();
	});

	function startTime(sprint: Sprint): number {
		return sprint.start_date ? new Date(sprint.start_date).getTime() : Number.POSITIVE_INFINITY;
	}

	const activeSprints = $derived.by(() =>
		sprints.filter((item) => item.status === 'active').sort((a, b) => startTime(a) - startTime(b))
	);
	const plannedSprints = $derived.by(() =>
		sprints.filter((item) => item.status === 'planned').sort((a, b) => startTime(a) - startTime(b))
	);
	// Completed and cancelled sprints are two collapsed groups.
	const completedSprints = $derived(pastSprints(sprints, 'completed'));
	const cancelledSprints = $derived(pastSprints(sprints, 'cancelled'));

	const sprintProgress = $derived.by(() => {
		const stats: Record<string, ReturnType<typeof sprintProgressOf>> = {};
		for (const sprint of sprints) stats[sprint.id] = sprintProgressOf(sprint);
		return stats;
	});

	async function loadIssues(sprintId: string) {
		const current = issueLists[sprintId] ?? emptyIssueList();
		const query = nextIssuePage(sprintId, current);
		if (!query) return;
		issueLists[sprintId] = { ...current, loading: true, failed: false };
		const response = await issuesApi.list(projectId, query);
		const latest = issueLists[sprintId] ?? current;
		if (response.code !== 0 || !response.data) {
			issueLists[sprintId] = { ...latest, loading: false, failed: true };
			return;
		}
		issueLists[sprintId] = withIssuePage(latest, response.data);
	}

	function toggleIssues(sprintId: string) {
		if (expanded.has(sprintId)) {
			expanded.delete(sprintId);
			return;
		}
		expanded.add(sprintId);
		if (!issueLists[sprintId]) void loadIssues(sprintId);
	}

	async function loadData() {
		loading = true;
		errorMessage = '';
		projectMissing = false;

		const sprintsResponse = await sprintsApi.list(projectId);

		if (sprintsResponse.code !== 0) {
			errorMessage = sprintsResponse.message;
			projectMissing = isMissingResult(sprintsResponse);
			sprints = [];
		} else if (sprintsResponse.data) {
			sprints = sprintsResponse.data.items ?? [];
		}

		// Lists read before are stale after a change; the expanded sprints read their first page again.
		issueLists = {};
		for (const id of expanded) void loadIssues(id);

		loading = false;
	}

	function formatDate(dateText: string): string {
		return new Date(dateText).toLocaleDateString(get(locale) === 'en' ? 'en-US' : 'zh-CN', {
			year: 'numeric',
			month: '2-digit',
			day: '2-digit'
		});
	}

	function formatRange(sprint: Sprint): string {
		if (!sprint.start_date || !sprint.end_date) {
			return get(t)('cycles.noDates');
		}
		return `${formatDate(sprint.start_date)} ~ ${formatDate(sprint.end_date)}`;
	}

	function toDateInputValue(dateText: string | null): string {
		if (!dateText) return '';
		const date = new Date(dateText);
		const year = date.getFullYear();
		const month = String(date.getMonth() + 1).padStart(2, '0');
		const day = String(date.getDate()).padStart(2, '0');
		return `${year}-${month}-${day}`;
	}

	function getStatusClass(status: SprintStatus): string {
		const map: Record<SprintStatus, string> = {
			planned: 'bg-surface text-fg',
			active: 'bg-success-soft text-success',
			completed: 'bg-surface text-fg',
			cancelled: 'bg-danger-soft text-danger'
		};
		return map[status];
	}

	// A refused call shows its own translated text for the sprint rules the server names, and the
	// generic failure text with the server's message otherwise.
	function showFailure(response: { message: string; error_code?: string; details?: unknown }) {
		toast.error(sprintFailureMessage(get(t), response));
	}

	function formatIssueCode(issue: Issue): string {
		return issue.identifier;
	}

	function openCreateModal() {
		createForm = { ...emptyForm };
		createNameError = '';
		showCreateModal = true;
	}

	function openEditModal(sprint: Sprint) {
		editingSprint = sprint;
		editForm = {
			name: sprint.name,
			description: sprint.description ?? '',
			start_date: toDateInputValue(sprint.start_date),
			end_date: toDateInputValue(sprint.end_date)
		};
		showEditModal = true;
	}

	function buildPayload(form: SprintForm): CreateSprintData {
		return {
			name: form.name.trim(),
			description: form.description.trim() || undefined,
			start_date: form.start_date,
			end_date: form.end_date
		};
	}

	function validateForm(form: SprintForm): string | null {
		if (!form.name.trim()) {
			return get(t)('cycles.enterName');
		}
		if (!form.start_date || !form.end_date) {
			return get(t)('cycles.selectDates');
		}
		if (new Date(form.start_date).getTime() > new Date(form.end_date).getTime()) {
			return get(t)('cycles.endDateAfterStart');
		}
		return null;
	}

	async function submitCreate() {
		if (saving) return;
		const validateError = validateForm(createForm);
		if (validateError) {
			toast.error(validateError);
			return;
		}

		saving = true;
		const response = await sprintsApi.create(projectId, buildPayload(createForm));
		saving = false;

		if (response.code !== 0) {
			if (response.error_code === 'sprint_name_taken') {
				createNameError = sprintFailureMessage(get(t), response);
				createNameInput?.focus();
				createNameInput?.select();
				return;
			}
			showFailure(response);
			return;
		}

		showCreateModal = false;
		toast.success(get(t)('cycles.createSuccess'));
		await loadData();
	}

	async function submitEdit() {
		if (saving || !editingSprint) return;
		const validateError = validateForm(editForm);
		if (validateError) {
			toast.error(validateError);
			return;
		}

		saving = true;
		const payload: UpdateSprintData = buildPayload(editForm);
		const response = await sprintsApi.update(editingSprint.id, payload);
		saving = false;

		if (response.code !== 0) {
			showFailure(response);
			return;
		}

		showEditModal = false;
		editingSprint = null;
		toast.success(get(t)('cycles.updateSuccess'));
		await loadData();
	}

	async function removeSprint(sprint: Sprint) {
		const nameValues = { values: { name: sprint.name } };
		const confirmed = await confirmAction({
			tone: 'destructive',
			title: get(t)('cycles.deleteTitle', nameValues),
			body: get(t)('cycles.deleteBody'),
			confirmLabel: get(t)('cycles.deleteAction', nameValues)
		});
		if (!confirmed) return;

		const response = await sprintsApi.delete(sprint.id);
		if (response.code !== 0) {
			showFailure(response);
			return;
		}

		toast.success(get(t)('cycles.deleteSuccess'));
		await loadData();
	}

	// Starting a second sprint is a 409 that names the sprint already active in its details.
	async function startSprint(sprint: Sprint) {
		const response = await sprintsApi.update(sprint.id, { status: 'active' });
		if (response.code !== 0) {
			showFailure(response);
			return;
		}
		toast.success(get(t)('cycles.started'));
		await loadData();
	}

	function openEndDialog(sprint: Sprint, status: 'completed' | 'cancelled') {
		endDialog = { sprint, status };
		rolloverChoice = '';
		showEndDialog = true;
	}

	async function submitEnd() {
		if (saving || !endDialog) return;
		const { sprint, status } = endDialog;
		// The sprints that could take the items, as they are before the call changes the list.
		const targets = endTargets.map(({ id, name }) => ({ id, name }));
		saving = true;
		const response = await sprintsApi.update(sprint.id, {
			status,
			rollover_sprint_id: rolloverChoice || null
		});
		saving = false;
		if (response.code !== 0) {
			showFailure(response);
			return;
		}
		showEndDialog = false;
		endDialog = null;
		toast.success(rolloverMessage(get(t), status, response.data?.rollover, targets));
		await loadData();
	}

	function goSprintBoard(sprintId: string) {
		goto(
			resolve(`/(app)/workspace/[workspaceId]/projects/[projectId]/board?sprintId=${sprintId}`, {
				workspaceId,
				projectId
			})
		);
	}
</script>

<div class="mx-auto max-w-7xl space-y-6">
	<div class="rounded-card border border-line bg-raised p-4 sm:p-5">
		<div class="mb-4 text-sm text-fg-muted">
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
			<span class="font-medium text-fg">{$t('cycles.title')}</span>
		</div>

		<div class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
			<div>
				<h1 class="text-2xl font-semibold text-fg">{$t('cycles.title')}</h1>
				<p class="mt-1 text-sm text-fg-muted">{$t('cycles.subtitle')}</p>
			</div>
			<button
				onclick={openCreateModal}
				class="rounded-control rf-filled bg-primary px-4 py-2 text-sm font-medium text-on-primary"
			>
				+ {$t('cycles.create')}
			</button>
		</div>
	</div>

	{#if loading}
		<div class="rounded-card border border-line bg-raised p-8 text-center text-fg-muted">
			<div
				class="mx-auto mb-3 h-6 w-6 animate-spin rounded-full border-2 border-fg border-t-transparent"
			></div>
			{$t('cycles.loading')}
		</div>
	{:else if errorMessage}
		<StateBlock
			kind="loadFailed"
			message={errorMessage}
			action={{ run: () => void loadData() }}
			data-testid="cycles-load-failed"
		/>
		{#if projectMissing}
			<p class="text-center text-sm">
				<a
					href={resolve('/(app)/workspace/[workspaceId]/projects', { workspaceId })}
					class="font-medium text-fg underline"
					data-testid="cycles-back-to-projects"
				>
					{$t('workbench.projects')}
				</a>
			</p>
		{/if}
	{:else}
		<section class="space-y-3">
			<h2 class="text-lg font-semibold text-fg">
				{$t('cycles.current')}
			</h2>
			{#if activeSprints.length === 0}
				<div class="rounded-card border border-line bg-raised p-6 text-sm text-fg-muted">
					{$t('cycles.noActive')}
				</div>
			{:else}
				<div class="space-y-3">
					{#each activeSprints as sprint (sprint.id)}
						<div class="rounded-card border border-line bg-raised p-4 sm:p-5">
							<div class="mb-3 flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
								<div>
									<h3 class="text-lg font-semibold text-fg">
										{sprint.name}
									</h3>
									<p class="text-sm text-fg-muted">
										{formatRange(sprint)}
									</p>
								</div>
								<span
									class="inline-flex w-fit rounded px-2 py-1 text-xs font-medium {getStatusClass(
										sprint.status
									)}"
								>
									{$t(`cycles.status.${sprint.status}`)}
								</span>
							</div>

							{#if sprint.description}
								<p class="mb-3 text-sm text-fg">
									{$t('cycles.description')}: {sprint.description}
								</p>
							{/if}

							<ProgressBar
								value={sprintProgress[sprint.id]?.percent ?? 0}
								label={$t('cycles.progressLabel', {
									values: {
										done: sprintProgress[sprint.id]?.done ?? 0,
										total: sprintProgress[sprint.id]?.total ?? 0
									}
								})}
							/>
							{@render issuePanel(sprint)}

							<div class="mt-4 flex flex-wrap gap-2">
								<button
									onclick={() => goSprintBoard(sprint.id)}
									class="rounded-control border border-fg px-3 py-1.5 text-sm text-fg hover:bg-surface"
								>
									{$t('cycles.viewDetail')}
								</button>
								{#if canManage}
									<button
										onclick={() => openEndDialog(sprint, 'cancelled')}
										class="rounded-control border px-3 py-1.5 text-sm text-danger hover:bg-danger-soft"
									>
										{$t('cycles.cancelSprint')}
									</button>
									<button
										onclick={() => openEndDialog(sprint, 'completed')}
										class="rounded-control rf-filled bg-primary px-3 py-1.5 text-sm text-on-primary"
									>
										{$t('cycles.completeSprint')}
									</button>
								{/if}
							</div>
						</div>
					{/each}
				</div>
			{/if}
		</section>

		<section class="space-y-3">
			<h2 class="text-lg font-semibold text-fg">
				{$t('cycles.planned')}
			</h2>
			{#if plannedSprints.length === 0}
				<div class="rounded-card border border-line bg-raised p-6 text-sm text-fg-muted">
					{$t('cycles.noPlanned')}
				</div>
			{:else}
				<div class="space-y-3">
					{#each plannedSprints as sprint (sprint.id)}
						<div class="rounded-card border border-line bg-raised p-4 sm:p-5">
							<div class="mb-3 flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
								<div>
									<h3 class="text-lg font-semibold text-fg">
										{sprint.name}
									</h3>
									<p class="text-sm text-fg-muted">
										{formatRange(sprint)}
									</p>
								</div>
								<span
									class="inline-flex w-fit rounded px-2 py-1 text-xs font-medium {getStatusClass(
										sprint.status
									)}"
								>
									{$t(`cycles.status.${sprint.status}`)}
								</span>
							</div>

							{#if sprint.description}
								<p class="mb-3 text-sm text-fg">
									{$t('cycles.description')}: {sprint.description}
								</p>
							{/if}
							{@render issuePanel(sprint)}

							<div class="mt-4 flex flex-wrap gap-2">
								<button
									onclick={() => openEditModal(sprint)}
									class="rounded-control border border-fg px-3 py-1.5 text-sm text-fg hover:bg-surface"
								>
									{$t('common.edit')}
								</button>
								{#if canManage}
									<button
										onclick={() => removeSprint(sprint)}
										class="rounded-control border px-3 py-1.5 text-sm text-danger hover:bg-danger-soft"
									>
										{$t('common.delete')}
									</button>
									<button
										onclick={() => openEndDialog(sprint, 'cancelled')}
										class="rounded-control border px-3 py-1.5 text-sm text-danger hover:bg-danger-soft"
									>
										{$t('cycles.cancelSprint')}
									</button>
									<button
										onclick={() => startSprint(sprint)}
										class="rounded-control rf-filled bg-primary px-3 py-1.5 text-sm text-on-primary"
									>
										{$t('cycles.startSprint')}
									</button>
								{/if}
							</div>
						</div>
					{/each}
				</div>
			{/if}
		</section>

		<section
			class="rounded-card border border-line bg-raised p-4 sm:p-5"
			data-testid="cycles-completed-group"
		>
			<details>
				<summary class="cursor-pointer list-none text-lg font-semibold text-fg">
					<span class="inline-flex items-center gap-2"
						>{$t('cycles.completedList')}
						<span class="text-sm text-fg-muted">({completedSprints.length})</span></span
					>
				</summary>
				<div class="mt-4 space-y-3">
					{#if completedSprints.length === 0}
						<p class="text-sm text-fg-muted">{$t('cycles.noCompleted')}</p>
					{:else}
						{#each completedSprints as sprint (sprint.id)}
							{@render pastCard(sprint)}
						{/each}
					{/if}
				</div>
			</details>
		</section>
		<section
			class="rounded-card border border-line bg-raised p-4 sm:p-5"
			data-testid="cycles-cancelled-group"
		>
			<details>
				<summary class="cursor-pointer list-none text-lg font-semibold text-fg">
					<span class="inline-flex items-center gap-2"
						>{$t('cycles.cancelledList')}
						<span class="text-sm text-fg-muted">({cancelledSprints.length})</span></span
					>
				</summary>
				<div class="mt-4 space-y-3">
					{#if cancelledSprints.length === 0}
						<p class="text-sm text-fg-muted">{$t('cycles.noCancelled')}</p>
					{:else}
						{#each cancelledSprints as sprint (sprint.id)}
							{@render pastCard(sprint)}
						{/each}
					{/if}
				</div>
			</details>
		</section>
	{/if}
</div>

{#snippet issuePanel(sprint: Sprint)}
	{@const progress = sprintProgress[sprint.id]}
	{@const list = issueLists[sprint.id]}
	<div class="mt-4 rounded-control border border-line bg-surface p-3">
		<div class="flex flex-wrap items-center justify-between gap-2">
			<p class="text-xs font-medium uppercase tracking-wide text-fg-muted">
				{$t('cycles.linkedIssues', { values: { count: progress?.total ?? 0 } })}
			</p>
			{#if (progress?.total ?? 0) > 0}
				<button
					onclick={() => toggleIssues(sprint.id)}
					aria-expanded={expanded.has(sprint.id)}
					class="rounded-control border border-fg px-2 py-1 text-xs text-fg hover:bg-surface"
					data-testid="cycles-toggle-issues"
				>
					{expanded.has(sprint.id) ? $t('cycles.hideIssues') : $t('cycles.showIssues')}
				</button>
			{/if}
		</div>
		{#if (progress?.total ?? 0) === 0}
			<p class="mt-2 text-sm text-fg-muted">{$t('cycles.noLinkedIssue')}</p>
		{:else if expanded.has(sprint.id)}
			<div class="mt-2 space-y-2" data-testid="cycles-issue-list">
				{#each list?.items ?? [] as issue (issue.id)}
					<a
						href={resolve('/(app)/workspace/[workspaceId]/projects/[projectId]/issues/[issueId]', {
							workspaceId,
							projectId,
							issueId: issue.id
						})}
						class="block rounded border border-line bg-raised px-3 py-2 text-sm hover:border-line-strong hover:bg-surface"
					>
						<p class="font-medium text-fg">{issue.title}</p>
						<p class="font-mono text-xs text-fg-muted">{formatIssueCode(issue)}</p>
					</a>
				{/each}
				{#if list?.failed}
					<p class="text-sm text-danger" role="alert">{$t('cycles.loadIssuesFailed')}</p>
				{/if}
				{#if list && hasMoreIssues(list)}
					<button
						onclick={() => void loadIssues(sprint.id)}
						disabled={list.loading}
						class="rounded-control border border-fg px-3 py-1.5 text-sm text-fg hover:bg-surface disabled:text-fg-muted"
						data-testid="cycles-load-more"
					>
						{list.loading ? $t('common.loading') : $t('cycles.loadMore')}
					</button>
				{/if}
			</div>
		{/if}
	</div>
{/snippet}

{#snippet pastCard(sprint: Sprint)}
	<div class="rounded-card border border-line p-4">
		<div class="mb-2 flex items-start justify-between gap-2">
			<div>
				<p class="font-medium text-fg">{sprint.name}</p>
				<p class="text-sm text-fg-muted">
					{formatRange(sprint)}
				</p>
			</div>
			<span
				class="inline-flex rounded px-2 py-1 text-xs font-medium {getStatusClass(sprint.status)}"
			>
				{$t(`cycles.status.${sprint.status}`)}
			</span>
		</div>
		<ProgressBar
			value={sprintProgress[sprint.id]?.percent ?? 0}
			label={$t('cycles.completedLabel', {
				values: {
					done: sprintProgress[sprint.id]?.done ?? 0,
					total: sprintProgress[sprint.id]?.total ?? 0
				}
			})}
		/>
		{@render issuePanel(sprint)}
	</div>
{/snippet}

{#snippet createFooter()}
	<button
		onclick={() => (showCreateModal = false)}
		class="rounded-control border border-fg px-4 py-2 text-sm text-fg hover:bg-surface"
	>
		{$t('common.cancel')}
	</button>
	<button
		onclick={submitCreate}
		disabled={saving}
		class="rounded-control rf-filled bg-primary px-4 py-2 text-sm text-on-primary disabled:bg-surface disabled:text-fg-muted disabled:cursor-not-allowed"
	>
		{saving ? $t('common.creating') : $t('common.create')}
	</button>
{/snippet}

<Modal
	bind:open={showCreateModal}
	title={$t('cycles.create')}
	maxWidthClass="max-w-lg"
	footer={createFooter}
>
	<div class="space-y-4">
		<div>
			<label for="create-name" class="mb-1 block text-sm font-medium text-fg"
				>{$t('cycles.name')}</label
			>
			<input
				id="create-name"
				type="text"
				bind:value={createForm.name}
				bind:this={createNameInput}
				oninput={() => (createNameError = '')}
				aria-invalid={createNameError ? 'true' : undefined}
				aria-describedby={createNameError ? 'create-name-error' : undefined}
				placeholder={$t('cycles.namePlaceholder')}
				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg placeholder:text-fg-muted"
			/>
			{#if createNameError}
				<p id="create-name-error" role="alert" class="mt-1 text-sm text-danger">
					{createNameError}
				</p>
			{/if}
		</div>
		<div>
			<label for="create-description" class="mb-1 block text-sm font-medium text-fg"
				>{$t('cycles.description')}</label
			>
			<textarea
				id="create-description"
				bind:value={createForm.description}
				rows="3"
				placeholder={$t('cycles.descriptionPlaceholder')}
				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg placeholder:text-fg-muted"
			></textarea>
		</div>
		<div class="grid grid-cols-1 gap-4 sm:grid-cols-2">
			<div>
				<label for="create-start-date" class="mb-1 block text-sm font-medium text-fg"
					>{$t('cycles.startDate')}</label
				>
				<input
					id="create-start-date"
					type="date"
					bind:value={createForm.start_date}
					class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
				/>
			</div>
			<div>
				<label for="create-end-date" class="mb-1 block text-sm font-medium text-fg"
					>{$t('cycles.endDate')}</label
				>
				<input
					id="create-end-date"
					type="date"
					bind:value={createForm.end_date}
					class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
				/>
			</div>
		</div>
	</div>
</Modal>

{#snippet editFooter()}
	<button
		onclick={() => {
			showEditModal = false;
			editingSprint = null;
		}}
		class="rounded-control border border-fg px-4 py-2 text-sm text-fg hover:bg-surface"
	>
		{$t('common.cancel')}
	</button>
	<button
		onclick={submitEdit}
		disabled={saving}
		class="rounded-control rf-filled bg-primary px-4 py-2 text-sm text-on-primary disabled:bg-surface disabled:text-fg-muted disabled:cursor-not-allowed"
	>
		{saving ? $t('common.saving') : $t('common.save')}
	</button>
{/snippet}

<Modal
	bind:open={showEditModal}
	title={$t('cycles.edit')}
	maxWidthClass="max-w-lg"
	onclose={() => {
		editingSprint = null;
	}}
	footer={editFooter}
>
	<div class="space-y-4">
		<div>
			<label for="edit-name" class="mb-1 block text-sm font-medium text-fg"
				>{$t('cycles.name')}</label
			>
			<input
				id="edit-name"
				type="text"
				bind:value={editForm.name}
				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
			/>
		</div>
		<div>
			<label for="edit-description" class="mb-1 block text-sm font-medium text-fg"
				>{$t('cycles.description')}</label
			>
			<textarea
				id="edit-description"
				bind:value={editForm.description}
				rows="3"
				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
			></textarea>
		</div>
		<div class="grid grid-cols-1 gap-4 sm:grid-cols-2">
			<div>
				<label for="edit-start-date" class="mb-1 block text-sm font-medium text-fg"
					>{$t('cycles.startDate')}</label
				>
				<input
					id="edit-start-date"
					type="date"
					bind:value={editForm.start_date}
					class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
				/>
			</div>
			<div>
				<label for="edit-end-date" class="mb-1 block text-sm font-medium text-fg"
					>{$t('cycles.endDate')}</label
				>
				<input
					id="edit-end-date"
					type="date"
					bind:value={editForm.end_date}
					class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
				/>
			</div>
		</div>
	</div>
</Modal>

{#snippet endFooter()}
	<button
		onclick={() => (showEndDialog = false)}
		class="rounded-control border border-fg px-4 py-2 text-sm text-fg hover:bg-surface"
	>
		{$t('cycles.rolloverKeep')}
	</button>
	<button
		onclick={submitEnd}
		disabled={saving}
		data-testid="cycles-end-confirm"
		class="rounded-control rf-filled bg-primary px-4 py-2 text-sm text-on-primary disabled:bg-surface disabled:text-fg-muted disabled:cursor-not-allowed"
	>
		{endDialog?.status === 'cancelled' ? $t('cycles.cancelSprint') : $t('cycles.completeSprint')}
	</button>
{/snippet}

<Modal
	bind:open={showEndDialog}
	title={endDialog
		? $t(
				endDialog.status === 'cancelled'
					? 'cycles.rolloverCancelTitle'
					: 'cycles.rolloverCompleteTitle',
				{ values: { name: endDialog.sprint.name } }
			)
		: ''}
	maxWidthClass="max-w-lg"
	onclose={() => {
		endDialog = null;
	}}
	footer={endFooter}
>
	<div class="space-y-4">
		<p class="text-sm text-fg-muted">{$t('cycles.rolloverBody')}</p>
		<div>
			<label for="end-rollover" class="mb-1 block text-sm font-medium text-fg"
				>{$t('cycles.rolloverTarget')}</label
			>
			<select
				id="end-rollover"
				bind:value={rolloverChoice}
				data-testid="cycles-end-target"
				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
			>
				<option value="">{$t('cycles.rolloverBacklog')}</option>
				{#each endTargets as target (target.id)}
					<option value={target.id}>{target.name}</option>
				{/each}
			</select>
		</div>
	</div>
</Modal>
