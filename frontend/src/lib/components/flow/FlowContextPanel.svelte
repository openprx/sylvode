<script lang="ts">
	// The object page's right panel: Info (metadata, copy reference, convert, export), Share,
	// Relations and History, as keyboard-operable tabs (`role="tablist"`, arrow keys, Home/End).
	// Share and Relations mount the first time their tab is opened and stay mounted afterwards, so
	// their reads are lazy and are not repeated on every tab switch.
	import { t } from 'svelte-i18n';
	import PackageExportWarnings from './PackageExportWarnings.svelte';
	import PortablePagePackage from './PortablePagePackage.svelte';
	import PortableGraphPackage from './PortableGraphPackage.svelte';
	import { conversionAvailability } from '#lib/flow/convert-availability.js';
	import { resolve } from '$app/paths';
	import type {
		FlowExportJob,
		FlowHistoryEntry,
		FlowObjectExportFormat,
		FlowObjectView
	} from '#lib/api/flow.js';
	import { flowApi } from '#lib/api/flow.js';
	import { FlowCommandService } from '#lib/flow/command-service.js';
	import { flowErrorFromResult } from '#lib/flow/command-service.js';
	import { flowErrorDetailKey } from '#lib/flow/errors.js';
	import { HistoryDiffLoader } from '#lib/flow/history-diff.js';
	import {
		OBJECT_EXPORT_FORMAT_KEYS,
		ObjectExportController,
		exportJobErrorKey,
		objectExportFormats,
		type ObjectExportSnapshot
	} from '#lib/flow/object-export.js';
	import { EXPORT_STATUS_KEYS } from '#lib/flow/package-export.js';
	import { toast } from '#lib/stores/toast.js';
	import FlowCommentsPanel from './FlowCommentsPanel.svelte';
	import FlowMountPanel from './FlowMountPanel.svelte';
	import FlowSharePanel from './FlowSharePanel.svelte';
	import FlowRelationsPanel from './FlowRelationsPanel.svelte';
	import FlowHistoryDiff from './FlowHistoryDiff.svelte';

	const panelId = $props.id();
	const commandService = new FlowCommandService();

	interface Props {
		object: FlowObjectView;
		permittedActions?: readonly string[] | null;
		portablePackagesReady?: boolean;
		portableGraphReady?: boolean;
		portableExportReady?: boolean;
		historyRestoreReady?: boolean;
		onHistoryRestored?: (objectId: string) => Promise<boolean>;
		onPortablePageCreated?: (objectId: string) => void;
		onGraphPagesCreated?: (objects: Array<{ id: string; parentId: string }>) => void;
		commentBlockId?: string | null;
		commentNonce?: number;
		commentRevision?: string;
		selectedTool?: 'info' | 'comments' | 'share' | 'relations' | 'history';
		onclose?: () => void;
	}

	let {
		object,
		permittedActions = null,
		portablePackagesReady = false,
		portableGraphReady = false,
		portableExportReady = false,
		historyRestoreReady = true,
		onHistoryRestored,
		onPortablePageCreated,
		onGraphPagesCreated,
		commentBlockId = null,
		commentNonce = 0,
		commentRevision = '',
		selectedTool,
		onclose
	}: Props = $props();

	const currentActions = $derived(permittedActions ?? object.permitted_actions);
	const portablePackageAvailable = $derived(
		object.object_type === 'page' &&
			object.lifecycle_status === 'active' &&
			currentActions?.includes('view') === true
	);
	const TABS = ['info', 'comments', 'share', 'relations', 'history'] as const;
	type Tab = (typeof TABS)[number];
	const TAB_KEYS: Record<Tab, string> = {
		info: 'flow.panel.tabs.info',
		comments: 'flow.panel.tabs.comments',
		share: 'flow.panel.tabs.share',
		relations: 'flow.panel.tabs.relations',
		history: 'flow.panel.tabs.history'
	};

	let activeTab = $state<Tab>('info');
	$effect(() => {
		if (selectedTool) selectTab(selectedTool);
	});
	let visited = $state<Record<Tab, boolean>>({
		info: true,
		comments: false,
		share: false,
		relations: false,
		history: false
	});
	const tabButtons: Record<string, HTMLButtonElement | undefined> = {};

	$effect(() => {
		if (commentNonce > 0) selectTab('comments');
	});

	function selectTab(tab: Tab, focus = false): void {
		activeTab = tab;
		visited[tab] = true;
		if (focus) tabButtons[tab]?.focus();
	}

	function onTabKeydown(event: KeyboardEvent): void {
		const index = TABS.indexOf(activeTab);
		let next: number | null = null;
		if (event.key === 'ArrowRight' || event.key === 'ArrowDown') next = (index + 1) % TABS.length;
		else if (event.key === 'ArrowLeft' || event.key === 'ArrowUp')
			next = (index - 1 + TABS.length) % TABS.length;
		else if (event.key === 'Home') next = 0;
		else if (event.key === 'End') next = TABS.length - 1;
		if (next === null) return;
		event.preventDefault();
		selectTab(TABS[next], true);
	}

	// ---- history ----------------------------------------------------------------------------

	let history = $state<FlowHistoryEntry[]>([]);
	let historyLoading = $state(true);
	let restoringSeq = $state<number | null>(null);
	let selectedRestoreSeq = $state<number | null>(null);
	let restoreReceipt = $state<{ targetSeq: number; headSeq: number; changed: boolean } | null>(
		null
	);
	let restoreIntent: { seq: number; key: string } | null = null;
	let restoreRefreshNeeded = $state(false);
	let historyFailure = $state<string | null>(null);
	let historyRequest = 0;
	const canRestore = $derived(
		object.object_type === 'page' &&
			object.lifecycle_status === 'active' &&
			currentActions?.includes('edit') === true
	);
	let restoreFailureKey = $state<string | null>(null);
	let nextBeforeSeq = $state<number | undefined>(undefined);
	let expanded = $state<Record<number, boolean>>({});
	let diffLoader = $state(new HistoryDiffLoader(''));

	async function loadHistory(before?: number) {
		const objectId = object.id;
		const request = ++historyRequest;
		historyLoading = true;
		historyFailure = null;
		try {
			const result = await commandService.getHistory(objectId, { before_seq: before, limit: 20 });
			if (object.id !== objectId || historyRequest !== request) return;
			if (result.code === 0 && result.data) {
				history = before ? [...history, ...result.data.items] : result.data.items;
				nextBeforeSeq = result.data.next_before_seq;
			} else {
				historyFailure = flowErrorDetailKey(flowErrorFromResult(result));
			}
		} catch {
			if (object.id === objectId && historyRequest === request)
				historyFailure = 'flow.history.loadFailed';
		} finally {
			if (object.id === objectId && historyRequest === request) historyLoading = false;
		}
	}

	async function restoreVersion(seq: number): Promise<void> {
		if (restoringSeq !== null || selectedRestoreSeq !== seq || !canRestore || !historyRestoreReady)
			return;
		const objectId = object.id;
		restoringSeq = seq;
		restoreFailureKey = null;
		restoreReceipt = null;
		restoreRefreshNeeded = false;
		const intent = restoreIntent?.seq === seq ? restoreIntent : { seq, key: crypto.randomUUID() };
		restoreIntent = intent;
		try {
			const result = await flowApi.restoreVersion(objectId, { seq, idempotency_key: intent.key });
			if (object.id !== objectId || restoreIntent !== intent) return;
			if (result.code !== 0 || !result.data) {
				const details = result.details;
				const reason =
					typeof details === 'object' && details !== null && 'reason' in details
						? details.reason
						: null;
				if (result.code >= 400 && result.code < 499) restoreIntent = null;
				restoreFailureKey =
					reason === 'history_not_retained'
						? 'flow.history.notRetained'
						: result.code !== 0
							? flowErrorDetailKey(flowErrorFromResult(result))
							: 'flow.history.restoreFailed';
				return;
			}
			restoreIntent = null;
			selectedRestoreSeq = null;
			const { target_seq, head_seq, changed } = result.data;
			if (changed && onHistoryRestored) {
				const updated = await onHistoryRestored(objectId);
				if (object.id !== objectId) return;
				restoreRefreshNeeded = !updated;
			}
			restoreReceipt = { targetSeq: target_seq, headSeq: head_seq, changed };
			diffLoader = new HistoryDiffLoader(objectId);
			expanded = {};
			await loadHistory();
		} catch {
			if (object.id === objectId) restoreFailureKey = 'flow.history.restoreFailed';
		} finally {
			if (object.id === objectId) restoringSeq = null;
		}
	}

	async function refreshRestoredPage() {
		if (!onHistoryRestored) return;
		const objectId = object.id;
		const updated = await onHistoryRestored(objectId);
		if (object.id === objectId && updated) restoreRefreshNeeded = false;
	}

	// ---- export -----------------------------------------------------------------------------

	let exportFormat = $state<FlowObjectExportFormat>('markdown');
	let includeHistory = $state(false);
	let exportState = $state<ObjectExportSnapshot>({ job: null, format: null });
	let exportFailureKey = $state<string | null>(null);
	let exporting = $state(false);
	let downloading = $state(false);
	let needsFreshExport = $state(false);
	let liveMessage = $state('');
	let exportController: ObjectExportController | null = null;
	const formats = $derived(
		objectExportFormats(
			object.object_type,
			object.supported_export_formats ??
				((object.minimum_editor_model ?? 1) >= 5 ? [] : object.workspace_id ? undefined : [])
		)
	);
	const historyExportSupported = $derived(
		object.history_export_supported ??
			((object.minimum_editor_model ?? 1) >= 5 ? false : Boolean(object.workspace_id))
	);
	const exportUnavailableKey = $derived(
		object.export_unavailable_reason === 'field_secrecy'
			? 'flow.panel.export.fieldSecrecy'
			: 'flow.personal.exportUnavailable'
	);

	$effect(() => {
		// Re-initialise everything object-scoped whenever the panel is pointed at another object.
		const objectId = object.id;
		const scopeId = object.scope_id;
		history = [];
		selectedRestoreSeq = null;
		restoringSeq = null;
		restoreIntent = null;
		restoreFailureKey = null;
		restoreReceipt = null;
		restoreRefreshNeeded = false;
		historyFailure = null;
		nextBeforeSeq = undefined;
		expanded = {};
		diffLoader = new HistoryDiffLoader(objectId);
		const controller = new ObjectExportController(
			objectId,
			scopeId,
			(value) => (exportState = value)
		);
		exportController = controller;
		exportState = controller.snapshot;
		exportFailureKey = null;
		needsFreshExport = false;
		return () => {
			controller.dispose();
			historyRequest++;
			if (exportController === controller) exportController = null;
		};
	});

	$effect(() => {
		if (!formats.includes(exportFormat)) exportFormat = formats[0] ?? 'markdown';
		if (exportFormat !== 'package' || !historyExportSupported) includeHistory = false;
	});

	$effect(() => {
		if (activeTab === 'history') {
			void object.id;
			void loadHistory();
		}
	});
	async function startExport(): Promise<void> {
		if (!exportController || !formats.includes(exportFormat)) return;
		exporting = true;
		exportFailureKey = null;
		const outcome = await exportController.submit({ format: exportFormat, includeHistory });
		exporting = false;
		if (outcome.status === 'failed') {
			exportFailureKey = outcome.failure.messageKey;
			liveMessage = $t(outcome.failure.messageKey);
		} else if (outcome.status === 'started') {
			needsFreshExport = false;
			liveMessage = $t('flow.panel.export.started');
		}
	}

	async function downloadJob(job: FlowExportJob): Promise<void> {
		if (!exportController) return;
		downloading = true;
		exportFailureKey = null;
		const outcome = await exportController.download(job, object.title);
		downloading = false;
		if (outcome.status === 'failed') {
			exportFailureKey = outcome.failure.messageKey;
			if (outcome.failure.cause === 'export_authority_changed') needsFreshExport = true;
			return;
		}
		const url = URL.createObjectURL(outcome.blob);
		const anchor = document.createElement('a');
		anchor.href = url;
		anchor.download = outcome.fileName;
		anchor.rel = 'noopener';
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		setTimeout(() => URL.revokeObjectURL(url), 60_000);
		liveMessage = $t('flow.panel.export.downloaded', { values: { name: outcome.fileName } });
	}

	function statusKey(status: string): string {
		return EXPORT_STATUS_KEYS[status] ?? 'flow.export.status.unknown';
	}

	// ---- info -------------------------------------------------------------------------------

	async function copyReference() {
		try {
			await navigator.clipboard.writeText(object.id);
			toast.success($t('flow.panel.copied'));
		} catch {
			toast.error($t('flow.panel.copyReference'));
		}
	}

	function formatTime(value: string): string {
		try {
			return new Date(value).toLocaleString();
		} catch {
			return value;
		}
	}

	/** `FlowHistoryEntry.semantic_summary` is a structured JSON object server-side (`accepted_seq`,
	 * `changed_block_ids`, ...), not a rendered string -- render the fields a human can act on. */
	function formatSummary(summary: unknown): string {
		if (
			summary &&
			typeof summary === 'object' &&
			'imported' in summary &&
			summary.imported === true &&
			'source_seq' in summary &&
			typeof summary.source_seq === 'number'
		) {
			return $t('flow.history.importedVersion', { values: { version: summary.source_seq } });
		}
		if (summary && typeof summary === 'object' && 'accepted_seq' in summary) {
			const s = summary as { accepted_seq: number; changed_block_ids?: unknown[] };
			const blocks = Array.isArray(s.changed_block_ids) ? s.changed_block_ids.length : 0;
			return `seq ${s.accepted_seq}${blocks > 0 ? ` · ${blocks}` : ''}`;
		}
		return typeof summary === 'string' ? summary : '';
	}

	const linkButtonClass =
		'min-h-11 text-xs font-medium text-fg underline disabled:text-fg-muted disabled:hover:bg-transparent';
	const selectClass =
		'min-h-11 w-full rounded-control border border-input bg-field px-2 text-sm text-fg';
