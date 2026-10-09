<script lang="ts">
	// Workspace home (`/workspace/{id}`, task FP-N6): the workspace's entry point into its product
	// surfaces. Card visibility, the recent-project cut and the loader live in
	// `$lib/workspace/home`; the Flow flag comes from the shared fail-closed `flowFeatureStore`,
	// which the app layout already refreshes for this workspace (no second flag request here).
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import { page } from '$app/stores';
	import { resolve } from '$app/paths';
	import EmptyState from '$lib/components/EmptyState.svelte';
	import type { Project } from '$lib/api/projects';
	import type { Workspace } from '$lib/api/workspaces';
	import { flowFeatureStore } from '$lib/stores/flow-feature';
	import { requireRouteParam } from '$lib/utils/route-params';
	import {
		WORKSPACE_ADMIN_LINKS,
		defaultWorkspaceHomeDeps,
		isWorkspaceAdminRole,
		loadWorkspaceHome,
		rememberWorkspaceName,
		workspaceHomeCards,
		type WorkspaceRole
	} from '$lib/workspace/home';

	const workspaceId = requireRouteParam($page.params.workspaceId, 'workspaceId');

	let view = $state<'loading' | 'ready' | 'not_found' | 'failed'>('loading');
	let workspace = $state.raw<Workspace | null>(null);
	let role = $state<WorkspaceRole | null>(null);
	let projectTotal = $state(0);
	let recent = $state.raw<readonly Project[]>([]);
	let projectsFailed = $state(false);

	const isAdmin = $derived(isWorkspaceAdminRole(role));
	const cards = $derived(workspaceHomeCards($flowFeatureStore[workspaceId] === true, isAdmin));

	const linkClass =
		'inline-flex min-h-11 items-center rounded-md text-sm font-medium text-blue-700 hover:underline focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:text-blue-300';
	const cardClass =
		'flex flex-col gap-3 rounded-lg border border-slate-200 bg-white p-6 dark:border-slate-700 dark:bg-slate-800';

	async function load() {
		view = 'loading';
		const outcome = await loadWorkspaceHome(workspaceId, defaultWorkspaceHomeDeps);
		if (outcome.status !== 'ready') {
			view = outcome.status;
			return;
		}
		workspace = outcome.workspace;
		role = outcome.role;
		projectTotal = outcome.projectTotal;
		recent = outcome.recent;
		projectsFailed = outcome.projectsFailed;
		rememberWorkspaceName(workspaceId, outcome.workspace.name);
		view = 'ready';
	}

	onMount(() => {
		void load();
	});
</script>

