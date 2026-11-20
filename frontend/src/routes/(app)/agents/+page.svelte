<script lang="ts">
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import { page } from '$app/state';
	import { workspacesApi } from '#lib/api/workspaces.js';
	import { authStore } from '#lib/stores/auth.js';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import { isWorkspaceAdminRole, type WorkspaceRole } from '#lib/workspace/home.js';
	import { WorkspaceRoleTracker } from '#lib/workspace/role.js';
	import AgentsLists from '#lib/shell/column/AgentsLists.svelte';
	import { defaultColumnApis } from '#lib/shell/column/column-apis.js';
	import { ColumnData } from '#lib/shell/column/column-data.svelte.js';
	import { getShell } from '#lib/shell/context.js';
	import { ShellState } from '#lib/shell/shell-state.svelte.js';

	// Inside the app shell the column shows these lists next to the page and this page only repeats
	// them in the one-column narrow layout. Without a shell around it, the page shows them itself.
	const shell = getShell();
	const own = shell ?? new ShellState({ userId: $authStore.user?.id ?? null });
	const scope = $derived(own.scope);
	const data = new ColumnData(defaultColumnApis);

	let role = $state<WorkspaceRole | null>(null);
	const tracker = new WorkspaceRoleTracker(
		(workspaceId) => workspacesApi.getMembers(workspaceId),
		(next) => {
			role = next;
		}
	);
	const userId = $derived($authStore.user?.id ?? null);
	const workspaceId = $derived(scope === 'personal' ? null : scope);
	$effect(() => {
		void tracker.select(workspaceId, userId);
	});
	const workspaceAdmin = $derived(isWorkspaceAdminRole(role) && !!workspaceId);

	onMount(() => void data.loadWorkspaces());
</script>

<svelte:head>
	<title>{$t('shell.pageTitle.agents')}</title>
</svelte:head>

<div class="page" data-testid="agents-page">
	<h1>{$t('shell.destinations.agents')}</h1>
	<StateBlock kind="empty" message={$t('shell.column.agents.choose')} />
	<nav
		class="lists"
		class:narrow-only={shell !== null}
		aria-label={$t('shell.destinations.agents')}
		data-agents-lists
	>
		<AgentsLists {scope} url={page.url} {workspaceAdmin} {data} />
	</nav>
</div>

<style>
	.page {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-6);
		max-width: var(--rf-layout-reading);
	}
	h1 {
		margin: 0;
		color: var(--rf-text);
		font-size: var(--rf-size-page);
		line-height: var(--rf-leading-page);
		font-weight: var(--rf-weight-semibold);
	}
	.lists {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-4);
	}
	@media (min-width: 801px) {
		.lists.narrow-only {
			display: none;
		}
	}
</style>
