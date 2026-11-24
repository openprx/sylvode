<svelte:options runes />

<script lang="ts">
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { locale, t } from 'svelte-i18n';
	import { milestonesApi, type Milestone, type MilestoneStatus } from '#lib/api/milestones.js';
	import { workspacesApi } from '#lib/api/workspaces.js';
	import { describeResult } from '#lib/i18n/describe.js';
	import MilestoneDialog from '#lib/milestones/MilestoneDialog.svelte';
	import {
		canManageMilestones,
		formatTargetDate,
		isOverdue,
		milestoneFailureMessage,
		milestoneProgressOf
	} from '#lib/milestones/rules.js';
	import ProgressBar from '#lib/components/ProgressBar.svelte';
	import { authStore } from '#lib/stores/auth.js';
	import { toast } from '#lib/stores/toast.js';
	import Pagination from '#lib/ui/pagination/Pagination.svelte';
	import Segmented from '#lib/ui/segmented/Segmented.svelte';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import { confirmAction } from '#lib/ui/dialog/confirm.js';
	import { isMissingResult } from '#lib/utils/api-helpers.js';
	import { requireRouteParam } from '#lib/utils/route-params.js';
	import type { WorkspaceRole } from '#lib/workspace/home.js';
	import { WorkspaceRoleTracker } from '#lib/workspace/role.js';

	const PAGE_SIZE = 20;

	const workspaceId = $derived(requireRouteParam(page.params.workspaceId, 'workspaceId'));

	let loading = $state(true);
	let failure = $state<{ message: string; missing: boolean } | null>(null);
	let milestones = $state<Milestone[]>([]);
	let total = $state(0);
	let pageNumber = $state(1);
	let status = $state<MilestoneStatus>('open');
	// Reads overlap when the status or page changes quickly: only the latest one is kept.
	let generation = 0;

	// Creating, editing, closing and deleting are for owners and admins; the controls are not
	// shown to anyone else (the server refuses the calls too).
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
	const canManage = $derived(canManageMilestones(role));

	let dialogOpen = $state(false);
	let editing = $state<Milestone | null>(null);
	let busyId = $state<string | null>(null);

	const statusOptions = $derived([
		{ value: 'open' as const, label: $t('milestones.filter.open') },
		{ value: 'closed' as const, label: $t('milestones.filter.closed') }
	]);

	async function load(target = pageNumber) {
		const mine = ++generation;
		loading = true;
		failure = null;
		const response = await milestonesApi.list(workspaceId, {
			status,
			page: target,
			per_page: PAGE_SIZE
		});
		if (mine !== generation) return;
		if (response.code !== 0 || !response.data) {
			failure = { message: describeResult(response), missing: isMissingResult(response) };
			milestones = [];
			total = 0;
		} else {
			milestones = response.data.items ?? [];
			total = response.data.total ?? 0;
			pageNumber = response.data.page || target;
		}
		loading = false;
	}

	onMount(() => {
		void load(1);
	});

	function chooseStatus(next: MilestoneStatus) {
		status = next;
		void load(1);
	}

	function openCreate() {
		editing = null;
		dialogOpen = true;
	}

	function openEdit(milestone: Milestone) {
		editing = milestone;
		dialogOpen = true;
	}

	async function setStatus(milestone: Milestone, next: MilestoneStatus) {
		if (busyId) return;
		busyId = milestone.id;
		const response = await milestonesApi.update(milestone.id, { status: next });
		busyId = null;
		if (response.code !== 0) {
			toast.error(milestoneFailureMessage(get(t), response));
			return;
		}
		toast.success(
			get(t)(next === 'closed' ? 'milestones.closedToast' : 'milestones.reopenedToast')
		);
		await load(pageNumber);
	}

	async function remove(milestone: Milestone) {
		const values = { values: { name: milestone.name } };
		const confirmed = await confirmAction({
			tone: 'destructive',
			title: get(t)('milestones.deleteTitle', values),
			body: get(t)('milestones.deleteBody'),
			confirmLabel: get(t)('milestones.deleteConfirm', values)
		});
		if (!confirmed || busyId) return;
		busyId = milestone.id;
		const response = await milestonesApi.delete(milestone.id);
		busyId = null;
		if (response.code !== 0) {
			toast.error(milestoneFailureMessage(get(t), response));
			return;
		}
		toast.success(get(t)('milestones.deletedToast'));
		// The last row of a page leaves: step back to the page before it.
		await load(milestones.length === 1 && pageNumber > 1 ? pageNumber - 1 : pageNumber);
	}

	async function saved() {
		// A milestone saved in the other status group is not on this page: show the open ones.
		await load(editing ? pageNumber : 1);
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
			<span class="font-medium text-fg">{$t('milestones.title')}</span>
		</div>
		<div class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
			<div>
				<h1 class="text-2xl font-semibold text-fg">{$t('milestones.title')}</h1>
				<p class="mt-1 text-sm text-fg-muted">{$t('milestones.subtitle')}</p>
			</div>
			{#if canManage}
				<button
					type="button"
					onclick={openCreate}
					data-testid="milestone-create"
					class="rounded-control rf-filled bg-primary px-4 py-2 text-sm font-medium text-on-primary"
				>
					+ {$t('milestones.create')}
				</button>
			{/if}
		</div>
		<div class="mt-4">
			<Segmented
				options={statusOptions}
				value={status}
				onchange={chooseStatus}
				label={$t('milestones.filter.label')}
			/>
		</div>
		{#if role && !canManage}
			<p class="mt-3 text-sm text-fg-muted" data-testid="milestones-read-only">
				{$t('milestones.readOnly')}
			</p>
		{/if}
	</div>

	{#if loading}
		<div
			class="rounded-card border border-line bg-raised p-8 text-center text-fg-muted"
			role="status"
		>
			<div
				class="mx-auto mb-3 h-6 w-6 animate-spin rounded-full border-2 border-fg border-t-transparent"
			></div>
			{$t('milestones.loading')}
		</div>
	{:else if failure}
		<StateBlock
			kind={failure.missing ? 'forbidden' : 'loadFailed'}
			thing={$t('milestones.things.milestones')}
			message={failure.message || undefined}
			action={{ run: () => void load(pageNumber) }}
			data-testid="milestones-load-failed"
		/>
	{:else if milestones.length === 0}
		<StateBlock
			kind="empty"
			title={$t(
				status === 'open' && pageNumber === 1 && total === 0
					? 'milestones.empty'
					: status === 'open'
						? 'milestones.emptyOpen'
						: 'milestones.emptyClosed'
			)}
			message={status === 'open'
				? $t(canManage ? 'milestones.emptyAdmin' : 'milestones.emptyMember')
				: ''}
			data-testid="milestones-empty"
		/>
	{:else}
		<ul class="space-y-3" data-testid="milestones-list">
			{#each milestones as milestone (milestone.id)}
				{@const progress = milestoneProgressOf(milestone)}
				<li
					class="rounded-card border border-line bg-raised p-4 sm:p-5"
					data-milestone-id={milestone.id}
				>
					<div class="flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
						<div class="min-w-0">
							<h2 class="text-lg font-semibold text-fg">
								<a
									href={resolve('/(app)/workspace/[workspaceId]/milestones/[milestoneId]', {
										workspaceId,
										milestoneId: milestone.id
									})}
									class="hover:underline">{milestone.name}</a
								>
							</h2>
							<p class="text-sm text-fg-muted">
								{#if milestone.target_date}
									{$t('milestones.targetDate')}: {formatTargetDate(
										milestone.target_date,
										$locale ?? null
									)}
									{#if isOverdue(milestone)}
										<span
											class="ml-2 rounded bg-danger-soft px-2 py-0.5 text-xs font-medium text-danger"
											>{$t('milestones.overdue')}</span
										>
									{/if}
								{:else}
									{$t('milestones.noTargetDate')}
								{/if}
							</p>
							{#if milestone.description}
								<p class="mt-2 text-sm text-fg">{milestone.description}</p>
							{/if}
						</div>
						{#if canManage}
							<div class="flex shrink-0 flex-wrap gap-2">
								<button
									type="button"
									onclick={() => openEdit(milestone)}
									disabled={busyId !== null}
									class="rounded-control border border-fg px-3 py-1.5 text-sm text-fg hover:bg-surface disabled:text-fg-muted"
								>
									{$t('milestones.editAction')}
								</button>
								<button
									type="button"
									onclick={() =>
										setStatus(milestone, milestone.status === 'open' ? 'closed' : 'open')}
									disabled={busyId !== null}
									class="rounded-control border border-fg px-3 py-1.5 text-sm text-fg hover:bg-surface disabled:text-fg-muted"
								>
									{$t(milestone.status === 'open' ? 'milestones.close' : 'milestones.reopen')}
								</button>
								<button
									type="button"
									onclick={() => remove(milestone)}
									disabled={busyId !== null}
									class="rounded-control border px-3 py-1.5 text-sm text-danger hover:bg-danger-soft disabled:text-fg-muted"
								>
									{$t('milestones.deleteAction')}
								</button>
							</div>
						{/if}
					</div>
					<div class="mt-3">
						{#if progress.total === 0}
							<p class="text-sm text-fg-muted">{$t('milestones.noItems')}</p>
						{:else}
							<ProgressBar
								value={progress.percent}
								label={$t('milestones.progress', {
									values: { done: progress.done, total: progress.total }
								})}
							/>
							<ul class="mt-2 flex flex-wrap gap-x-4 gap-y-1 text-xs text-fg-muted">
								{#each milestone.progress.by_project as entry (entry.project_id)}
									<li>
										{$t('milestones.projectCount', {
											values: { key: entry.project_key, done: entry.done, total: entry.total }
										})}
									</li>
								{/each}
							</ul>
						{/if}
					</div>
				</li>
			{/each}
		</ul>
		<Pagination page={pageNumber} pageSize={PAGE_SIZE} {total} onpage={(next) => void load(next)} />
	{/if}
</div>

<MilestoneDialog bind:open={dialogOpen} {workspaceId} milestone={editing} onsaved={saved} />