</script>

<aside
	class="flex w-full shrink-0 flex-col gap-4 border-t border-line bg-canvas p-4 lg:w-full lg:border-l lg:border-t-0"
	aria-label={$t('flow.panel.title')}
>
	<p class="sr-only" aria-live="polite">{liveMessage}</p>
	{#if selectedTool}
		<div class="flex items-center justify-between gap-3">
			<h2 class="font-semibold">{$t(TAB_KEYS[selectedTool])}</h2>
			<button
				type="button"
				class="min-h-11 min-w-11 rounded hover:bg-surface"
				aria-label={$t('common.close')}
				onclick={onclose}>✕</button
			>
		</div>
	{:else}
		<div
			role="tablist"
			aria-label={$t('flow.panel.tabs.label')}
			class="grid grid-cols-5 gap-1 border-b border-line"
		>
			{#each TABS as tab (tab)}
				<button
					bind:this={tabButtons[tab]}
					type="button"
					role="tab"
					id={`${panelId}-tab-${tab}`}
					aria-selected={activeTab === tab}
					aria-controls={`${panelId}-${tab}`}
					tabindex={activeTab === tab ? 0 : -1}
					class={`min-h-11 border-b-2 px-1 text-xs font-medium ${
						activeTab === tab
							? 'border-fg text-fg'
							: 'border-transparent text-fg-muted hover:text-fg'
					}`}
					onclick={() => selectTab(tab)}
					onkeydown={onTabKeydown}
				>
					{$t(TAB_KEYS[tab])}
				</button>
			{/each}
		</div>
	{/if}
	<div
		id={`${panelId}-info`}
		role="tabpanel"
		aria-label={$t(TAB_KEYS[activeTab])}
		data-tab-label="flow-panel-tab-info"
		hidden={activeTab !== 'info'}
		class="space-y-6"
	>
		<div>
			<h2 class="mb-3 text-sm font-semibold text-fg">
				{$t('flow.panel.title')}
			</h2>
			<dl class="space-y-2 text-sm">
				<div class="flex items-center justify-between gap-2">
					<dt class="text-fg-muted">{$t('flow.panel.objectId')}</dt>
					<dd class="flex items-center gap-2">
						<code class="truncate text-xs text-fg-muted" title={object.id}>
							{object.id.slice(0, 8)}…
						</code>
						<button
							type="button"
							class="text-xs font-medium text-fg underline"
							onclick={copyReference}
						>
							{$t('flow.panel.copyReference')}
						</button>
					</dd>
				</div>
				<div class="flex items-center justify-between">
					<dt class="text-fg-muted">{$t('flow.panel.created')}</dt>
					<dd class="text-fg">{formatTime(object.created_at)}</dd>
				</div>
				<div class="flex items-center justify-between">
					<dt class="text-fg-muted">{$t('flow.panel.updated')}</dt>
					<dd class="text-fg">{formatTime(object.updated_at)}</dd>
				</div>
			</dl>
		</div>

		{#if object.object_type === 'page'}
			{#if conversionAvailability( { ...object, permitted_actions: currentActions ? [...currentActions] : undefined } ).mode !== 'unavailable'}
				<a
					href={resolve('/(app)/flow/[objectId]/convert', { objectId: object.id })}
					class="flex min-h-11 items-center justify-center rounded-control border border-fg bg-raised px-3 text-sm font-medium text-fg hover:bg-surface"
					data-testid="flow-panel-convert"
				>
					{$t('flow.bridge.convert.panelAction')}
				</a>
			{:else}<p class="text-xs text-fg-muted" role="status">
					{$t('flow.personal.conversionUnavailable')}
				</p>{/if}
		{/if}

		{#if portableGraphReady && portablePackageAvailable}
			{#key object.id}
				<PortableGraphPackage
					{object}
					canImport={currentActions?.includes('edit') === true}
					exportReady={portableExportReady}
					oncreated={onGraphPagesCreated}
				/>
			{/key}
		{/if}
		{#if portablePackagesReady && portablePackageAvailable}
			{#key object.id}
				<PortablePagePackage
					objectId={object.id}
					canImport={currentActions?.includes('edit') === true}
					exportReady={portableExportReady}
					oncreated={onPortablePageCreated}
				/>
			{/key}
		{/if}

		<div class="space-y-3 text-sm" data-testid="flow-panel-export">
			<h2 class="text-sm font-semibold text-fg">
				{$t('flow.panel.export.heading')}
			</h2>
			{#if (object.minimum_editor_model ?? 1) >= 5}<p class="text-xs text-fg-muted">
					{$t('flow.body.referenceExportOnly')}
				</p>
				{#if !formats.includes('package') && !((portablePackagesReady || portableGraphReady) && portablePackageAvailable)}<p
						class="text-xs text-fg-muted"
					>
						{$t('flow.body.rawPackageUnavailable')}
					</p>{/if}{/if}
			{#if formats.length > 0}
				<label class="block">
					<span class="mb-1 block text-xs text-fg-muted">{$t('flow.panel.export.formatLabel')}</span
					>
					<select
						aria-label={$t('flow.panel.export.formatLabel')}
						class={selectClass}
						bind:value={exportFormat}
						disabled={exporting || formats.length === 0}
					>
						{#each formats as format (format)}
							<option value={format}>{$t(OBJECT_EXPORT_FORMAT_KEYS[format])}</option>
						{/each}
					</select>
				</label>
				<label class="flex min-h-11 items-center gap-3 text-fg">
					<input
						type="checkbox"
						class="h-5 w-5"
						bind:checked={includeHistory}
						disabled={exporting || exportFormat !== 'package' || !historyExportSupported}
						aria-describedby="flow-panel-export-history-hint"
					/>
					<span>{$t('flow.panel.export.includeHistory')}</span>
				</label>
				<p id="flow-panel-export-history-hint" class="text-xs text-fg-muted">
					{$t(
						historyExportSupported
							? 'flow.panel.export.historyPackageOnly'
							: 'flow.personal.historyUnavailable'
					)}
				</p>
			{/if}
			{#if exportFormat === 'package' && formats.includes('package')}
				<p
					class="my-3 rounded-card border border-line bg-warning-soft p-3 text-sm text-fg"
					data-testid="flow-object-package-history-notice"
				>
					{$t('flow.package.snapshotHistoryNotice')}
				</p>
			{/if}
			<button
				type="button"
				class="min-h-11 w-full rounded-control bg-primary rf-filled px-3 text-sm font-medium text-on-primary disabled:bg-surface disabled:text-fg-muted"
				disabled={exporting || formats.length === 0 || exportState.job?.polling === true}
				aria-describedby={formats.length === 0 ? 'flow-panel-export-unavailable' : undefined}
				onclick={startExport}
			>
				{exporting
					? $t('flow.panel.export.exporting')
					: needsFreshExport
						? $t('flow.personal.regenerateExport')
						: $t('flow.panel.export.start')}
			</button>
			{#if formats.length === 0}
				<p id="flow-panel-export-unavailable" class="text-xs text-fg-muted" role="status">
					{$t(exportUnavailableKey)}
				</p>
			{/if}
			{#if exportState.job}
				{@const view = exportState.job}
				<div class="space-y-1" data-testid="flow-panel-export-job" data-job-id={view.jobId}>
					<p class="text-fg">
						{$t('flow.export.status.label')}:
						<span data-testid="flow-panel-export-status">
							{view.job
								? $t(statusKey(view.job.status), { values: { status: view.job.status } })
								: $t('flow.export.status.loading')}
						</span>
					</p>
					{#if view.job?.format === 'package' && view.job.status !== 'failed'}<PackageExportWarnings
							warnings={view.job.warnings}
						/>{/if}
					{#if view.failure}
						<p class="text-danger" role="alert">{$t(view.failure.messageKey)}</p>
					{:else if exportJobErrorKey(view.job)}
						<p class="text-danger" role="alert">
							{$t(exportJobErrorKey(view.job) ?? 'flow.panel.export.failed')}
						</p>
					{/if}
					{#if view.job && view.job.status === 'completed' && view.job.download_url}
						{@const job = view.job}
						<button
							type="button"
							class={linkButtonClass}
							disabled={downloading || needsFreshExport}
							onclick={() => downloadJob(job)}
						>
							{downloading ? $t('flow.panel.export.downloading') : $t('flow.panel.export.download')}
						</button>
					{/if}
				</div>
			{/if}
			{#if exportFailureKey}
				<p class="text-danger" role="alert">{$t(exportFailureKey)}</p>
			{/if}
		</div>
	</div>

	<div
		id={`${panelId}-comments`}
		role="tabpanel"
		aria-label={$t(TAB_KEYS[activeTab])}
		data-tab-label="flow-panel-tab-comments"
		hidden={activeTab !== 'comments'}
	>
		<h2 class="mb-3 text-sm font-semibold text-fg">
			{$t('flow.comments.title')}
		</h2>
		{#if visited.comments}
			<FlowCommentsPanel
				canComment={currentActions?.includes('comment') ?? true}
				canEdit={currentActions?.includes('edit') ?? false}
				canManage={currentActions?.includes('manage') ?? false}
				objectId={object.id}
				blockId={commentBlockId}
				nonce={commentNonce}
				revision={commentRevision}
			/>
		{/if}
	</div>

	<div
		id={`${panelId}-share`}
		role="tabpanel"
		aria-label={$t(TAB_KEYS[activeTab])}
		data-tab-label="flow-panel-tab-share"
		hidden={activeTab !== 'share'}
	>
		<h2 class="mb-3 text-sm font-semibold text-fg">
			{$t('flow.share.title')}
		</h2>
		{#if visited.share}
			<FlowSharePanel
				permittedActions={currentActions}
				scopeId={object.scope_id}
				objectId={object.id}
				workspaceId={object.workspace_id ?? null}
			/>
			{#if !object.workspace_id && currentActions?.includes('manage')}
				{#if object.object_type !== 'record' || object.record_context_supported === true}<FlowMountPanel
						objectId={object.id}
					/>{:else}<p class="text-sm text-fg-muted">
						{$t('flow.personal.recordContextUnavailable')}
					</p>{/if}
			{/if}
		{/if}
	</div>

	<div
		id={`${panelId}-relations`}
		role="tabpanel"
		aria-label={$t(TAB_KEYS[activeTab])}
		data-tab-label="flow-panel-tab-relations"
		hidden={activeTab !== 'relations'}
	>
		<h2 class="mb-3 text-sm font-semibold text-fg">
			{$t('flow.relations.title')}
		</h2>
		{#if visited.relations}
			<FlowRelationsPanel objectId={object.id} scopeId={object.scope_id} />
		{/if}
	</div>

	<div
		id={`${panelId}-history`}
		role="tabpanel"
		aria-label={$t(TAB_KEYS[activeTab])}
		data-tab-label="flow-panel-tab-history"
		hidden={activeTab !== 'history'}
	>
		<h2 class="mb-3 text-sm font-semibold text-fg">
			{$t('flow.history.title')}
		</h2>
		{#if object.historical_asset_reads_supported === false && (object.minimum_editor_model ?? 1) >= 5}<p
				class="mb-3 text-xs text-fg-muted"
			>
				{$t('flow.body.historyPreviewUnavailable')}
			</p>{/if}
		{#if historyFailure}
			<p role="alert" class="text-sm text-danger">{$t(historyFailure)}</p>
			<button type="button" class={linkButtonClass} onclick={() => loadHistory()}
				>{$t('flow.mounts.retry')}</button
			>
		{/if}
		{#if restoreReceipt}
			<p role="status" class="mb-3 text-sm font-medium" data-testid="flow-history-restore-receipt">
				{$t(
					restoreReceipt.changed ? 'flow.history.restoreChanged' : 'flow.history.restoreUnchanged',
					{
						values: { version: restoreReceipt.targetSeq, head: restoreReceipt.headSeq }
					}
				)}
			</p>
		{/if}
		{#if restoreRefreshNeeded}
			<p role="status" class="mb-3 text-sm">{$t('flow.history.restoreRefreshNeeded')}</p>
			<button type="button" class={linkButtonClass} onclick={refreshRestoredPage}
				>{$t('flow.history.refreshPage')}</button
			>
		{/if}
		{#if canRestore && !historyRestoreReady}<p class="mb-3 text-sm">
				{$t('flow.history.restoreWait')}
			</p>{/if}
		{#if historyLoading && history.length === 0}
			<p class="text-sm text-fg-muted">{$t('common.loading')}</p>
		{:else if history.length === 0}
			<p class="text-sm text-fg-muted">{$t('flow.history.empty')}</p>
		{:else}
			<ul class="space-y-3" data-testid="flow-history-list">
				{#each history as entry (entry.seq)}
					<li
						class="border-l-2 border-line pl-3 text-sm"
						data-testid="flow-history-entry"
						data-version-seq={entry.seq}
					>
						<p class="font-medium">
							{$t('flow.history.versionLabel', { values: { version: entry.seq } })}
						</p>
						{#if entry.message}
							<p class="text-fg">{entry.message}</p>
						{/if}
						<p class="text-fg">
							{formatSummary(entry.semantic_summary)}
						</p>
						<p class="text-xs text-fg-muted">
							{$t('flow.history.entry', {
								values: {
									actor: entry.actor ?? $t('flow.history.actorUnknown'),
									time: formatTime(entry.created_at)
								}
							})}
						</p>
						{#if canRestore}
							{#if selectedRestoreSeq === entry.seq}
								<p class="mt-2 text-sm" id={`${panelId}-restore-${entry.seq}`}>
									{$t('flow.history.restoreConfirm', { values: { version: entry.seq } })}
								</p>
								<div class="flex flex-wrap gap-2">
									<button
										type="button"
										class={linkButtonClass}
										aria-describedby={`${panelId}-restore-${entry.seq}`}
										data-testid="flow-history-confirm-restore"
										disabled={restoringSeq !== null || !historyRestoreReady}
										onclick={() => restoreVersion(entry.seq)}
									>
										{restoringSeq === entry.seq
											? $t('flow.history.restoring')
											: $t('flow.history.restore')}
									</button>
									<button
										type="button"
										class={linkButtonClass}
										disabled={restoringSeq !== null}
										onclick={() => (selectedRestoreSeq = null)}>{$t('common.cancel')}</button
									>
								</div>
							{:else}
								<button
									type="button"
									class={linkButtonClass}
									disabled={restoringSeq !== null || !historyRestoreReady}
									onclick={() => (selectedRestoreSeq = entry.seq)}
									>{$t('flow.history.restore')}</button
								>
							{/if}
						{/if}
						{#if entry.seq > 0}
							<button
								type="button"
								class={linkButtonClass}
								aria-expanded={expanded[entry.seq] === true}
								aria-controls={`flow-history-diff-${entry.seq}`}
								onclick={() => (expanded[entry.seq] = !expanded[entry.seq])}
							>
								{expanded[entry.seq] ? $t('flow.history.diff.hide') : $t('flow.history.diff.show')}
							</button>
							{#if expanded[entry.seq]}
								<div id={`flow-history-diff-${entry.seq}`}>
									<FlowHistoryDiff seq={entry.seq} loader={diffLoader} />
								</div>
							{/if}
						{/if}
					</li>
				{/each}
			</ul>
			{#if restoreFailureKey}
				<p class="text-sm text-danger" role="alert">{$t(restoreFailureKey)}</p>
			{/if}
			{#if nextBeforeSeq}
				<button
					type="button"
					class={`mt-3 ${linkButtonClass}`}
					onclick={() => loadHistory(nextBeforeSeq)}
				>
					{$t('flow.history.loadMore')}
				</button>
			{/if}
		{/if}
	</div>
</aside>
