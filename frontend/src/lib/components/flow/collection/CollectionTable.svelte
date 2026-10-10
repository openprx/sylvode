<script lang="ts">
	// `collection:table` renderer. Rows are virtualized (`virtualWindow`: only the visible slice
	// plus overscan is in the DOM), columns are resizable, cells use typed editors. It is a
	// `role="grid"`: arrow keys move the focused cell, Enter edits, Escape cancels. Below 768px
	// the table becomes a card list. Every write goes through the controller; this component only
	// keeps the focused cell, the cell being edited and a column drag in progress.
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import {
		CollectionViewController,
		columnWidthIn,
		draftIn,
		nextGridCell,
		virtualWindow,
		type CollectionViewState,
		type RecordRow
	} from '$lib/flow/collection-view';
	import {
		COLUMN_WIDTH_DEFAULT,
		editorValue,
		isEditableField,
		type CollectionField
	} from '$lib/flow/collection-values';
	import CollectionCellValue from './CollectionCellValue.svelte';
	import CollectionValueEditor from './CollectionValueEditor.svelte';

	interface Props {
		controller: CollectionViewController;
		snapshot: CollectionViewState;
		workspaceId: string;
	}

	let { controller, snapshot, workspaceId }: Props = $props();

	const ROW_HEIGHT = 48;
	const OPEN_COLUMN_WIDTH = 64;

	let scroller: HTMLDivElement | undefined = $state();
	let scrollTop = $state(0);
	let viewportHeight = $state(480);
	let active = $state<[number, number]>([0, 1]);
	let editing = $state<{ recordId: string; fieldId: string } | null>(null);
	let narrow = $state(false);
	let resizing: { fieldId: string; startX: number; startWidth: number } | null = null;

	const fields = $derived(snapshot.collection?.fields ?? []);
	const rows = $derived(snapshot.rows);
	const columnCount = $derived(fields.length + 1);
	const windowed = $derived(
		virtualWindow({
			scrollTop,
			viewportHeight,
			rowHeight: ROW_HEIGHT,
			rowCount: rows.length
		})
	);
	const visibleRows = $derived(rows.slice(windowed.start, windowed.end));

	onMount(() => {
		const query = window.matchMedia('(max-width: 767px)');
		narrow = query.matches;
		const onChange = (event: MediaQueryListEvent) => (narrow = event.matches);
		query.addEventListener('change', onChange);
		return () => query.removeEventListener('change', onChange);
	});

	$effect(() => {
		if (!scroller) return;
		const element = scroller;
		const observer = new ResizeObserver(() => (viewportHeight = element.clientHeight));
		observer.observe(element);
		viewportHeight = element.clientHeight;
		return () => observer.disconnect();
	});

	function width(field: CollectionField): number {
		return columnWidthIn(snapshot, field.fieldId) ?? COLUMN_WIDTH_DEFAULT;
	}

	const gridTemplate = $derived(
		`${OPEN_COLUMN_WIDTH}px ${fields.map((field) => `${width(field)}px`).join(' ')}`
	);

	function onScroll(): void {
		if (!scroller) return;
		scrollTop = scroller.scrollTop;
		if (
			snapshot.nextCursor &&
			!snapshot.loadingRecords &&
			scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - ROW_HEIGHT * 4
		) {
			void controller.loadMore();
		}
	}

	function cellId(rowIndex: number, column: number): string {
		return `collection-cell-${controller.collectionId}-${rowIndex}-${column}`;
	}

	function focusCell(rowIndex: number, column: number): void {
		active = [rowIndex, column];
		if (scroller) {
			const top = rowIndex * ROW_HEIGHT;
			if (top < scroller.scrollTop) scroller.scrollTop = top;
			else if (top + ROW_HEIGHT > scroller.scrollTop + scroller.clientHeight) {
				scroller.scrollTop = top + ROW_HEIGHT - scroller.clientHeight;
			}
			scrollTop = scroller.scrollTop;
		}
		queueMicrotask(() => document.getElementById(cellId(rowIndex, column))?.focus());
	}

	function startEdit(row: RecordRow, field: CollectionField): void {
		if (!isEditableField(field)) return;
		editing = { recordId: row.recordId, fieldId: field.fieldId };
	}

	function onCellKeydown(event: KeyboardEvent, rowIndex: number, column: number): void {
		if (editing) return;
		const row = rows[rowIndex];
		if (event.key === 'Enter' || event.key === 'F2') {
			event.preventDefault();
			if (!row) return;
			if (column === 0) {
				void controller.openPanel(row.recordId);
				return;
			}
			const field = fields[column - 1];
			if (field) startEdit(row, field);
			return;
		}
		const next = nextGridCell([rowIndex, column], event.key, rows.length, columnCount);
		if (next) {
			event.preventDefault();
			focusCell(next[0], next[1]);
		}
	}

	async function commit(recordId: string, fieldId: string): Promise<void> {
		const accepted = await controller.commitCell(recordId, fieldId);
		if (accepted) {
			editing = null;
			focusCell(active[0], active[1]);
		}
	}

	function cancel(recordId: string, fieldId: string): void {
		controller.cancelDraft(recordId, fieldId);
		editing = null;
		focusCell(active[0], active[1]);
	}

	function startResize(event: PointerEvent, field: CollectionField): void {
		event.preventDefault();
		resizing = { fieldId: field.fieldId, startX: event.clientX, startWidth: width(field) };
		(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
	}

	function moveResize(event: PointerEvent): void {
		if (!resizing) return;
		controller.setColumnWidth(
			resizing.fieldId,
			resizing.startWidth + event.clientX - resizing.startX
		);
	}

	function endResize(): void {
		resizing = null;
	}

	function resizeKey(event: KeyboardEvent, field: CollectionField): void {
		if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') {
			event.preventDefault();
			controller.setColumnWidth(
				field.fieldId,
				width(field) + (event.key === 'ArrowRight' ? 16 : -16)
			);
		}
	}

	function statusFor(recordId: string, fieldId: string) {
		return snapshot.cellStatus[`${recordId}:${fieldId}`] ?? null;
	}

	let creating = $state(false);
	let createError = $state<string | null>(null);

	async function createRecord(): Promise<void> {
		creating = true;
		createError = null;
		const outcome = await controller.createRecord();
		creating = false;
		if (!outcome.ok) createError = outcome.key;
	}
