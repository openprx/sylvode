<script lang="ts">
	// Workspace package export and import (`ui-surface-v1.md` "Package round-trip UI (v0.8)").
	// Workspace admins only; the server's admin checks on every package endpoint are the real
	// guard, this only hides the controls from everyone else.
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import { page } from '$app/stores';
	import { resolve } from '$app/paths';
	import EmptyState from '$lib/components/EmptyState.svelte';
	import PackageExportCard from '$lib/components/flow/PackageExportCard.svelte';
	import PackageImportWizard from '$lib/components/flow/PackageImportWizard.svelte';
	import { resolveWorkspaceAdmin } from '$lib/flow/flow-settings';
	import { listWorkspaceProjects, type WorkspaceProjectOption } from '$lib/flow/package-export';
	import { requireRouteParam } from '$lib/utils/route-params';

	const workspaceId = requireRouteParam($page.params.workspaceId, 'workspaceId');

	let view = $state<'loading' | 'ready' | 'forbidden'>('loading');
	let projects = $state.raw<WorkspaceProjectOption[]>([]);

	onMount(async () => {
		if (!(await resolveWorkspaceAdmin(workspaceId))) {
			view = 'forbidden';
			return;
		}
		projects = await listWorkspaceProjects(workspaceId);
		view = 'ready';
	});
</script>

<div class="mx-auto max-w-3xl space-y-6">
	<div>
		<a
			href={resolve('/(app)/workspace/[workspaceId]/settings/flow', { workspaceId })}
			class="inline-flex min-h-11 items-center text-sm text-blue-700 hover:underline focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:text-blue-300"
		>
			{$t('flow.package.backToSettings')}
		</a>
		<h1 class="text-2xl font-bold text-slate-900 dark:text-slate-100">
			{$t('flow.package.title')}
		</h1>
		<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">{$t('flow.package.subtitle')}</p>
	</div>

	{#if view === 'loading'}
		<div
			class="rounded-lg border border-slate-200 bg-white p-6 text-slate-500 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-400"
			role="status"
			aria-live="polite"
		>
			{$t('flow.package.loading')}
		</div>
	{:else if view === 'forbidden'}
		<div data-testid="flow-package-forbidden" role="status" aria-live="polite">
			<EmptyState
				icon="office"
				title={$t('flow.package.forbiddenTitle')}
				description={$t('flow.error.forbidden')}
			/>
		</div>
	{:else}
		<PackageExportCard {workspaceId} workspaceLabel={workspaceId} {projects} />
		<PackageImportWizard {workspaceId} {projects} />
	{/if}
</div>