<div class="mx-auto max-w-6xl space-y-6">
	{#if view === 'loading'}
		<div
			class="rounded-lg border border-slate-200 bg-white p-6 text-slate-500 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-400"
			role="status"
			aria-live="polite"
		>
			{$t('common.loading')}
		</div>
	{:else if view === 'not_found'}
		<div data-testid="workspace-home-not-found" role="status" aria-live="polite" class="space-y-4">
			<EmptyState
				icon="office"
				title={$t('workspace.home.notFoundTitle')}
				description={$t('workspace.home.notFoundBody')}
			>
				{#snippet action()}
					<a href={resolve('/(app)/workspace')} class={linkClass}
						>{$t('workspace.home.backToList')}</a
					>
				{/snippet}
			</EmptyState>
		</div>
	{:else if view === 'failed'}
		<div
			class="flex flex-col gap-3 rounded-lg border border-red-200 bg-red-50 p-6 text-red-800 dark:border-red-500/40 dark:bg-red-500/20 dark:text-red-300"
			role="alert"
		>
			<p>{$t('workspace.home.loadFailed')}</p>
			<button
				type="button"
				class="min-h-11 self-start rounded-md border border-red-300 px-4 text-sm font-medium focus:outline-none focus-visible:ring-2 focus-visible:ring-red-500 dark:border-red-700"
				onclick={() => void load()}
			>
				{$t('common.retry')}
			</button>
		</div>
	{:else if workspace}
		<header class="space-y-2">
			<div class="flex flex-wrap items-center gap-3">
				<h1
					class="text-2xl font-bold break-words text-slate-900 dark:text-slate-100"
					data-testid="workspace-home-name"
				>
					{workspace.name}
				</h1>
				{#if role}
					<span
						class="rounded-full border border-blue-200 bg-blue-50 px-2.5 py-0.5 text-xs font-medium text-blue-700 dark:border-blue-500/40 dark:bg-blue-500/20 dark:text-blue-300"
						data-testid="workspace-home-role"
					>
						<span class="sr-only">{$t('workspace.home.roleLabel')}: </span>{$t(`roles.${role}`)}
					</span>
				{/if}
			</div>
			<p class="text-sm text-slate-500 dark:text-slate-400">
				{$t('workspace.home.slugLabel')}:
				<code class="font-mono" data-testid="workspace-home-slug">{workspace.slug}</code>
			</p>
			{#if workspace.description}
				<p class="text-slate-600 dark:text-slate-300">{workspace.description}</p>
			{/if}
		</header>

		<section
			aria-label={$t('workspace.home.sections')}
			class="grid grid-cols-1 gap-6 md:grid-cols-2 lg:grid-cols-3"
		>
			<div class={cardClass} data-testid="workspace-home-projects">
				<div class="flex items-baseline justify-between gap-3">
					<h2 class="text-lg font-semibold text-slate-900 dark:text-slate-100">
						{$t('workspace.home.projectsTitle')}
					</h2>
					{#if !projectsFailed}
						<span
							class="text-sm text-slate-500 dark:text-slate-400"
							data-testid="workspace-home-project-total"
						>
							{$t('workspace.home.projectTotal', { values: { count: projectTotal } })}
						</span>
					{/if}
				</div>
				{#if projectsFailed}
					<p class="text-sm text-red-700 dark:text-red-300" role="alert">
						{$t('workspace.home.projectsLoadFailed')}
					</p>
				{:else if recent.length === 0}
					<div data-testid="workspace-home-empty">
						<EmptyState
							icon="box"
							title={$t('workspace.home.emptyTitle')}
							description={$t('workspace.home.emptyBody')}
						>
							{#snippet action()}
								<a
									href={resolve('/(app)/workspace/[workspaceId]/projects', { workspaceId })}
									class={linkClass}
								>
									{$t('workspace.home.newProject')}
								</a>
							{/snippet}
						</EmptyState>
					</div>
				{:else}
					<h3 class="text-sm font-medium text-slate-600 dark:text-slate-300">
						{$t('workspace.home.recentProjects')}
					</h3>
					<ul class="space-y-1" data-testid="workspace-home-recent">
						{#each recent as project (project.id)}
							<li>
								<a
									href={resolve('/(app)/workspace/[workspaceId]/projects/[projectId]', {
										workspaceId,
										projectId: project.id
									})}
									class={`${linkClass} break-all`}
								>
									{project.name}
								</a>
							</li>
						{/each}
					</ul>
				{/if}
				<div class="mt-auto">
					<a
						href={resolve('/(app)/workspace/[workspaceId]/projects', { workspaceId })}
						class={linkClass}
					>
						{$t('workspace.home.allProjects')}
					</a>
				</div>
			</div>

			{#if cards.flow !== 'hidden'}
				<div class={cardClass} data-testid="workspace-home-flow">
					<h2 class="text-lg font-semibold text-slate-900 dark:text-slate-100">
						{$t('workspace.home.flowTitle')}
					</h2>
					{#if cards.flow === 'open'}
						<p class="text-sm text-slate-600 dark:text-slate-300">
							{$t('workspace.home.flowOpenBody')}
						</p>
						<a
							href={resolve('/(app)/workspace/[workspaceId]/flow', { workspaceId })}
							class={linkClass}
						>
							{$t('workspace.home.flowOpen')}
						</a>
					{:else}
						<p class="text-sm text-slate-600 dark:text-slate-300">
							{$t('workspace.home.flowDisabledBody')}
						</p>
						<a
							href={resolve('/(app)/workspace/[workspaceId]/settings/flow', { workspaceId })}
							class={linkClass}
						>
							{$t('workspace.home.flowEnable')}
						</a>
					{/if}
				</div>
			{/if}

			{#if cards.admin}
				<div class={cardClass} data-testid="workspace-home-admin">
					<h2 class="text-lg font-semibold text-slate-900 dark:text-slate-100">
						{$t('workspace.home.adminTitle')}
					</h2>
					<ul class="space-y-1">
						{#each WORKSPACE_ADMIN_LINKS as link (link.key)}
							<li>
								<a href={resolve(link.route, { workspaceId })} class={linkClass}
									>{$t(link.labelKey)}</a
								>
							</li>
						{/each}
					</ul>
				</div>
			{/if}
		</section>
	{/if}
</div>
