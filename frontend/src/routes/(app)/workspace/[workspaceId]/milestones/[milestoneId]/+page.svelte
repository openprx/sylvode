<svelte:options runes />

<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { locale, t } from 'svelte-i18n';
	import {
		milestonesApi,
		type MilestoneDetail,
		type MilestoneStatus
	} from '#lib/api/milestones.js';
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
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import { confirmAction } from '#lib/ui/dialog/confirm.js';
	import { isMissingResult } from '#lib/utils/api-helpers.js';
	import { requireRouteParam } from '#lib/utils/route-params.js';
	import type { WorkspaceRole } from '#lib/workspace/home.js';
	import { WorkspaceRoleTracker } from '#lib/workspace/role.js';

	const workspaceId = $derived(requireRouteParam(page.params.workspaceId, 'workspaceId'));
	const milestoneId = $derived(requireRouteParam(page.params.milestoneId, 'milestoneId'));

	let loading = $state(true);
	let failure = $state<{ message: string; missing: boolean } | null>(null);
	let milestone = $state<MilestoneDetail | null>(null);
	let busy = $state(false);
	let generation = 0;

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

	const progress = $derived(milestone ? milestoneProgressOf(milestone) : null);

	async function load() {
		const mine = ++generation;
		loading = true;
		failure = null;
		const response = await milestonesApi.get(milestoneId);
		if (mine !== generation) return;
		if (response.code !== 0 || !response.data) {
			failure = { message: describeResult(response), missing: isMissingResult(response) };
			milestone = null;
		} else {
			milestone = response.data;
		}
		loading = false;
	}

	onMount(() => {
		void load();
	});

	async function setStatus(next: MilestoneStatus) {
		if (!milestone || busy) return;
		busy = true;
		const response = await milestonesApi.update(milestone.id, { status: next });
		busy = false;
		if (response.code !== 0) {
			toast.error(milestoneFailureMessage(get(t), response));
			return;
		}
		toast.success(
			get(t)(next === 'closed' ? 'milestones.closedToast' : 'milestones.reopenedToast')
		);
		await load();
	}

	async function remove() {
		if (!milestone || busy) return;
		const values = { values: { name: milestone.name } };
		const confirmed = await confirmAction({
			tone: 'destructive',
			title: get(t)('milestones.deleteTitle', values),
			body: get(t)('milestones.deleteBody'),
			confirmLabel: get(t)('milestones.deleteConfirm', values)
		});
		if (!confirmed) return;
		busy = true;
		const response = await milestonesApi.delete(milestone.id);
		busy = false;
		if (response.code !== 0) {
			toast.error(milestoneFailureMessage(get(t), response));
			return;
		}
		toast.success(get(t)('milestones.deletedToast'));
		await goto(resolve('/(app)/workspace/[workspaceId]/milestones', { workspaceId }));
	}
</script>

