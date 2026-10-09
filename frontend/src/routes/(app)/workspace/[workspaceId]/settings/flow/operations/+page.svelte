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
	import type { FlowAdminIntegrity, FlowAdminLag, FlowProjectionLag } from '$lib/api/flow';
	import {
		FlowOperationsService,
		type HealthMonitor,
		type HealthSnapshot,
		type MaintenanceKind,
		type MaintenanceOperation,
		type OpsIntegrityLimit,
		type OpsRead,
		type ProjectionLagPager
	} from '$lib/flow/operations-service';
	import { requireRouteParam } from '$lib/utils/route-params';

	const workspaceId = requireRouteParam($page.params.workspaceId, 'workspaceId');
	const service = new FlowOperationsService(workspaceId);
	const replay = service.replay();

	let view = $state<'loading' | 'ready' | 'forbidden'>('loading');
	let healthSnap = $state.raw<HealthSnapshot | null>(null);
	let monitor: HealthMonitor | null = null;

	let lag = $state.raw<FlowAdminLag | null>(null);
	let lagErrorKey = $state('');
	let pager: ProjectionLagPager | null = null;
	let projection = $state.raw<FlowProjectionLag | null>(null);
	let projectionErrorKey = $state('');
	let lagPage = $state(1);
	let lagHasPrevious = $state(false);
	let lagHasNext = $state(false);
	let lagBusy = $state(false);

	let integrity = $state.raw<FlowAdminIntegrity | null>(null);
	let integrityErrorKey = $state('');
	let integrityLimit = $state<OpsIntegrityLimit>(50);
	let integrityBusy = $state(false);

	let drawerOpen = $state(false);
	let operation = $state.raw<MaintenanceOperation | null>(null);

	onMount(async () => {
		if (!(await service.isWorkspaceAdmin())) {
			view = 'forbidden';
			return;
		}
		monitor = service.healthMonitor((snapshot) => {
			healthSnap = snapshot;
			if (snapshot.forbidden) view = 'forbidden';
		});
		const first = await monitor.refresh();
		if (first.forbidden) return;
		view = 'ready';
		monitor.start();
		pager = service.projectionLagPager();
		await Promise.all([loadLag(), loadProjection(() => pager?.load() ?? null), loadIntegrity()]);
	});

	onDestroy(() => monitor?.dispose());

	async function loadLag() {
		const read = await service.loadLag();
		if (read.status === 'ready') {
			lag = read.data;
			lagErrorKey = '';
		} else if (read.status === 'forbidden') {
			view = 'forbidden';
		} else {
			lagErrorKey = read.failure.messageKey;
		}
	}

	async function loadProjection(step: () => Promise<OpsRead<FlowProjectionLag> | null> | null) {
		if (!pager) return;
		lagBusy = true;
		const read = await step();
		lagBusy = false;
		if (!read) return;
		if (read.status === 'ready') {
			projection = read.data;
			projectionErrorKey = '';
		} else if (read.status === 'forbidden') {
			view = 'forbidden';
		} else {
			projectionErrorKey = read.failure.messageKey;
		}
		lagPage = pager.pageNumber;
		lagHasPrevious = pager.hasPrevious();
		lagHasNext = pager.hasNext();
	}

	async function loadIntegrity() {
		integrityBusy = true;
		const read = await service.loadIntegrity(integrityLimit);
		integrityBusy = false;
		if (read.status === 'ready') {
			integrity = read.data;
			integrityErrorKey = '';
		} else if (read.status === 'forbidden') {
			view = 'forbidden';
		} else {
			integrityErrorKey = read.failure.messageKey;
		}
	}

	function changeLimit(limit: OpsIntegrityLimit) {
		integrityLimit = limit;
		void loadIntegrity();
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
		void loadIntegrity();
		void loadLag();
		void loadProjection(() => pager?.load() ?? null);
		void monitor?.refresh();
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

	{#if view === 'loading'}
		<div
			class="rounded-lg border border-slate-200 bg-white p-6 text-slate-500 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-400"
			role="status"
			aria-live="polite"
		>
			{$t('flow.operations.loading')}
		</div>
	{:else if view === 'forbidden'}
		<div data-testid="flow-operations-forbidden" role="status" aria-live="polite">
			<EmptyState
				icon="office"
				title={$t('flow.operations.forbiddenTitle')}
				description={$t('flow.error.forbidden')}
			/>
		</div>
	{:else}
		{#if healthSnap}
			<OpsHealthCard
				snapshot={healthSnap}
				onpause={() => monitor?.pause()}
				onresume={() => monitor?.resume()}
				onrefresh={() => void monitor?.refresh()}
			/>
		{/if}
		<OpsLagTable
			{lag}
			{lagErrorKey}
			{projection}
			{projectionErrorKey}
			pageNumber={lagPage}
			hasPrevious={lagHasPrevious}
			hasNext={lagHasNext}
			busy={lagBusy}
			onprevious={() => void loadProjection(() => pager?.previous() ?? null)}
			onnext={() => void loadProjection(() => pager?.next() ?? null)}
		/>
		<OpsIntegrityTable
			{integrity}
			errorKey={integrityErrorKey}
			limit={integrityLimit}
			busy={integrityBusy}
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
