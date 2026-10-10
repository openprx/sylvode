<script lang="ts">
	// Workspace Flow operations panel (FP-N3): health, lag, integrity, maintenance drawer and
	// delivery replay. Lives under `settings/flow/` rather than `flow/` because it must work while
	// Flow is disabled and needs no navigator. All rules live in `lib/flow/operations-service.ts`;
	// this page only wires snapshots to components. Workspace admins only -- the server's
	// `require_flow_workspace_admin_access` is the real guard; this only hides the controls.
	import { onDestroy, onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import { page } from '$app/stores';
	import { resolve } from '$app/paths';
	import EmptyState from '$lib/components/EmptyState.svelte';
	import OpsHealthCard from '$lib/components/flow/OpsHealthCard.svelte';
	import OpsLagTable from '$lib/components/flow/OpsLagTable.svelte';
	import OpsIntegrityTable from '$lib/components/flow/OpsIntegrityTable.svelte';
	import OpsOperationDrawer from '$lib/components/flow/OpsOperationDrawer.svelte';
	import OpsReplayForm from '$lib/components/flow/OpsReplayForm.svelte';
	import {
		FlowOperationsService,
		OperationsPanelSession,
		type MaintenanceKind,
		type MaintenanceOperation,
		type OpsIntegrityLimit,
		type OpsPanelState
	} from '$lib/flow/operations-service';
	import { requireRouteParam } from '$lib/utils/route-params';

	const workspaceId = requireRouteParam($page.params.workspaceId, 'workspaceId');
	const service = new FlowOperationsService(workspaceId);
	const replay = service.replay();

	// Load order, the view decision and the unmount race live in `OperationsPanelSession`.
	let panel = $state.raw<OpsPanelState | null>(null);
	const session = new OperationsPanelSession(service, (next) => {
		panel = next;
	});
	panel = session.snapshot;

	let drawerOpen = $state(false);
	let operation = $state.raw<MaintenanceOperation | null>(null);

	onMount(() => {
		void session.open();
	});

	onDestroy(() => session.dispose());

	function changeLimit(limit: OpsIntegrityLimit) {
		void session.loadIntegrity(limit);
	}

	function openOperation(kind: MaintenanceKind, targetId: string, expectedHead: number | null) {
		operation = service.operation(kind, targetId, expectedHead);
		drawerOpen = true;
	}

	function closeDrawer() {
		drawerOpen = false;
		operation = null;
	}

	function afterExecute() {
		session.reloadAll();
	}
</script>

<div class="mx-auto max-w-5xl space-y-6">
	<div>
		<a
			href={resolve('/(app)/workspace/[workspaceId]/settings/flow', { workspaceId })}
			class="inline-flex min-h-11 items-center text-sm text-blue-700 hover:underline focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:text-blue-300"
		>
			{$t('flow.operations.backToSettings')}
		</a>
		<h1 class="text-2xl font-bold text-slate-900 dark:text-slate-100">
			{$t('flow.operations.title')}
		</h1>
		<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">{$t('flow.operations.subtitle')}</p>
	</div>

	{#if !panel || panel.view === 'loading'}
		<div
			class="rounded-lg border border-slate-200 bg-white p-6 text-slate-500 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-400"
			role="status"
			aria-live="polite"
		>
			{$t('flow.operations.loading')}
		</div>
	{:else if panel.view === 'forbidden'}
		<div data-testid="flow-operations-forbidden" role="status" aria-live="polite">
			<EmptyState
				icon="office"
				title={$t('flow.operations.forbiddenTitle')}
				description={$t('flow.error.forbidden')}
			/>
		</div>
	{:else}
		{#if panel.health}
			<OpsHealthCard
				snapshot={panel.health}
				onpause={() => session.pauseHealth()}
				onresume={() => session.resumeHealth()}
				onrefresh={() => session.refreshHealth()}
			/>
		{/if}
		<OpsLagTable
			lag={panel.lag}
			lagErrorKey={panel.lagErrorKey}
			projection={panel.projection.data}
			projectionErrorKey={panel.projection.errorKey}
			projectionUnavailable={panel.projection.unavailable}
			pageNumber={panel.lagPage}
			hasPrevious={panel.lagHasPrevious}
			hasNext={panel.lagHasNext}
			busy={panel.lagBusy}
			onprevious={() => void session.loadProjection('previous')}
			onnext={() => void session.loadProjection('next')}
		/>
		<OpsIntegrityTable
			integrity={panel.integrity}
			errorKey={panel.integrityErrorKey}
			limit={panel.integrityLimit}
			busy={panel.integrityBusy}
			onlimit={changeLimit}
			onoperate={openOperation}
		/>
		<OpsReplayForm {replay} />
	{/if}
</div>

<OpsOperationDrawer
	bind:open={drawerOpen}
	{operation}
	onclose={closeDrawer}
	onexecuted={afterExecute}
/>