</script>

{#snippet cell(row: RecordRow, field: CollectionField)}
	{@const draft = draftIn(snapshot, row.recordId, field.fieldId)}
	{@const status = statusFor(row.recordId, field.fieldId)}
	{#if editing && editing.recordId === row.recordId && editing.fieldId === field.fieldId}
		<CollectionValueEditor
			{field}
			id={`collection-editor-${row.recordId}-${field.fieldId}`}
			value={editorValue(field, row.values, draft)}
			autofocus
			onchange={(value) => controller.setDraft(row.recordId, field.fieldId, value)}
			oncommit={() => void commit(row.recordId, field.fieldId)}
			oncancel={() => cancel(row.recordId, field.fieldId)}
		/>
	{:else if draft !== null}
		<span class="truncate text-amber-800 dark:text-amber-200" data-cell-draft="true">
			{Array.isArray(draft)
				? draft.join(', ')
				: typeof draft === 'boolean'
					? draft
						? $t('flow.collection.value.yes')
						: $t('flow.collection.value.no')
					: draft}
		</span>
	{:else}
		<CollectionCellValue {field} values={row.values} {workspaceId} />
	{/if}
	{#if status}
		<span
			class="ml-1 shrink-0 text-xs {status.state === 'saving'
				? 'text-slate-500'
				: 'text-red-600 dark:text-red-400'}"
			data-cell-status={status.state}
		>
			{status.state === 'saving'
				? $t('flow.collection.cell.saving')
				: status.state === 'conflict'
					? $t('flow.collection.cell.conflict')
					: $t(status.key)}
		</span>
	{/if}
{/snippet}

{#if narrow}
	<ul class="space-y-3 p-4" data-testid="collection-cards" aria-label={snapshot.collection?.title}>
		{#each rows as row (row.recordId)}
			<li
				class="rounded-lg border border-slate-200 p-3 dark:border-slate-700"
				data-testid="collection-card"
				data-record-id={row.recordId}
			>
				<dl class="space-y-1">
					{#each fields as field (field.fieldId)}
						<div class="flex min-w-0 gap-2 text-sm">
							<dt class="w-28 shrink-0 truncate text-slate-500 dark:text-slate-400">
								{field.label}
							</dt>
							<dd class="flex min-w-0 flex-1 items-center">{@render cell(row, field)}</dd>
						</div>
					{/each}
				</dl>
				<button
					type="button"
					class="mt-2 min-h-11 w-full rounded border border-slate-300 px-3 text-sm font-medium focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:border-slate-600"
					onclick={() => void controller.openPanel(row.recordId)}
				>
					{$t('flow.collection.table.openRecord')}
				</button>
			</li>
		{/each}
		{#if rows.length === 0 && !snapshot.loadingRecords}
			<li class="text-sm text-slate-500">{$t('flow.collection.table.empty')}</li>
		{/if}
		{#if snapshot.nextCursor}
			<li>
				<button
					type="button"
					class="min-h-11 w-full rounded border border-slate-300 px-3 text-sm dark:border-slate-600"
					disabled={snapshot.loadingRecords}
					onclick={() => void controller.loadMore()}>{$t('flow.collection.table.loadMore')}</button
				>
			</li>
		{/if}
	</ul>
{:else}
	<div
		bind:this={scroller}
		class="relative max-h-[65vh] min-h-48 overflow-auto"
		onscroll={onScroll}
		data-testid="collection-table-scroller"
	>
		<div
			role="grid"
			aria-label={snapshot.collection?.title}
			aria-rowcount={rows.length + 1}
			aria-colcount={columnCount}
			class="min-w-max text-sm"
			data-testid="collection-table"
		>
			<div
				role="row"
				aria-rowindex={1}
				class="sticky top-0 z-10 grid border-b border-slate-200 bg-slate-50 dark:border-slate-700 dark:bg-slate-900"
				style:grid-template-columns={gridTemplate}
			>
				<div role="columnheader" class="px-2 py-2 text-xs text-slate-500">
					<span class="sr-only">{$t('flow.collection.table.openColumn')}</span>
				</div>
				{#each fields as field (field.fieldId)}
					<div
						role="columnheader"
						class="relative flex items-center px-2 py-2 text-xs font-semibold text-slate-600 dark:text-slate-300"
						data-field-id={field.fieldId}
					>
						<span class="truncate">{field.label}</span>
						<button
							type="button"
							class="absolute top-0 right-0 h-full w-3 cursor-col-resize touch-none focus:outline-none focus-visible:bg-blue-500"
							aria-label={$t('flow.collection.table.resize', { values: { field: field.label } })}
							data-testid="collection-column-resize"
							onpointerdown={(event) => startResize(event, field)}
							onpointermove={moveResize}
							onpointerup={endResize}
							onpointercancel={endResize}
							onkeydown={(event) => resizeKey(event, field)}
						></button>
					</div>
				{/each}
			</div>
			<div style:height={`${windowed.padTop}px`} aria-hidden="true"></div>
			{#each visibleRows as row, offset (row.recordId)}
				{@const rowIndex = windowed.start + offset}
				<div
					role="row"
					aria-rowindex={rowIndex + 2}
					class="grid border-b border-slate-100 dark:border-slate-800"
					style:grid-template-columns={gridTemplate}
					style:height={`${ROW_HEIGHT}px`}
					data-testid="collection-row"
					data-record-id={row.recordId}
				>
					<div
						role="gridcell"
						id={cellId(rowIndex, 0)}
						tabindex={active[0] === rowIndex && active[1] === 0 ? 0 : -1}
						class="flex items-center justify-center focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500"
						onkeydown={(event) => onCellKeydown(event, rowIndex, 0)}
						onfocus={() => (active = [rowIndex, 0])}
					>
						<button
							type="button"
							tabindex="-1"
							class="min-h-11 min-w-11 rounded text-xs text-blue-600 hover:bg-blue-50 dark:text-blue-400 dark:hover:bg-slate-800"
							aria-label={$t('flow.collection.table.openRecord')}
							data-testid="collection-open-record"
							onclick={() => void controller.openPanel(row.recordId)}>↗</button
						>
					</div>
					{#each fields as field, fieldIndex (field.fieldId)}
						<div
							role="gridcell"
							id={cellId(rowIndex, fieldIndex + 1)}
							tabindex={active[0] === rowIndex && active[1] === fieldIndex + 1 ? 0 : -1}
							aria-readonly={!isEditableField(field)}
							class="flex min-w-0 items-center overflow-hidden px-2 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 focus-visible:ring-inset"
							data-field-id={field.fieldId}
							onkeydown={(event) => onCellKeydown(event, rowIndex, fieldIndex + 1)}
							onfocus={() => (active = [rowIndex, fieldIndex + 1])}
							ondblclick={() => {
								active = [rowIndex, fieldIndex + 1];
								startEdit(row, field);
							}}
						>
							{@render cell(row, field)}
						</div>
					{/each}
				</div>
			{/each}
			<div style:height={`${windowed.padBottom}px`} aria-hidden="true"></div>
		</div>
		{#if rows.length === 0 && !snapshot.loadingRecords}
			<p class="p-4 text-sm text-slate-500">{$t('flow.collection.table.empty')}</p>
		{/if}
	</div>
{/if}

<div
	class="flex flex-wrap items-center gap-3 border-t border-slate-200 px-4 py-2 dark:border-slate-700"
>
	<button
		type="button"
		class="min-h-11 rounded px-3 text-sm font-medium text-blue-700 hover:bg-blue-50 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 disabled:opacity-50 dark:text-blue-300 dark:hover:bg-slate-800"
		disabled={creating}
		data-testid="collection-new-record"
		onclick={() => void createRecord()}
	>
		+ {$t('flow.collection.table.newRecord')}
	</button>
	<span class="text-xs text-slate-500" aria-live="polite">
		{$t('flow.collection.table.count', { values: { loaded: rows.length } })}
	</span>
	{#if createError}
		<span class="text-xs text-red-600" role="alert">{$t(createError)}</span>
	{/if}
</div>
