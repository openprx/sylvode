<script lang="ts">
	// `collection:board` renderer: one column per option of the grouping `select` field
	// (`boardGroupFieldIn`). Dropping a card on another column is one `record_patch` of that field
	// (`controller.moveCard`); every card also has keyboard-operable "move to previous / next
	// column" buttons as the drag alternative. Each column is a `role="list"`. Without a `select`
	// field the board is disabled with a hint.
	import { t } from 'svelte-i18n';
	import {
		CollectionViewController,
		boardColumns,
		boardGroupFieldIn,
		type BoardColumn,
		type CollectionViewState
	} from '#lib/flow/collection-view.js';
	import CollectionCellValue from './CollectionCellValue.svelte';

	interface Props {
		controller: CollectionViewController;
		snapshot: CollectionViewState;
		scopeId: string;
	}

	let { controller, snapshot, scopeId }: Props = $props();

	const groupField = $derived(boardGroupFieldIn(snapshot));
	const columns = $derived(groupField ? boardColumns(snapshot.rows, groupField) : []);
	const cardFields = $derived(
		(snapshot.collection?.fields ?? [])
			.filter((field) => field.fieldId !== groupField?.fieldId)
			.slice(0, 3)
	);

	let dragging = $state<string | null>(null);
	let dropTarget = $state<number | null>(null);
	let creatingIn = $state<number | null>(null);
	let errorKey = $state<string | null>(null);

	function columnLabel(column: BoardColumn): string {
		return column.value ?? $t('flow.collection.board.noValue');
	}

	async function move(recordId: string, toIndex: number): Promise<void> {
		const column = columns[toIndex];
		if (!column) return;
		errorKey = null;
		const accepted = await controller.moveCard(recordId, column.value);
		if (!accepted) {
			const field = groupField;
			const status = field ? snapshot.cellStatus[`${recordId}:${field.fieldId}`] : undefined;
			errorKey =
				status?.state === 'error'
					? status.key
					: status?.state === 'conflict'
						? 'flow.collection.cell.conflict'
						: 'flow.collection.board.moveFailed';
		}
	}

	function onDrop(event: DragEvent, toIndex: number): void {
		event.preventDefault();
		const recordId = event.dataTransfer?.getData('text/plain') || dragging;
		dragging = null;
		dropTarget = null;
		if (recordId) void move(recordId, toIndex);
	}

	async function createIn(index: number): Promise<void> {
		const column = columns[index];
		if (!column || !groupField) return;
		creatingIn = index;
		errorKey = null;
		const outcome = await controller.createRecord(
			column.value === null ? {} : { [groupField.fieldId]: column.value }
		);
		creatingIn = null;
		if (!outcome.ok) errorKey = outcome.key;
	}
</script>

{#if !groupField}
	<div class="p-6 text-sm text-fg-muted" data-testid="collection-board-disabled" role="note">
		{$t('flow.collection.board.needsSelect')}
	</div>
{:else}
	{#if errorKey}
		<p class="px-4 pt-2 text-sm text-danger" role="alert">{$t(errorKey)}</p>
	{/if}
	<div
		class="flex gap-3 overflow-x-auto p-4"
		data-testid="collection-board"
		data-group-field-id={groupField.fieldId}
	>
		{#each columns as column, index (column.value ?? '')}
			<section
				class="flex w-72 shrink-0 flex-col rounded-card border bg-surface {dropTarget === index
					? 'border-fg'
					: 'border-line'}"
				aria-label={columnLabel(column)}
				data-testid="collection-board-column"
				data-option={column.value ?? ''}
				ondragover={(event) => {
					event.preventDefault();
					dropTarget = index;
				}}
				ondragleave={() => {
					if (dropTarget === index) dropTarget = null;
				}}
				ondrop={(event) => onDrop(event, index)}
			>
				<header class="flex items-center justify-between gap-2 px-3 py-2">
					<h3 class="truncate text-sm font-semibold text-fg">
						{columnLabel(column)}
						<span class="ml-1 text-xs font-normal text-fg-muted">{column.rows.length}</span>
					</h3>
					<button
						type="button"
						class="min-h-11 min-w-11 rounded text-lg text-fg hover:bg-surface disabled:text-control disabled:hover:bg-transparent"
						aria-label={$t('flow.collection.board.newInColumn', {
							values: { column: columnLabel(column) }
						})}
						disabled={creatingIn !== null}
						data-testid="collection-board-new"
						onclick={() => void createIn(index)}>+</button
					>
				</header>
				<ul
					role="list"
					class="flex min-h-16 flex-col gap-2 px-3 pb-3"
					aria-label={columnLabel(column)}
				>
					{#each column.rows as row (row.recordId)}
						<li
							class="rounded-card border border-line bg-raised p-2 {dragging === row.recordId
								? 'opacity-50'
								: ''}"
							draggable="true"
							data-testid="collection-board-card"
							data-record-id={row.recordId}
							ondragstart={(event) => {
								dragging = row.recordId;
								event.dataTransfer?.setData('text/plain', row.recordId);
								if (event.dataTransfer) event.dataTransfer.effectAllowed = 'move';
							}}
							ondragend={() => {
								dragging = null;
								dropTarget = null;
							}}
						>
							<!-- The card body is the drag source: Chromium starts no drag from a <button>. -->
							<div
								class="flex min-h-11 cursor-grab flex-col gap-0.5 text-sm"
								data-testid="collection-board-card-body"
							>
								{#each cardFields as field (field.fieldId)}
									<span class="flex min-w-0 gap-1">
										<span class="sr-only">{field.label}:</span>
										<CollectionCellValue {field} values={row.values} {scopeId} />
									</span>
								{:else}
									<span>{$t('flow.collection.record.untitled')}</span>
								{/each}
							</div>
							<div class="mt-1 flex items-center justify-end gap-1">
								<button
									type="button"
									class="mr-auto min-h-11 rounded px-2 text-sm text-fg hover:bg-surface"
									data-testid="collection-board-open"
									onclick={() => void controller.openPanel(row.recordId)}
									>{$t('flow.collection.table.openRecord')}</button
								>
								<button
									type="button"
									class="min-h-11 min-w-11 rounded text-sm hover:bg-surface disabled:text-control disabled:hover:bg-transparent"
									aria-label={$t('flow.collection.board.moveLeft')}
									disabled={index === 0}
									data-testid="collection-board-move-left"
									onclick={() => void move(row.recordId, index - 1)}>←</button
								>
								<button
									type="button"
									class="min-h-11 min-w-11 rounded text-sm hover:bg-surface disabled:text-control disabled:hover:bg-transparent"
									aria-label={$t('flow.collection.board.moveRight')}
									disabled={index === columns.length - 1}
									data-testid="collection-board-move-right"
									onclick={() => void move(row.recordId, index + 1)}>→</button
								>
							</div>
						</li>
					{/each}
				</ul>
			</section>
		{/each}
	</div>
	{#if snapshot.nextCursor}
		<div class="px-4 pb-4">
			<button
				type="button"
				class="min-h-11 rounded border border-fg px-3 text-sm bg-raised"
				disabled={snapshot.loadingRecords}
				onclick={() => void controller.loadMore()}>{$t('flow.collection.table.loadMore')}</button
			>
		</div>
	{/if}
{/if}
