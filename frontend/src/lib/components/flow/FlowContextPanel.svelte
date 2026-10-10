<script lang="ts">
	// The object page's right panel: Info (metadata, copy reference, convert, export), Share,
	// Relations and History, as keyboard-operable tabs (`role="tablist"`, arrow keys, Home/End).
	// Share and Relations mount the first time their tab is opened and stay mounted afterwards, so
	// their reads are lazy and are not repeated on every tab switch.
	import { t } from 'svelte-i18n';
	import { resolve } from '$app/paths';
	import type {
		FlowExportJob,
		FlowHistoryEntry,
		FlowObjectExportFormat,
		FlowObjectView
	} from '$lib/api/flow';
	import { FlowCommandService } from '$lib/flow/command-service';
	import { HistoryDiffLoader } from '$lib/flow/history-diff';
	import {
		OBJECT_EXPORT_FORMAT_KEYS,
		ObjectExportController,
		exportJobErrorKey,
		objectExportFormats,
		type ObjectExportSnapshot
	} from '$lib/flow/object-export';
	import { EXPORT_STATUS_KEYS } from '$lib/flow/package-export';
	import { toast } from '$lib/stores/toast';
	import FlowSharePanel from './FlowSharePanel.svelte';
	import FlowRelationsPanel from './FlowRelationsPanel.svelte';
	import FlowHistoryDiff from './FlowHistoryDiff.svelte';

	const commandService = new FlowCommandService();

	interface Props {
		object: FlowObjectView;
	}

	let { object }: Props = $props();

	const TABS = ['info', 'share', 'relations', 'history'] as const;
	type Tab = (typeof TABS)[number];
	const TAB_KEYS: Record<Tab, string> = {
		info: 'flow.panel.tabs.info',
		share: 'flow.panel.tabs.share',
		relations: 'flow.panel.tabs.relations',
		history: 'flow.panel.tabs.history'
	};

	let activeTab = $state<Tab>('info');
	let visited = $state<Record<Tab, boolean>>({
		info: true,
		share: false,
		relations: false,
		history: false
	});
	const tabButtons: Record<string, HTMLButtonElement | undefined> = {};

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
	let nextBeforeSeq = $state<number | undefined>(undefined);
	let expanded = $state<Record<number, boolean>>({});
	let diffLoader = $state(new HistoryDiffLoader(''));

	async function loadHistory(before?: number) {
		historyLoading = true;
		const result = await commandService.getHistory(object.id, { before_seq: before, limit: 20 });
		if (result.code === 0 && result.data) {
			history = before ? [...history, ...result.data.items] : result.data.items;
			nextBeforeSeq = result.data.next_before_seq;
		}
		historyLoading = false;
	}

	// ---- export -----------------------------------------------------------------------------

	let exportFormat = $state<FlowObjectExportFormat>('markdown');
	let includeHistory = $state(false);
	let exportState = $state<ObjectExportSnapshot>({ job: null, format: null });
	let exportFailureKey = $state<string | null>(null);
	let exporting = $state(false);
	let downloading = $state(false);
	let liveMessage = $state('');
	let exportController: ObjectExportController | null = null;
	const formats = $derived(objectExportFormats(object.object_type));

	$effect(() => {
		// Re-initialise everything object-scoped whenever the panel is pointed at another object.
		const objectId = object.id;
		const workspaceId = object.workspace_id;
		history = [];
		nextBeforeSeq = undefined;
		expanded = {};
		diffLoader = new HistoryDiffLoader(objectId);
		void loadHistory();
		const controller = new ObjectExportController(
			objectId,
			workspaceId,
			(value) => (exportState = value)
		);
		exportController = controller;
		exportState = controller.snapshot;
		exportFailureKey = null;
		return () => {
			controller.dispose();
			if (exportController === controller) exportController = null;
		};
	});

	$effect(() => {
		if (!formats.includes(exportFormat)) exportFormat = 'markdown';
		if (exportFormat !== 'package') includeHistory = false;
	});

	async function startExport(): Promise<void> {
		if (!exportController) return;
		exporting = true;
		exportFailureKey = null;
		const outcome = await exportController.submit({ format: exportFormat, includeHistory });
		exporting = false;
		if (outcome.status === 'failed') {
			exportFailureKey = outcome.failure.messageKey;
			liveMessage = $t(outcome.failure.messageKey);
		} else if (outcome.status === 'started') {
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
		if (summary && typeof summary === 'object' && 'accepted_seq' in summary) {
			const s = summary as { accepted_seq: number; changed_block_ids?: unknown[] };
			const blocks = Array.isArray(s.changed_block_ids) ? s.changed_block_ids.length : 0;
			return `seq ${s.accepted_seq}${blocks > 0 ? ` · ${blocks}` : ''}`;
		}
		return typeof summary === 'string' ? summary : '';
	}

	const linkButtonClass =
		'min-h-11 text-xs font-medium text-blue-600 hover:underline focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 disabled:opacity-50 dark:text-blue-400';
	const selectClass =
		'min-h-11 w-full rounded-md border border-slate-300 bg-white px-2 text-sm text-slate-900 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100';
</script>

<aside
	class="flex w-full shrink-0 flex-col gap-4 border-t border-slate-200 bg-white p-4 lg:h-full lg:w-80 lg:overflow-y-auto lg:border-l lg:border-t-0 dark:border-slate-800 dark:bg-slate-900"
	aria-label={$t('flow.panel.title')}
>
	<p class="sr-only" aria-live="polite">{liveMessage}</p>
	<div
		role="tablist"
		aria-label={$t('flow.panel.tabs.label')}
		class="grid grid-cols-4 gap-1 border-b border-slate-200 dark:border-slate-700"
	>
		{#each TABS as tab (tab)}
			<button
				bind:this={tabButtons[tab]}
				type="button"
				role="tab"
				id={`flow-panel-tab-${tab}`}
				aria-selected={activeTab === tab}
				aria-controls={`flow-panel-${tab}`}
				tabindex={activeTab === tab ? 0 : -1}
				class={`min-h-11 border-b-2 px-1 text-xs font-medium focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 ${
					activeTab === tab
						? 'border-blue-600 text-blue-700 dark:border-blue-400 dark:text-blue-300'
						: 'border-transparent text-slate-600 hover:text-slate-900 dark:text-slate-400 dark:hover:text-slate-100'
				}`}
				onclick={() => selectTab(tab)}
				onkeydown={onTabKeydown}
			>
				{$t(TAB_KEYS[tab])}
			</button>
		{/each}
	</div>

	<div
		id="flow-panel-info"
		role="tabpanel"
		aria-labelledby="flow-panel-tab-info"
		hidden={activeTab !== 'info'}
		class="space-y-6"
	>
		<div>
			<h2 class="mb-3 text-sm font-semibold text-slate-900 dark:text-slate-100">
				{$t('flow.panel.title')}
			</h2>
			<dl class="space-y-2 text-sm">
				<div class="flex items-center justify-between gap-2">
					<dt class="text-slate-500 dark:text-slate-400">{$t('flow.panel.objectId')}</dt>
					<dd class="flex items-center gap-2">
						<code class="truncate text-xs text-slate-600 dark:text-slate-300" title={object.id}>
							{object.id.slice(0, 8)}…
						</code>
						<button
							type="button"
							class="text-xs font-medium text-blue-600 hover:underline dark:text-blue-400"
							onclick={copyReference}
						>
							{$t('flow.panel.copyReference')}
						</button>
					</dd>
				</div>
				<div class="flex items-center justify-between">
					<dt class="text-slate-500 dark:text-slate-400">{$t('flow.panel.created')}</dt>
					<dd class="text-slate-700 dark:text-slate-200">{formatTime(object.created_at)}</dd>
				</div>
				<div class="flex items-center justify-between">
					<dt class="text-slate-500 dark:text-slate-400">{$t('flow.panel.updated')}</dt>
					<dd class="text-slate-700 dark:text-slate-200">{formatTime(object.updated_at)}</dd>
				</div>
			</dl>
		</div>

		{#if object.object_type === 'page'}
			<a
				href={resolve('/(app)/workspace/[workspaceId]/flow/[objectId]/convert', {
					workspaceId: object.workspace_id,
					objectId: object.id
				})}
				class="flex min-h-11 items-center justify-center rounded-md border border-slate-300 px-3 text-sm font-medium text-slate-700 hover:bg-slate-100 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:border-slate-600 dark:text-slate-200 dark:hover:bg-slate-800"
				data-testid="flow-panel-convert"
			>
				{$t('flow.bridge.convert.panelAction')}
			</a>
		{/if}

		<div class="space-y-3 text-sm" data-testid="flow-panel-export">
			<h2 class="text-sm font-semibold text-slate-900 dark:text-slate-100">
				{$t('flow.panel.export.heading')}
			</h2>
			<label class="block">
				<span class="mb-1 block text-xs text-slate-500 dark:text-slate-400"
					>{$t('flow.panel.export.formatLabel')}</span
				>
				<select
					aria-label={$t('flow.panel.export.formatLabel')}
					class={selectClass}
					bind:value={exportFormat}
					disabled={exporting}
				>
					{#each formats as format (format)}
						<option value={format}>{$t(OBJECT_EXPORT_FORMAT_KEYS[format])}</option>
					{/each}
				</select>
			</label>
			<label class="flex min-h-11 items-center gap-3 text-slate-700 dark:text-slate-200">
				<input
					type="checkbox"
					class="h-5 w-5 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500"
					bind:checked={includeHistory}
					disabled={exporting || exportFormat !== 'package'}
					aria-describedby="flow-panel-export-history-hint"
				/>
				<span>{$t('flow.panel.export.includeHistory')}</span>
			</label>
			<p id="flow-panel-export-history-hint" class="text-xs text-slate-500 dark:text-slate-400">
				{$t('flow.panel.export.historyPackageOnly')}
			</p>
			<button
				type="button"
				class="min-h-11 w-full rounded-md bg-blue-600 px-3 text-sm font-medium text-white hover:bg-blue-700 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 focus-visible:ring-offset-2 disabled:opacity-50"
				disabled={exporting || exportState.job?.polling === true}
				onclick={startExport}
			>
				{exporting ? $t('flow.panel.export.exporting') : $t('flow.panel.export.start')}
			</button>
			{#if exportState.job}
				{@const view = exportState.job}
				<div class="space-y-1" data-testid="flow-panel-export-job">
					<p class="text-slate-700 dark:text-slate-200">
						{$t('flow.export.status.label')}:
						<span data-testid="flow-panel-export-status">
							{view.job
								? $t(statusKey(view.job.status), { values: { status: view.job.status } })
								: $t('flow.export.status.loading')}
						</span>
					</p>
					{#if view.failure}
						<p class="text-red-600 dark:text-red-400" role="alert">{$t(view.failure.messageKey)}</p>
					{:else if exportJobErrorKey(view.job)}
						<p class="text-red-600 dark:text-red-400" role="alert">
							{$t(exportJobErrorKey(view.job) ?? 'flow.panel.export.failed')}
						</p>
					{/if}
					{#if view.job && view.job.status === 'completed' && view.job.download_url}
						{@const job = view.job}
						<button
							type="button"
							class={linkButtonClass}
							disabled={downloading}
							onclick={() => downloadJob(job)}
						>
							{downloading ? $t('flow.panel.export.downloading') : $t('flow.panel.export.download')}
						</button>
					{/if}
				</div>
			{/if}
			{#if exportFailureKey}
				<p class="text-red-600 dark:text-red-400" role="alert">{$t(exportFailureKey)}</p>
			{/if}
		</div>
	</div>

	<div
		id="flow-panel-share"
		role="tabpanel"
		aria-labelledby="flow-panel-tab-share"
		hidden={activeTab !== 'share'}
	>
		<h2 class="mb-3 text-sm font-semibold text-slate-900 dark:text-slate-100">
			{$t('flow.share.title')}
		</h2>
		{#if visited.share}
			<FlowSharePanel objectId={object.id} workspaceId={object.workspace_id} />
		{/if}
	</div>

	<div
		id="flow-panel-relations"
		role="tabpanel"
		aria-labelledby="flow-panel-tab-relations"
		hidden={activeTab !== 'relations'}
	>
		<h2 class="mb-3 text-sm font-semibold text-slate-900 dark:text-slate-100">
			{$t('flow.relations.title')}
		</h2>
		{#if visited.relations}
			<FlowRelationsPanel objectId={object.id} workspaceId={object.workspace_id} />
		{/if}
	</div>

	<div
		id="flow-panel-history"
		role="tabpanel"
		aria-labelledby="flow-panel-tab-history"
		hidden={activeTab !== 'history'}
	>
		<h2 class="mb-3 text-sm font-semibold text-slate-900 dark:text-slate-100">
			{$t('flow.history.title')}
		</h2>
		{#if historyLoading && history.length === 0}
			<p class="text-sm text-slate-500 dark:text-slate-400">{$t('common.loading')}</p>
		{:else if history.length === 0}
			<p class="text-sm text-slate-500 dark:text-slate-400">{$t('flow.history.empty')}</p>
		{:else}
			<ul class="space-y-3" data-testid="flow-history-list">
				{#each history as entry (entry.seq)}
					<li
						class="border-l-2 border-slate-200 pl-3 text-sm dark:border-slate-700"
						data-testid="flow-history-entry"
					>
						{#if entry.message}
							<p class="text-slate-800 dark:text-slate-100">{entry.message}</p>
						{/if}
						<p class="text-slate-700 dark:text-slate-200">
							{formatSummary(entry.semantic_summary)}
						</p>
						<p class="text-xs text-slate-400 dark:text-slate-500">
							{$t('flow.history.entry', {
								values: {
									actor: entry.actor ?? $t('flow.history.actorUnknown'),
									time: formatTime(entry.created_at)
								}
							})}
						</p>
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
					</li>
				{/each}
			</ul>
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
