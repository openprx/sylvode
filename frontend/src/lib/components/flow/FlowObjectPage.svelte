<script lang="ts">
	import { randomUuid } from '#lib/utils/uuid.js';
	// The Flow object route. Opens exactly one `FlowObjectRepository` entry for this `objectId`
	// (ref-counted; closed on navigation away) and hands its `doc`/`projection`/`session.state`
	// down to the canvas, sync indicator, and context panel -- none of which ever touch the
	// network or the CRDT doc directly themselves.

	import { getNamedMap } from '#lib/flow/object-repository.js';
	import { FLOW_TASK_SESSIONS, type FlowTaskSessions } from '#lib/workbench/flow-sessions.js';
	import { TaskWorkbench, WORKBENCH_CONTEXT } from '#lib/workbench/tabs.js';
	import { conversionAvailability } from '#lib/flow/convert-availability.js';
	import {
		FLOW_COLLECTION_UI_ENABLED,
		FLOW_PROCESS_READER_COMPATIBILITY
	} from '#lib/flow/product-mode.js';
	import { getContext, onDestroy, onMount, untrack } from 'svelte';
	import { t } from 'svelte-i18n';
	import { page } from '$app/state';
	import { resolve } from '$app/paths';
	import { requireRouteParam } from '#lib/utils/route-params.js';
	import type { FlowObjectRepository, OpenFlowObjectResult } from '#lib/flow/object-repository.js';
	import {
		FLOW_REPOSITORY_CONTEXT,
		FLOW_SCOPE_CONTEXT,
		FLOW_NAV_VIEW_CONTEXT
	} from '#lib/flow/context-keys.js';
	import { registryObjectType, resolveRenderer } from '#lib/flow/renderer-registry.js';
	import { toast } from '#lib/stores/toast.js';
	import FlowLegacyEditorPreview from '#lib/components/flow/FlowLegacyEditorPreview.svelte';
	import {
		canEnableLegacyEditor,
		initializeEditorRequest,
		editorMigrationState
	} from '#lib/flow/legacy-editor.js';
	import { flowApi } from '#lib/api/flow.js';
	import FlowCanvas from '#lib/components/flow/FlowCanvas.svelte';
	import FlowContextPanel from '#lib/components/flow/FlowContextPanel.svelte';
	import FlowResizablePane from './FlowResizablePane.svelte';
	import FlowPageToolbar from './FlowPageToolbar.svelte';
	import FlowBlockShareSource from './FlowBlockShareSource.svelte';
	import FlowBlockTransferDialog from './FlowBlockTransferDialog.svelte';
	import { blockTransfersSupported } from '#lib/flow/block-transfer/capability.js';
	import type { TransferPageRuntime } from '#lib/flow/block-transfer/runtime.js';
	import { sharedBlocksSupported } from '#lib/flow/shared-blocks/capability.js';
	import FlowPageHeader from '#lib/components/flow/FlowPageHeader.svelte';
	import type { FlowAppearance } from '#lib/api/flow.js';
	import FlowRecoveryDraftPanel from '#lib/components/flow/FlowRecoveryDraftPanel.svelte';
	import CollectionRenderer from '#lib/components/flow/collection/CollectionRenderer.svelte';
	import type { FlowError, SyncState } from '#lib/flow/types.js';
	import {
		flowErrorFromEnvelope,
		flowErrorDetailKey,
		flowErrorI18nKey,
		invalidUpdateDisposition
	} from '#lib/flow/errors.js';
	import type { RecoveryDraftEntry } from '#lib/flow/outbox.js';
	import { presenceColor, type PresenceCursor } from '#lib/flow/presence.js';
	import { authStore } from '#lib/stores/auth.js';
	import { get } from 'svelte/store';
	import type { FlowObjectView } from '#lib/api/flow.js';

	const repository = getContext<FlowObjectRepository>(FLOW_REPOSITORY_CONTEXT);
	const flowScopeId = getContext<() => string>(FLOW_SCOPE_CONTEXT);

	const navView = getContext<{ refresh: () => Promise<void> } | undefined>(FLOW_NAV_VIEW_CONTEXT);
	const taskWorkbench = getContext<TaskWorkbench | undefined>(WORKBENCH_CONTEXT);
	const flowTasks = getContext<(() => FlowTaskSessions | null) | undefined>(FLOW_TASK_SESSIONS)?.();
	let unregisterTask: () => void = () => {};
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
	let hiddenTypedObject = $state(false);
	let legacyEditorObject = $state<FlowObjectView | null>(null);
	let enablingEditor = $state(false);
	let sharedBlocksReady = $state(false);
	let sourceShareRevokeReady = $state(false);
	let transfersReady = $state(false);
	let portablePackagesReady = $state(false);
	let portableGraphReady = $state(false);

	let transferOpen = $state(false);
	let transferRoot = $state('');
	let transferKind = $state<'move' | 'copy'>('move');
	let transferRuntime = $state<TransferPageRuntime | null>(null);
	let visibleTransfers = $state<TransferPageRuntime[]>([]);
	$effect(() => {
		const actor = $authStore.user?.id,
			objectId = entry?.handle.objectId;
		let stop: (() => void) | null = null,
			active = true;
		if (actor && objectId)
			void import('#lib/flow/block-transfer/session.js').then(({ transferSessions }) => {
				if (!active) return;
				const sessions = transferSessions(repository, actor, () => get(authStore).user?.id ?? null);
				stop = sessions.subscribe(() => {
					if (active) visibleTransfers = sessions.forPage(objectId);
				});
			});
		else visibleTransfers = [];
		return () => {
			active = false;
			stop?.();
		};
	});
	async function openTransfer(blockId: string, kind: 'move' | 'copy') {
		const actor = get(authStore).user?.id;
		if (!entry || !actor || !transfersReady) return;
		const sourceId = entry.handle.objectId;
		const { transferSessions } = await import('#lib/flow/block-transfer/session.js');
		if (disposed || get(authStore).user?.id !== actor || entry?.handle.objectId !== sourceId)
			return;
		transferRuntime = transferSessions(
			repository,
			actor,
			() => get(authStore).user?.id ?? null
		).forSource(sourceId);
		transferRoot = transferRuntime.operation.beginNewIntent() ? blockId : '';
		transferKind = kind;
		transferOpen = true;
	}
	let sourceShareOpen = $state(false);
	let sourceShareRoot = $state<string | null>(null);
	onMount(() => {
		void flowApi.getCapabilities().then((result) => {
			if (!disposed) {
				sharedBlocksReady = sharedBlocksSupported(result);
				sourceShareRevokeReady =
					sharedBlocksReady && result.data?.block_share_source_revoke_supported === true;
				transfersReady = blockTransfersSupported(result);
				portablePackagesReady =
					result.code === 0 &&
					result.data?.api_version === 2 &&
					result.data.portable_page_packages_supported === true &&
					result.data.minimum_reader_version >= 5 &&
					result.data.minimum_reader_version <= FLOW_PROCESS_READER_COMPATIBILITY;
				portableGraphReady =
					result.code === 0 &&
					result.data?.api_version === 2 &&
					result.data.portable_graph_packages_supported === true &&
					result.data.minimum_reader_version >= 5 &&
					result.data.minimum_reader_version <= FLOW_PROCESS_READER_COMPATIBILITY;
			}
		});
	});

	let richEditorReady = $state(false);
	let upgradingRichEditor = $state(false);
	let richUpgradePromise: Promise<boolean> | null = null;
	let legacyEditorError = $state<FlowError | null>(null);
	let editorIntent: { objectId: string; frontier: string; key: string } | null = null;
	let syncState = $state<SyncState>('local');
	let pending = $state(0);
	let recoveringEdits = $state(false);
	let bodyPending = $state(0);
	const persistedPageReady = $derived(
		pending === 0 &&
			bodyPending === 0 &&
			!recoveringEdits &&
			(syncState === 'local' || syncState === 'saved' || syncState === 'read_only')
	);
	let objectMetadata = $state<FlowObjectView | null>(null);
	let drafts = $state<ReadonlyArray<RecoveryDraftEntry>>([]);
	let readOnly = $state(false);
	let permittedActions = $state<readonly string[] | null>(null);
	let docGeneration = $state(0);
	let title = $state('');
	let appearance = $state<FlowAppearance | null>(null);
	let commentBlockId = $state<string | null>(null);
	let commentNonce = $state(0);
	let commentStructureRevision = $state(0);
	let activeTool = $state<'info' | 'comments' | 'relations' | 'history' | null>(null);
	function closeTool() {
		const previous = activeTool;
		activeTool = null;
		document
			.querySelector<
				HTMLButtonElement | HTMLElement
			>(previous === 'comments' ? '[data-testid=flow-comments-toggle]' : '[data-testid=flow-more]')
			?.focus();
	}
	let toolScroll = $state(0);
	let lastTool = $state<'info' | 'comments' | 'relations' | 'history' | null>(null);
	$effect(() => {
		if (activeTool) lastTool = activeTool;
	});
	let reloading = $state(false);

	let disposed = false;
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
		if (disposed) return;
		let scopeId = flowScopeId();
		const objectId = requireRouteParam(page.params.objectId, 'objectId');
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
		hiddenTypedObject = false;
		legacyEditorObject = null;
		legacyEditorError = null;

		const controller = new AbortController();
		// Dispatch on `object_type` first. The read is handed to `open` so a Page is not read twice.
		let read: Awaited<ReturnType<FlowObjectRepository['get']>>;
		try {
			read = await repository.get(objectId);
		} catch {
			if (openedObjectId === objectId) loadError = { code: 'network_error', recoverable: true };
			return;
		}
		if (openedObjectId !== objectId) return;
		const object = read.code === 0 && read.data ? read.data : null;
		if (object) scopeId = object.scope_id;
		if (
			object?.object_type === 'page' &&
			(object.editor_state === 'migration_required' || object.editor_state === 'unsupported')
		) {
			legacyEditorObject = object;
			return;
		}
		if (object && (object.object_type === 'collection' || object.object_type === 'record')) {
			if (!FLOW_COLLECTION_UI_ENABLED) {
				hiddenTypedObject = true;
				taskWorkbench?.close(`flow:${objectId}`, true);
				return;
			}
			const kind = registryObjectType(object.object_type);

			const registered =
				kind === 'collection'
					? (resolveRenderer('collection', 'table') ?? resolveRenderer('collection', 'board'))
					: kind === 'record'
						? resolveRenderer('record', 'panel')
						: null;

			if (
				registered &&
				(object.object_type === 'collection' || object.record_context_supported === true)
			) {
				collectionObject = object;
			} else {
				unsupportedObject = object;
			}
			return;
		}
		try {
			const opened = await repository.open({
				scopeId,
				objectId,
				signal: controller.signal,
				object: object ?? undefined
			});
			if (disposed || openedObjectId !== objectId) {
				await repository.close(objectId);
				return;
			}
			// Older servers may omit editor_state. Never initialize a blank editor over
			// an accepted legacy tree-only snapshot merely by mounting the canvas.
			if (
				opened.handle.object.object_type === 'page' &&
				!opened.doc.getMap('doc').get('children')
			) {
				await repository.close(objectId);
				legacyEditorObject = {
					...opened.handle.object,
					editor_state: 'migration_required',
					editor_initialization_supported: false
				};
				return;
			}
			recoveringEdits = get(opened.offline.recovering);
			entry = opened;
			objectMetadata = opened.handle.object;
			richEditorReady = opened.handle.object.editor_structure_state === 'ready';
			const taskKey = `flow:${objectId}`;
			const savedTool = taskWorkbench?.snapshot<{
				activeTool: typeof activeTool;
				toolScroll: number;
			}>(taskKey);
			activeTool = savedTool?.activeTool ?? null;
			toolScroll = savedTool?.toolScroll ?? 0;
			await flowTasks?.retain(
				taskKey,
				'page',
				opened.handle.object.scope_id,
				objectId,
				opened.handle.object
			);
			taskWorkbench?.setTitle(taskKey, opened.handle.object.title || get(t)('flow.nav.untitled'));
			unregisterTask =
				taskWorkbench?.register(taskKey, {
					capture: () => ({
						activeTool,
						toolScroll:
							document.querySelector('[data-testid="flow-tool-panel"] .pane-content')?.scrollTop ??
							0
					}),
					dirty: () =>
						pending > 0 ||
						bodyPending > 0 ||
						!!taskWorkbench.snapshot<{ body: string; editBody: string }>(
							`${taskKey}::comment-draft`
						)?.body ||
						!!taskWorkbench.snapshot<{ editingId: string | null }>(`${taskKey}::comment-draft`)
							?.editingId,
					refresh: async () => {
						await navView?.refresh();
						const current = await repository.get(objectId);
						if (current.code !== 0)
							runtimeError = flowErrorFromEnvelope(current) ?? {
								code: 'network_error',
								recoverable: true
							};
					}
				}) ?? (() => {});
			title = String(getNamedMap(opened.doc, 'meta').get('title') ?? opened.handle.object.title);
			appearance = opened.handle.object.appearance ?? null;
			commentBlockId = null;
			commentNonce = 0;
			unsubscribeState = opened.session.state.subscribe((value) => (syncState = value));
			unsubscribeFlowError = opened.flowError.subscribe((value) => {
				if (value) runtimeError = value;
				else if (
					(runtimeError?.details as { reason?: string } | undefined)?.reason === 'sending_paused'
				)
					runtimeError = null;
			});
			unsubscribeOthers = [
				opened.pending.subscribe((value) => (pending = value)),
				opened.offline.recovering.subscribe((value) => (recoveringEdits = value)),
				opened.metadata?.subscribe((value) => (objectMetadata = value)) ?? (() => {}),
				opened.bodyPending?.subscribe((value) => (bodyPending = value)) ?? (() => {}),
				opened.recoveryDraft.subscribe((value) => (drafts = value)),
				opened.readOnly.subscribe((value) => (readOnly = value)),
				opened.permittedActions.subscribe((value) => (permittedActions = value)),
				opened.docGeneration.subscribe((value) => {
					docGeneration = value;
					commentStructureRevision++;
				})
			];
			let stopCommentStructure = () => {};
			unsubscribeOthers.push(
				opened.docGeneration.subscribe(() => {
					stopCommentStructure();
					const root = opened.doc.getMap('doc'),
						physical = root.get('children');
					const physicalId = (physical as unknown as { id?: string } | null)?.id ?? null;
					stopCommentStructure = opened.doc.subscribe((batch) => {
						if (
							batch.events.some(
								(event) =>
									event.diff.type === 'tree' ||
									event.target === root.id ||
									event.target === physicalId
							)
						)
							commentStructureRevision++;
					});
				}),
				() => stopCommentStructure()
			);

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
			const typed = error as FlowError;
			const migration = editorMigrationState(typed);
			if (object?.object_type === 'page' && migration) {
				legacyEditorObject = { ...object, editor_state: migration };
				return;
			}
			const fallback = await unsupportedFallback(objectId);
			if (openedObjectId !== objectId) return;
			if (fallback) unsupportedObject = fallback;
			else loadError = error as FlowError;
		}
	}

	$effect(() => {
		const previewId = legacyEditorObject?.id;
		if (!previewId) return;
		return untrack(() => {
			const key = `flow:${previewId}`;
			const saved = taskWorkbench?.snapshot<{ activeTool: typeof activeTool; toolScroll: number }>(
				key
			);
			activeTool = saved?.activeTool ?? null;
			toolScroll = saved?.toolScroll ?? 0;
			taskWorkbench?.setTitle(key, legacyEditorObject?.title ?? get(t)('flow.nav.untitled'));
			return taskWorkbench?.register(key, {
				capture: () => ({
					activeTool,
					toolScroll:
						document.querySelector('[data-testid="flow-tool-panel"] .pane-content')?.scrollTop ?? 0
				}),
				dirty: () => {
					const draft = taskWorkbench.snapshot<{ body: string; editingId: string | null }>(
						`${key}::comment-draft`
					);
					return !!draft?.body || !!draft?.editingId;
				},
				refresh: refreshLegacyEditor
			});
		});
	});

	async function refreshLegacyEditor(): Promise<void> {
		if (enablingEditor) return;
		openedObjectId = null;
		await openCurrent();
	}

	function ensureRichEditor(): Promise<boolean> {
		if (richEditorReady) return Promise.resolve(true);
		if (richUpgradePromise) return richUpgradePromise;
		richUpgradePromise = enableRichEditor().finally(() => (richUpgradePromise = null));
		return richUpgradePromise;
	}
	async function enableRichEditor(): Promise<boolean> {
		if (!entry || readOnly) return false;
		const id = entry.handle.objectId;
		upgradingRichEditor = true;
		runtimeError = null;
		try {
			if (pending > 0)
				await new Promise<void>((resolve, reject) => {
					const timer = setTimeout(() => {
						stop();
						reject({ code: 'resync_required', recoverable: true });
					}, 10000);
					const stop = entry!.pending.subscribe((count) => {
						if (count === 0) {
							clearTimeout(timer);
							queueMicrotask(() => stop());
							resolve();
						}
					});
				});
			const fresh = await repository.get(id);
			if (fresh.code !== 0 || !fresh.data)
				throw flowErrorFromEnvelope(fresh) ?? { code: 'server_rejected', recoverable: false };
			const result = await flowApi.executeCommand(id, {
				command: { type: 'initialize_editor_doc', payload: { model: 'rich_page_v4' } },
				expected_frontier: fresh.data.frontier,
				idempotency_key: randomUuid()
			});
			if (result.code !== 0)
				throw flowErrorFromEnvelope(result) ?? { code: 'server_rejected', recoverable: false };
			const bootstrap = await repository.bootstrap(id);
			await repository.replaceWithAccepted(bootstrap);
			if (entry?.handle.objectId === id) {
				richEditorReady = true;
				return true;
			}
			return false;
		} catch (error) {
			runtimeError = error as FlowError;
			return false;
		} finally {
			upgradingRichEditor = false;
		}
	}

	async function enableLegacyEditor(): Promise<void> {
		const object = legacyEditorObject;
		if (!object || !canEnableLegacyEditor(object) || enablingEditor) return;
		if (
			!editorIntent ||
			editorIntent.objectId !== object.id ||
			editorIntent.frontier !== object.frontier
		)
			editorIntent = { objectId: object.id, frontier: object.frontier, key: randomUuid() };
		enablingEditor = true;
		legacyEditorError = null;
		try {
			const result = await flowApi.executeCommand(
				object.id,
				initializeEditorRequest(object.frontier, editorIntent.key)
			);
			if (disposed || openedObjectId !== object.id) return;
			if (result.code !== 0) {
				legacyEditorError = flowErrorFromEnvelope(result) ?? {
					code: 'server_rejected',
					recoverable: false
				};
				const refreshed = await repository.get(object.id);
				if (openedObjectId === object.id && refreshed.code === 0 && refreshed.data)
					legacyEditorObject = refreshed.data;
				return;
			}
			editorIntent = null;
			openedObjectId = null;
			await openCurrent();
		} catch {
			if (openedObjectId === object.id)
				legacyEditorError = { code: 'network_error', recoverable: true };
		} finally {
			enablingEditor = false;
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
		void page.params.objectId;
		void openCurrent();
	});

	onDestroy(() => {
		unregisterTask();
		disposed = true;
		teardownSubscriptions();
		if (openedObjectId) void repository.close(openedObjectId);
		openedObjectId = null;
	});

	function onTitleInput(): void {
		if (entry) repository.setTitle(entry.handle.objectId, title);
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
	const editorWritePaused = $derived(
		runtimeError?.origin === 'client' &&
			runtimeError.code === 'invalid_update' &&
			(runtimeError.details as { reason?: string } | undefined)?.reason ===
				'editor_transaction_aborted'
	);

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

<svelte:window
	onkeydown={(event) => {
		if (event.key === 'Escape' && !event.defaultPrevented && activeTool) {
			event.preventDefault();
			closeTool();
		}
	}}
/>

{#each visibleTransfers as transfer (transfer.sourceId)}
	{@const applied = transfer.operation.current().phase === 'applied'}
	<div
		role="status"
		data-testid="flow-transfer-resolution"
		class="mx-auto my-2 flex max-w-3xl items-center justify-between gap-2 rounded border p-3 text-sm"
		class:border-warning={!applied}
	>
		<p>{applied ? $t('flow.transfer.applied') : $t('flow.transfer.pending')}</p>
		<button
			type="button"
			class="underline"
			onclick={() => {
				transferRuntime = transfer;
				transferRoot = '';
				transferKind = transfer.operation.current().review?.kind ?? 'move';
				transferOpen = true;
			}}>{applied ? $t('flow.transfer.viewReceipt') : $t('flow.transfer.checkReceipt')}</button
		>
	</div>
{/each}
{#if transferOpen && transferRuntime}
	<FlowBlockTransferDialog
		bind:open={transferOpen}
		runtime={transferRuntime}
		rootCid={transferRoot}
		pickerScopeId={entry?.handle.object.scope_id ?? flowScopeId()}
		initialKind={transferKind}
	/>
{/if}

{#if sourceShareOpen && (entry || legacyEditorObject)}<FlowBlockShareSource
		bind:open={sourceShareOpen}
		sourceId={entry?.handle.objectId ?? legacyEditorObject!.id}
		rootCid={sourceShareRoot}
		canRevoke={sourceShareRevokeReady}
		canManage={(
			permittedActions ??
			(objectMetadata ?? entry?.handle.object ?? legacyEditorObject)?.permitted_actions ??
			[]
		).includes('manage')}
	/>{/if}

{#snippet pageToolPane(toolObject: FlowObjectView, toolActions: readonly string[] | null)}
	{#if lastTool}
		<div hidden={!activeTool} class="flow-tool-pane" data-testid="flow-tool-panel">
			<FlowResizablePane scrollTop={toolScroll} side="right" label={$t('flow.layout.resizeTools')}>
				<FlowContextPanel
					object={toolObject}
					permittedActions={toolActions}
					{portablePackagesReady}
					{portableGraphReady}
					onGraphPagesCreated={(objects) => {
						for (const { id, parentId } of objects) repository.notifyObjectCreated(id, parentId);
						void navView?.refresh();
					}}
					onPortablePageCreated={(id) => {
						repository.notifyObjectCreated(id, toolObject.id);
						void navView?.refresh();
					}}
					portableExportReady={persistedPageReady}
					historyRestoreReady={persistedPageReady}
					onHistoryRestored={(id) =>
						entry?.handle.objectId === id ? repository.resyncAccepted(id) : Promise.resolve(true)}
					{commentBlockId}
					commentRevision={`${commentStructureRevision}:${docGeneration}:${toolActions?.join(',') ?? ''}`}
					{commentNonce}
					selectedTool={activeTool ?? lastTool}
					onclose={closeTool}
				/>
			</FlowResizablePane>
		</div>
	{/if}
{/snippet}
{#if loadError}
	<div class="flex flex-1 flex-col items-center justify-center gap-2 p-8 text-center" role="alert">
		<h1 class="text-lg font-semibold text-fg">
			{loadError.code === 'not_found'
				? $t('flow.route.notFoundTitle')
				: $t('flow.route.disabledTitle')}
		</h1>
		<p class="max-w-md text-sm text-fg-muted">
			{$t(flowErrorI18nKey(loadError))}
		</p>
	</div>
{:else if legacyEditorObject}
	<FlowPageToolbar
		object={legacyEditorObject}
		bind:activeTool
		onManageSharedBlocks={sharedBlocksReady &&
		legacyEditorObject.permitted_actions?.includes('manage')
			? () => {
					sourceShareRoot = null;
					sourceShareOpen = true;
				}
			: undefined}
	/>
	<div class="relative flex min-h-0 flex-1 overflow-hidden">
		<FlowLegacyEditorPreview
			object={legacyEditorObject}
			busy={enablingEditor}
			error={legacyEditorError}
			onEnable={enableLegacyEditor}
			onRefresh={refreshLegacyEditor}
		/>
		{@render pageToolPane(legacyEditorObject, null)}
	</div>
{:else if hiddenTypedObject}
	<section
		class="flex flex-1 flex-col items-center justify-center gap-3 p-8 text-center"
		data-testid="flow-typed-unavailable"
	>
		<h1 class="text-lg font-semibold">{$t('flow.pageOnly.title')}</h1>
		<p class="text-sm text-fg-muted">{$t('flow.pageOnly.body')}</p>
		<a class="min-h-11 py-2 text-fg underline" href={resolve('/(app)/flow')}
			>{$t('flow.pageOnly.back')}</a
		>
	</section>
{:else if collectionObject}
	{@const routeScopeId = collectionObject.scope_id}
	<div class="flex items-center justify-between border-b border-line px-6 py-3">
		<h1 class="flex-1 truncate text-lg font-semibold text-fg">
			{collectionObject.object_type === 'record'
				? collectionObject.title || $t('flow.collection.record.untitled')
				: collectionObject.title || $t('flow.nav.untitled')}
		</h1>
	</div>
	<div class="flex flex-1 flex-col overflow-y-auto lg:flex-row lg:overflow-hidden">
		{#key collectionObject.id}
			{#if collectionObject.object_type === 'record'}
				<div
					class="flex min-w-0 flex-col lg:flex-1 lg:overflow-auto"
					data-testid="flow-record-route"
				>
					<CollectionRenderer scopeId={routeScopeId} recordId={collectionObject.id} />
				</div>
			{:else}
				<!-- Below lg the column scrolls as a whole (no inner scroller that would clip the cards). -->
				<div class="flex min-w-0 flex-col lg:flex-1 lg:overflow-auto" data-object-type="collection">
					<CollectionRenderer scopeId={routeScopeId} collectionId={collectionObject.id} />
				</div>
			{/if}
		{/key}
		<FlowContextPanel object={collectionObject} />
	</div>
{:else if unsupportedObject}
	<div class="flex items-center justify-between border-b border-line px-6 py-3">
		<h1 class="flex-1 truncate text-lg font-semibold text-fg">
			{unsupportedObject.title || $t('flow.nav.untitled')}
		</h1>
	</div>
	<div class="flex flex-1 flex-col overflow-y-auto lg:flex-row lg:overflow-hidden">
		<div
			class="flex flex-1 items-center justify-center p-8 text-center"
			data-testid="flow-object-unsupported"
			data-object-type={unsupportedObject.object_type}
		>
			<p class="text-sm text-fg-muted">
				{$t(
					unsupportedObject.object_type === 'record'
						? 'flow.personal.recordContextUnavailable'
						: 'flow.canvas.unsupported'
				)}
			</p>
		</div>
		<FlowContextPanel object={unsupportedObject} />
	</div>
{:else if !entry}
	<div class="flex flex-1 items-center justify-center p-8">
		<p class="text-sm text-fg-muted">{$t('common.loading')}</p>
	</div>
{:else}
	<FlowPageToolbar
		onManageSharedBlocks={sharedBlocksReady &&
		(permittedActions ?? (objectMetadata ?? entry.handle.object).permitted_actions ?? []).includes(
			'manage'
		)
			? () => {
					sourceShareRoot = null;
					sourceShareOpen = true;
				}
			: undefined}
		object={objectMetadata ?? entry.handle.object}
		{permittedActions}
		{syncState}
		{pending}
		bind:activeTool
	/>
	<div class="relative flex min-h-0 flex-1 overflow-hidden">
		<div
			class="flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto bg-canvas"
			data-testid="flow-body-scroll"
			data-flow-scroll-owner
		>
			{#if runtimeError}
				<div
					class="flex flex-wrap items-center gap-3 border-b border-line bg-warning-soft px-6 py-2 text-sm text-fg"
					role="alert"
				>
					<!-- Key selection lives in `#lib/flow/errors.js`, not in this template: the inline
				 `reason === 'drain' ? drain : contention` ternary this replaced silently rendered a
				 `server_draining` with a MISSING required discriminator as an ordinary contention
				 retry, which `error-mapping-v1.md` forbids ("缺失/未知 reason 违反协议"). -->
					<span class="flex-1" data-testid="flow-runtime-error" data-error-code={runtimeError.code}
						>{$t(
							editorWritePaused
								? 'flow.error.editor_transaction_aborted'
								: flowErrorDetailKey(runtimeError),
							{
								values: runtimeErrorValues(runtimeError)
							}
						)}</span
					>
					{#if integrityRepair && entry.handle.object.workspace_id}
						<a
							class="font-medium underline underline-offset-2 hover:no-underline"
							href={resolve('/(app)/workspace/[workspaceId]/settings/flow/operations', {
								workspaceId: entry.handle.object.workspace_id!
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
							class="font-medium underline underline-offset-2 hover:no-underline disabled:text-fg-muted disabled:hover:bg-transparent"
							disabled={reloading || editorWritePaused}
							onclick={reloadFromServer}
						>
							{$t('flow.recovery.reload')}
						</button>
					{/if}
					<button
						type="button"
						class="font-medium underline underline-offset-2 hover:no-underline"
						disabled={editorWritePaused}
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

			<FlowPageHeader
				scopeId={entry.handle.scopeId}
				objectId={entry.handle.objectId}
				{title}
				{readOnly}
				{appearance}
				onTitle={(value) => {
					title = value;
					onTitleInput();
				}}
				onAppearance={(value) => (appearance = value)}
			/>
			{#if canvasRenderer}
				{#if upgradingRichEditor}<p
						role="status"
						class="mx-auto mb-2 max-w-3xl text-xs text-fg-muted"
					>
						{$t('flow.canvas.upgrading')}
					</p>{/if}
				<!-- A rebuild from the accepted snapshot replaces the engine doc; remount on it. -->
				{#key docGeneration}
					<FlowCanvas
						doc={entry.doc}
						scopeId={entry.handle.scopeId}
						pickerScopeId={flowScopeId()}
						canConvert={conversionAvailability(objectMetadata ?? entry.handle.object).mode !==
							'unavailable'}
						objectId={entry.handle.objectId}
						{recoveringEdits}
						{readOnly}
						onShareBlock={sharedBlocksReady && permittedActions?.includes('manage')
							? (blockId) => {
									sourceShareRoot = blockId;
									sourceShareOpen = true;
								}
							: undefined}
						canMoveToPage={permittedActions?.includes('manage') ?? false}
						onTransferBlock={transfersReady
							? (blockId, kind) => void openTransfer(blockId, kind)
							: undefined}
						onEnsureStructure={ensureRichEditor}
						lifecycleInfo={() => (entry ? repository.editorLifecycle(entry.handle.objectId) : null)}
						presence={entry.presence}
						onPresenceCursor={reportPresence}
						onCommentBlock={permittedActions && !permittedActions.includes('comment')
							? undefined
							: (blockId) => {
									activeTool = 'comments';
									commentBlockId = blockId;
									commentNonce += 1;
								}}
					/>
				{/key}
			{:else}
				<div class="flex flex-1 items-center justify-center p-8 text-center">
					<p class="text-sm text-fg-muted">{$t('flow.canvas.unsupported')}</p>
				</div>
			{/if}
		</div>
		{@render pageToolPane(objectMetadata ?? entry.handle.object, permittedActions)}
	</div>
{/if}

<style>
	.flow-tool-pane[hidden] {
		display: none;
	}
	.flow-tool-pane {
		display: flex;
		min-height: 0;
		flex-shrink: 0;
	}
	@container flowworkspace (max-width: 899px) {
		.flow-tool-pane[hidden] {
			display: none;
		}
		.flow-tool-pane {
			position: absolute;
			inset: 0 0 0 auto;
			z-index: 40;
			max-width: 100%;
			box-shadow: -8px 0 24px var(--rf-shadow);
		}
		.flow-tool-pane :global(.flow-pane) {
			width: min(90vw, 400px);
			max-width: 100%;
		}
	}
</style>
