<script lang="ts">
	// The Flow object route. Opens exactly one `FlowObjectRepository` entry for this `objectId`
	// (ref-counted; closed on navigation away) and hands its `doc`/`projection`/`session.state`
	// down to the canvas, sync indicator, and context panel -- none of which ever touch the
	// network or the CRDT doc directly themselves.

	import { getContext, onDestroy, onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import { page } from '$app/stores';
	import { resolve } from '$app/paths';
	import { requireRouteParam } from '$lib/utils/route-params';
	import type { FlowObjectRepository, OpenFlowObjectResult } from '$lib/flow/object-repository';
	import { FLOW_REPOSITORY_CONTEXT } from '$lib/flow/context-keys';
	import { registryObjectType, resolveRenderer } from '$lib/flow/renderer-registry';
	import { toast } from '$lib/stores/toast';
	import FlowCanvas from '$lib/components/flow/FlowCanvas.svelte';
	import FlowContextPanel from '$lib/components/flow/FlowContextPanel.svelte';
	import FlowSyncIndicator from '$lib/components/flow/FlowSyncIndicator.svelte';
	import FlowRecoveryDraftPanel from '$lib/components/flow/FlowRecoveryDraftPanel.svelte';
	import CollectionRenderer from '$lib/components/flow/collection/CollectionRenderer.svelte';
	import type { FlowError, SyncState } from '$lib/flow/types';
	import { flowErrorDetailKey, flowErrorI18nKey, invalidUpdateDisposition } from '$lib/flow/errors';
	import type { RecoveryDraftEntry } from '$lib/flow/outbox';
	import { presenceColor, type PresenceCursor } from '$lib/flow/presence';
	import { authStore } from '$lib/stores/auth';
	import { get } from 'svelte/store';
	import type { FlowObjectView } from '$lib/api/flow';

	const repository = getContext<FlowObjectRepository>(FLOW_REPOSITORY_CONTEXT);

	let entry = $state<OpenFlowObjectResult | null>(null);
	// Set only from `openCurrent`'s catch, i.e. the object itself could not be opened at all --
	// this is the ONLY error that replaces the whole route body.
	let loadError = $state<FlowError | null>(null);
	// Set from `flowError` AFTER `entry` exists -- a rejected update, a limit violation, a policy
	// rejection, etc. mid-session. Shown as a dismissible inline banner; it must never unmount the
	// already-open editor/canvas, which would silently discard whatever the user was mid-edit on.
	// (Previously this bug existed: both cases wrote the same `loadError`, so a `policy_rejected`
	// on an open document flipped the `{#if loadError}` branch and replaced `FlowCanvas` with the
	// full-page error view.)
	let runtimeError = $state<FlowError | null>(null);
	// An object that exists but whose type has no registered renderer. Shown as the registry's
	// unsupported fallback instead of the open failure ("Renderer registry": unregistered
	// `${objectType}:${viewType}` fails to read_only/unsupported).
	let unsupportedObject = $state<FlowObjectView | null>(null);
	// A v0.6 Collection or Record: server-only documents (the server refuses a collab ticket and a
	// bootstrap for both, `flow/collab/ticket.rs::object_type_admits_collab_session`), rendered by
	// the registry's `collection:table|board` / `record:panel` renderers over typed reads, never
	// through `repository.open` (no engine doc, no WebSocket).
	let collectionObject = $state<FlowObjectView | null>(null);
	let syncState = $state<SyncState>('local');
	let pending = $state(0);
	let drafts = $state<ReadonlyArray<RecoveryDraftEntry>>([]);
	let readOnly = $state(false);
	let docGeneration = $state(0);
	let title = $state('');
	let titleSaveTimer: ReturnType<typeof setTimeout> | null = null;
	let reloading = $state(false);

	let openedObjectId: string | null = null;
	let unsubscribeState: (() => void) | null = null;
	let unsubscribeFlowError: (() => void) | null = null;
	let unsubscribeOthers: Array<() => void> = [];

	function teardownSubscriptions(): void {
		unsubscribeState?.();
		unsubscribeState = null;
		unsubscribeFlowError?.();
		unsubscribeFlowError = null;
		for (const unsubscribe of unsubscribeOthers) unsubscribe();
		unsubscribeOthers = [];
	}

	async function openCurrent(): Promise<void> {
		const workspaceId = requireRouteParam($page.params.workspaceId, 'workspaceId');
		const objectId = requireRouteParam($page.params.objectId, 'objectId');
		if (openedObjectId === objectId) return;

		teardownSubscriptions();
		if (openedObjectId) {
			await repository.close(openedObjectId);
			entry = null;
		}
		openedObjectId = objectId;
		loadError = null;
		runtimeError = null;
		unsupportedObject = null;
		collectionObject = null;

		const controller = new AbortController();
		// Dispatch on `object_type` first. The read is handed to `open` so a Page is not read twice.
		const read = await repository.get(objectId);
		if (openedObjectId !== objectId) return;
		const object = read.code === 0 && read.data ? read.data : null;
		if (object && (object.object_type === 'collection' || object.object_type === 'record')) {
			const kind = registryObjectType(object.object_type);
			const registered =
				kind === 'collection'
					? (resolveRenderer('collection', 'table') ?? resolveRenderer('collection', 'board'))
					: kind === 'record'
						? resolveRenderer('record', 'panel')
						: null;
			if (registered && (object.object_type === 'collection' || object.parent_id)) {
				collectionObject = object;
			} else {
				unsupportedObject = object;
			}
			return;
		}
		try {
			const opened = await repository.open({
				workspaceId,
				objectId,
				signal: controller.signal,
				object: object ?? undefined
			});
			if (openedObjectId !== objectId) return; // navigated away again before this resolved
			entry = opened;
			title = opened.handle.object.title;
			unsubscribeState = opened.session.state.subscribe((value) => (syncState = value));
			unsubscribeFlowError = opened.flowError.subscribe((value) => {
				if (value) runtimeError = value;
			});
			unsubscribeOthers = [
				opened.pending.subscribe((value) => (pending = value)),
				opened.recoveryDraft.subscribe((value) => (drafts = value)),
				opened.readOnly.subscribe((value) => (readOnly = value)),
				opened.docGeneration.subscribe((value) => (docGeneration = value))
			];
			// Presence identity: who this user is, as a summary (id, display name, colour).
			const user = get(authStore).user;
			if (user) {
				opened.session.setPresenceIdentity({
					userId: user.id,
					name: user.name || user.email,
					color: presenceColor(user.id)
				});
			}
		} catch (error) {
			const fallback = await unsupportedFallback(objectId);
			if (openedObjectId !== objectId) return;
			if (fallback) unsupportedObject = fallback;
			else loadError = error as FlowError;
		}
	}

	/** The object, when it is readable but its type has no registered canvas renderer. */
	async function unsupportedFallback(objectId: string): Promise<FlowObjectView | null> {
		const result = await repository.get(objectId);
		if (result.code !== 0 || !result.data) return null;
		const kind = registryObjectType(result.data.object_type);
		return kind && resolveRenderer(kind, 'canvas') ? null : result.data;
	}

	onMount(() => {
		void openCurrent();
	});

	$effect(() => {
		void $page.params.objectId;
		void openCurrent();
	});

	onDestroy(() => {
		teardownSubscriptions();
		if (openedObjectId) void repository.close(openedObjectId);
	});

	function onTitleInput(): void {
		if (!entry) return;
		if (titleSaveTimer) clearTimeout(titleSaveTimer);
		titleSaveTimer = setTimeout(() => {
			if (entry) repository.setTitle(entry.handle.objectId, title);
		}, 400);
	}

	function dismissRuntimeError(): void {
		runtimeError = null;
	}

	function reportPresence(cursor: PresenceCursor | null): void {
		entry?.session.setPresence(cursor);
	}

	// The defensive `invalid_update` reason: stop ordinary writes and point at the workspace
	// operations page, where an administrator runs the integrity tools.
	const integrityRepair = $derived(
		runtimeError !== null && invalidUpdateDisposition(runtimeError)?.action === 'integrity_repair'
	);
	const permanentFailure = $derived(
		syncState === 'error' && runtimeError?.code === 'server_rejected'
	);

	async function copyRecoveryDraft(): Promise<void> {
		if (!entry) return;
		try {
			const text = repository.recoveryDraftText(entry.handle.objectId, (key, values) =>
				$t(key, values ? { values } : undefined)
			);
			await navigator.clipboard.writeText(text);
			toast.success($t('flow.recovery.copied'));
		} catch {
			toast.error($t('flow.recovery.copyFailed'));
		}
	}

	async function discardRecoveryDraft(): Promise<void> {
		if (!entry) return;
		await repository.clearRecoveryDraft(entry.handle.objectId);
	}

	// `policy_rejected`/`invalid_update`/`stale_frontier`: the rejected intent cannot be replayed
	// (no outbox this round -- see `object-session.ts`'s header comment), so offer a local
	// snapshot download instead of silently losing it (`ui-surface-v1.md` step 5: "不能重放的
	// intent 进入 recovery draft，并提供下载/复制动作").
	const canDownloadDraft = $derived(
		runtimeError !== null &&
			(runtimeError.code === 'policy_rejected' ||
				runtimeError.code === 'invalid_update' ||
				runtimeError.code === 'stale_frontier')
	);

	async function downloadRecoveryDraft(): Promise<void> {
		if (!entry) return;
		try {
			const blob = await repository.exportRecoveryDraft(entry.handle.objectId);
			const url = URL.createObjectURL(blob);
			const link = document.createElement('a');
			link.href = url;
			link.download = `${entry.handle.objectId}-recovery-draft.json`;
			document.body.appendChild(link);
			link.click();
			link.remove();
			URL.revokeObjectURL(url);
			toast.success($t('flow.recovery.downloaded'));
		} catch {
			toast.error($t('flow.recovery.downloadFailed'));
		}
	}

	// `stale_frontier`: this connection's local doc is behind the head the server just told it
	// about but did not push a resync for (only `resync_required` auto-resyncs). `bootstrap` +
	// `replaceWithAccepted` fetch a fresh accepted snapshot+tail over REST -- independent of the
	// live WebSocket session -- without discarding the rejected edit the user can still download.
	async function reloadFromServer(): Promise<void> {
		if (!entry) return;
		reloading = true;
		try {
			const bootstrap = await repository.bootstrap(entry.handle.objectId);
			await repository.replaceWithAccepted(bootstrap);
			runtimeError = null;
			toast.success($t('flow.recovery.reloaded'));
		} catch (error) {
			runtimeError = error as FlowError;
		} finally {
			reloading = false;
		}
	}

	/** Safe numeric/string details (limit, observed, ...) for the error copy's placeholders. */
	function runtimeErrorValues(error: FlowError): Record<string, string | number> {
		const details = error.details;
		if (typeof details !== 'object' || details === null) return {};
		const out: Record<string, string | number> = {};
		for (const [key, value] of Object.entries(details as Record<string, unknown>)) {
			if (typeof value === 'string' || typeof value === 'number') out[key] = value;
		}
		return out;
	}

	// `contracts/ui-surface-v1.md` "Renderer registry": an unregistered `${objectType}:${viewType}`
	// must fail to read-only/unsupported, never reach the CRDT-backed canvas.
	const canvasRenderer = $derived.by(() => {
		if (!entry) return null;
		const kind = registryObjectType(entry.handle.objectType);
		return kind ? resolveRenderer(kind, 'canvas') : null;
	});
</script>

{#if loadError}
	<div class="flex flex-1 flex-col items-center justify-center gap-2 p-8 text-center">
		<h1 class="text-lg font-semibold text-slate-900 dark:text-slate-100">
			{loadError.code === 'not_found'
				? $t('flow.route.notFoundTitle')
				: $t('flow.route.disabledTitle')}
		</h1>
		<p class="max-w-md text-sm text-slate-500 dark:text-slate-400">
			{$t(flowErrorI18nKey(loadError))}
		</p>
	</div>
{:else if collectionObject}
	{@const routeWorkspaceId = requireRouteParam($page.params.workspaceId, 'workspaceId')}
	<div
		class="flex items-center justify-between border-b border-slate-200 px-6 py-3 dark:border-slate-800"
	>
		<h1 class="flex-1 truncate text-lg font-semibold text-slate-900 dark:text-slate-100">
			{collectionObject.object_type === 'record'
				? collectionObject.title || $t('flow.collection.record.untitled')
				: collectionObject.title || $t('flow.nav.untitled')}
		</h1>
	</div>
	<div class="flex flex-1 flex-col overflow-y-auto lg:flex-row lg:overflow-hidden">
		{#key collectionObject.id}
			{#if collectionObject.object_type === 'record' && collectionObject.parent_id}
				<div
					class="flex min-w-0 flex-col lg:flex-1 lg:overflow-auto"
					data-testid="flow-record-route"
				>
					<CollectionRenderer
						workspaceId={routeWorkspaceId}
						collectionId={collectionObject.parent_id}
						recordId={collectionObject.id}
					/>
				</div>
			{:else}
				<!-- Below lg the column scrolls as a whole (no inner scroller that would clip the cards). -->
				<div class="flex min-w-0 flex-col lg:flex-1 lg:overflow-auto" data-object-type="collection">
					<CollectionRenderer workspaceId={routeWorkspaceId} collectionId={collectionObject.id} />
				</div>
			{/if}
		{/key}
		<FlowContextPanel object={collectionObject} />
	</div>
{:else if unsupportedObject}
	<div
		class="flex items-center justify-between border-b border-slate-200 px-6 py-3 dark:border-slate-800"
	>
		<h1 class="flex-1 truncate text-lg font-semibold text-slate-900 dark:text-slate-100">
			{unsupportedObject.title || $t('flow.nav.untitled')}
		</h1>
	</div>
	<div class="flex flex-1 flex-col overflow-y-auto lg:flex-row lg:overflow-hidden">
		<div
			class="flex flex-1 items-center justify-center p-8 text-center"
			data-testid="flow-object-unsupported"
			data-object-type={unsupportedObject.object_type}
		>
			<p class="text-sm text-slate-500 dark:text-slate-400">{$t('flow.canvas.unsupported')}</p>
		</div>
		<FlowContextPanel object={unsupportedObject} />
	</div>
{:else if !entry}
	<div class="flex flex-1 items-center justify-center p-8">
		<p class="text-sm text-slate-500 dark:text-slate-400">{$t('common.loading')}</p>
	</div>
{:else}
	<div
		class="flex items-center justify-between border-b border-slate-200 px-6 py-3 dark:border-slate-800"
	>
		<input
			bind:value={title}
			oninput={onTitleInput}
			readonly={readOnly}
			placeholder={$t('flow.titlePlaceholder')}
			aria-label={$t('flow.panel.title')}
			class="flex-1 truncate border-none bg-transparent text-lg font-semibold text-slate-900 outline-none dark:text-slate-100"
		/>
		<FlowSyncIndicator state={syncState} {pending} />
	</div>

	{#if runtimeError}
		<div
			class="flex flex-wrap items-center gap-3 border-b border-amber-200 bg-amber-50 px-6 py-2 text-sm text-amber-900 dark:border-amber-900 dark:bg-amber-950 dark:text-amber-100"
			role="alert"
		>
			<!-- Key selection lives in `$lib/flow/errors`, not in this template: the inline
				 `reason === 'drain' ? drain : contention` ternary this replaced silently rendered a
				 `server_draining` with a MISSING required discriminator as an ordinary contention
				 retry, which `error-mapping-v1.md` forbids ("缺失/未知 reason 违反协议"). -->
			<span class="flex-1" data-testid="flow-runtime-error" data-error-code={runtimeError.code}
				>{$t(flowErrorDetailKey(runtimeError), {
					values: runtimeErrorValues(runtimeError)
				})}</span
			>
			{#if integrityRepair}
				<a
					class="font-medium underline underline-offset-2 hover:no-underline"
					href={resolve('/(app)/workspace/[workspaceId]/settings/flow/operations', {
						workspaceId: entry.handle.workspaceId
					})}
					data-testid="flow-integrity-repair-link"
				>
					{$t('flow.error.integrityRepairLink')}
				</a>
			{/if}
			{#if canDownloadDraft}
				<button
					type="button"
					class="font-medium underline underline-offset-2 hover:no-underline"
					onclick={downloadRecoveryDraft}
				>
					{$t('flow.recovery.download')}
				</button>
				<button
					type="button"
					class="font-medium underline underline-offset-2 hover:no-underline disabled:opacity-50"
					disabled={reloading}
					onclick={reloadFromServer}
				>
					{$t('flow.recovery.reload')}
				</button>
			{/if}
			<button
				type="button"
				class="font-medium underline underline-offset-2 hover:no-underline"
				onclick={dismissRuntimeError}
			>
				{$t('flow.recovery.dismiss')}
			</button>
		</div>
	{/if}

	{#if drafts.length > 0}
		<FlowRecoveryDraftPanel
			entries={drafts}
			permanent={permanentFailure}
			ondownload={() => void downloadRecoveryDraft()}
			oncopy={() => void copyRecoveryDraft()}
			ondiscard={() => void discardRecoveryDraft()}
		/>
	{/if}

	<!-- Below lg the context panel stacks under the canvas and the column scrolls (390px phones). -->
	<div class="flex flex-1 flex-col overflow-y-auto lg:flex-row lg:overflow-hidden">
		{#if canvasRenderer}
			<!-- A rebuild from the accepted snapshot replaces the engine doc; remount on it. -->
			{#key docGeneration}
				<FlowCanvas
					doc={entry.doc}
					workspaceId={entry.handle.workspaceId}
					objectId={entry.handle.objectId}
					{readOnly}
					presence={entry.presence}
					onPresenceCursor={reportPresence}
				/>
			{/key}
		{:else}
			<div class="flex flex-1 items-center justify-center p-8 text-center">
				<p class="text-sm text-slate-500 dark:text-slate-400">{$t('flow.canvas.unsupported')}</p>
			</div>
		{/if}
		<FlowContextPanel object={entry.handle.object} />
	</div>
{/if}