<div class="mx-auto max-w-5xl space-y-6">
	<div class="rounded-card border border-line bg-raised p-4 sm:p-5">
		<div class="mb-4 text-sm text-fg-muted">
			<a href={resolve('/(app)/workspace')} class="hover:underline">{$t('nav.myWorkspace')}</a>
			<span class="mx-1 text-fg-muted">/</span>
			<a
				href={resolve('/(app)/workspace/[workspaceId]/milestones', { workspaceId })}
				class="hover:underline">{$t('milestones.title')}</a
			>
			<span class="mx-1 text-fg-muted">/</span>
			<span class="font-medium text-fg">{milestone?.name ?? $t('milestones.pageTitle.detail')}</span
			>
		</div>

		{#if loading}
			<p class="text-sm text-fg-muted" role="status">{$t('milestones.loading')}</p>
		{:else if failure}
			<StateBlock
				kind={failure.missing ? 'notFound' : 'loadFailed'}
				thing={$t('milestones.things.milestones')}
				object={$t('milestones.pageTitle.detail')}
				place={$t('milestones.back')}
				message={failure.missing ? $t('milestones.notFound') : failure.message || undefined}
				headingLevel={1}
				action={failure.missing
					? {
							label: $t('milestones.back'),
							run: () =>
								void goto(resolve('/(app)/workspace/[workspaceId]/milestones', { workspaceId }))
						}
					: { run: () => void load() }}
				data-testid="milestone-load-failed"
			/>
		{:else if milestone}
			<div class="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
				<div class="min-w-0">
					<h1 class="text-2xl font-semibold text-fg">{milestone.name}</h1>
					<p class="mt-1 flex flex-wrap items-center gap-2 text-sm text-fg-muted">
						<span
							class="inline-flex rounded px-2 py-0.5 text-xs font-medium {milestone.status ===
							'open'
								? 'bg-success-soft text-success'
								: 'bg-surface text-fg'}"
							data-testid="milestone-status"
						>
							{$t(`milestones.status.${milestone.status}`)}
						</span>
						{#if milestone.target_date}
							<span
								>{$t('milestones.targetDate')}: {formatTargetDate(
									milestone.target_date,
									$locale ?? null
								)}</span
							>
							{#if isOverdue(milestone)}
								<span class="rounded bg-danger-soft px-2 py-0.5 text-xs font-medium text-danger"
									>{$t('milestones.overdue')}</span
								>
							{/if}
						{:else}
							<span>{$t('milestones.noTargetDate')}</span>
						{/if}
					</p>
					{#if milestone.description}
						<p class="mt-3 whitespace-pre-wrap text-sm text-fg">{milestone.description}</p>
					{/if}
				</div>
				{#if canManage}
					<div class="flex shrink-0 flex-wrap gap-2">
						<button
							type="button"
							onclick={() => (dialogOpen = true)}
							disabled={busy}
							class="rounded-control border border-fg px-3 py-1.5 text-sm text-fg hover:bg-surface disabled:text-fg-muted"
						>
							{$t('milestones.editAction')}
						</button>
						<button
							type="button"
							onclick={() => setStatus(milestone?.status === 'open' ? 'closed' : 'open')}
							disabled={busy}
							class="rounded-control border border-fg px-3 py-1.5 text-sm text-fg hover:bg-surface disabled:text-fg-muted"
						>
							{$t(milestone.status === 'open' ? 'milestones.close' : 'milestones.reopen')}
						</button>
						<button
							type="button"
							onclick={remove}
							disabled={busy}
							class="rounded-control border px-3 py-1.5 text-sm text-danger hover:bg-danger-soft disabled:text-fg-muted"
						>
							{$t('milestones.deleteAction')}
						</button>
					</div>
				{/if}
			</div>

			<div class="mt-4">
				{#if progress && progress.total > 0}
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
				{:else}
					<p class="text-sm text-fg-muted">{$t('milestones.noItems')}</p>
				{/if}
			</div>
			{#if role && !canManage}
				<p class="mt-3 text-sm text-fg-muted" data-testid="milestone-read-only">
					{$t('milestones.readOnly')}
				</p>
			{/if}
		{/if}
	</div>

	{#if milestone}
		<section class="space-y-3" aria-labelledby="milestone-items-heading">
			<h2 id="milestone-items-heading" class="text-lg font-semibold text-fg">
				{$t('milestones.items')}
			</h2>
			{#if milestone.items_by_project.length === 0}
				<div class="rounded-card border border-line bg-raised p-6 text-sm text-fg-muted">
					{$t('milestones.itemsNone')}
				</div>
			{:else}
				{#each milestone.items_by_project as group (group.project_id)}
					<div
						class="rounded-card border border-line bg-raised p-4 sm:p-5"
						data-project-group={group.project_id}
					>
						<h3 class="mb-2 text-sm font-semibold text-fg">
							{$t('milestones.projectItems', {
								values: { key: group.project_key, count: group.items.length }
							})}
						</h3>
						<ul class="space-y-2">
							{#each group.items as item (item.id)}
								<li>
									<a
										href={resolve(
											'/(app)/workspace/[workspaceId]/projects/[projectId]/issues/[issueId]',
											{ workspaceId, projectId: group.project_id, issueId: item.id }
										)}
										class="flex items-baseline justify-between gap-3 rounded border border-line bg-surface px-3 py-2 text-sm hover:border-line-strong"
									>
										<span class="min-w-0">
											<span class="font-medium text-fg">{item.title}</span>
											<span class="ml-2 font-mono text-xs text-fg-muted">{item.identifier}</span>
										</span>
										{#if item.done}
											<span class="shrink-0 text-xs text-success">{$t('milestones.itemDone')}</span>
										{/if}
									</a>
								</li>
							{/each}
						</ul>
					</div>
				{/each}
				{#if milestone.truncated}
					<p class="text-sm text-fg-muted" data-testid="milestone-truncated">
						{$t('milestones.truncated')}
					</p>
				{/if}
			{/if}
		</section>
	{/if}
</div>

<MilestoneDialog {workspaceId} bind:open={dialogOpen} {milestone} onsaved={() => void load()} />
