<script lang="ts">
	// Filter / sort / board grouping of the active view. Changes here are an ephemeral draft
	// (`controller.setDraftQuery`: a query parameter, never a CRDT write); "Save view" writes the
	// draft into the view's config with `view_update` and exists only in schema mode.
	import { t } from 'svelte-i18n';
	import {
		CollectionViewController,
		activeViewIn,
		boardGroupFieldIn,
		effectiveQueryIn,
		type CollectionViewState
	} from '#lib/flow/collection-view.js';
	import {
		ENCODE_FAILURE_KEYS,
		emptyDraft,
		encodeFilterValue,
		filterOperators,
		type DraftValue
	} from '#lib/flow/collection-values.js';
	import CollectionValueEditor from './CollectionValueEditor.svelte';

	interface Props {
		controller: CollectionViewController;
		snapshot: CollectionViewState;
	}

	let { controller, snapshot }: Props = $props();

	const OPERATOR_KEYS: Readonly<Record<string, string>> = {
		eq: 'flow.collection.query.op.eq',
		ne: 'flow.collection.query.op.ne',
		lt: 'flow.collection.query.op.lt',
		lte: 'flow.collection.query.op.lte',
		gt: 'flow.collection.query.op.gt',
		gte: 'flow.collection.query.op.gte',
		contains: 'flow.collection.query.op.contains'
	};

	const view = $derived(activeViewIn(snapshot));
	const query = $derived(effectiveQueryIn(snapshot));
	const filterable = $derived(
		(snapshot.collection?.fields ?? []).filter(
			(field) => !field.restricted && field.type !== null && field.type !== 'relation'
		)
	);
	const selects = $derived(filterable.filter((field) => field.type === 'select'));
	const groupField = $derived(boardGroupFieldIn(snapshot));

	let filterFieldId = $state('');
	let filterOp = $state('eq');
	let filterValue = $state<DraftValue>('');
	let filterError = $state<string | null>(null);
	let saveMessage = $state<string | null>(null);

	const filterField = $derived(filterable.find((field) => field.fieldId === filterFieldId) ?? null);

	function pickFilterField(fieldId: string): void {
		filterFieldId = fieldId;
		const field = filterable.find((candidate) => candidate.fieldId === fieldId);
		filterOp = filterOperators(field?.type ?? null)[0] ?? 'eq';
		filterValue = field ? (field.type === 'multi_select' ? '' : emptyDraft(field)) : '';
		filterError = null;
	}

	async function applyFilter(): Promise<void> {
		if (!filterField) {
			await controller.setDraftQuery({ filter: null });
			return;
		}
		const encoded = encodeFilterValue(filterField, filterOp, filterValue);
		if (!encoded.ok) {
			filterError = ENCODE_FAILURE_KEYS[encoded.reason];
			return;
		}
		filterError = null;
		await controller.setDraftQuery({
			filter: { field_id: filterField.fieldId, op: filterOp, value: encoded.value }
		});
	}

	async function setSort(fieldId: string, direction: 'asc' | 'desc'): Promise<void> {
		await controller.setDraftQuery({
			sort: fieldId === '' ? null : { field_id: fieldId, direction }
		});
	}

	async function saveView(): Promise<void> {
		const outcome = await controller.saveActiveView();
		saveMessage = outcome.ok ? 'flow.collection.query.saved' : outcome.key;
	}

	const inputClass =
		'min-h-11 rounded border border-slate-300 bg-white px-2 text-sm text-slate-900 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-100';
</script>

<div
	class="flex flex-wrap items-end gap-3 border-b border-slate-200 px-4 py-2 text-xs text-slate-600 dark:border-slate-800 dark:text-slate-300"
	data-testid="collection-query-bar"
>
	<label class="flex flex-col gap-1">
		{$t('flow.collection.query.filterField')}
		<select
			class={inputClass}
			value={filterFieldId}
			onchange={(event) => pickFilterField(event.currentTarget.value)}
		>
			<option value="">{$t('flow.collection.query.noFilter')}</option>
			{#each filterable as field (field.fieldId)}
				<option value={field.fieldId}>{field.label}</option>
			{/each}
		</select>
	</label>
	{#if filterField}
		<label class="flex flex-col gap-1">
			{$t('flow.collection.query.operator')}
			<select class={inputClass} bind:value={filterOp}>
				{#each filterOperators(filterField.type) as op (op)}
					<option value={op}>{$t(OPERATOR_KEYS[op] ?? 'flow.collection.query.op.eq')}</option>
				{/each}
			</select>
		</label>
		<div class="flex min-w-40 flex-col gap-1">
			<span>{$t('flow.collection.query.value')}</span>
			{#if filterField.type === 'multi_select'}
				<select
					class={inputClass}
					value={typeof filterValue === 'string' ? filterValue : ''}
					aria-label={$t('flow.collection.query.value')}
					onchange={(event) => (filterValue = event.currentTarget.value)}
				>
					<option value="">{$t('flow.collection.value.none')}</option>
					{#each filterField.options as option (option)}
						<option value={option}>{option}</option>
					{/each}
				</select>
			{:else}
				<CollectionValueEditor
					field={filterField}
					id={`collection-filter-value-${controller.collectionId}`}
					value={filterValue}
					onchange={(value) => (filterValue = value)}
					oncommit={() => void applyFilter()}
				/>
			{/if}
		</div>
	{/if}
	<button
		type="button"
		class="min-h-11 rounded border border-slate-300 px-3 text-sm dark:border-slate-600"
		onclick={() => void applyFilter()}>{$t('flow.collection.query.applyFilter')}</button
	>
	{#if filterError}
		<span class="text-red-600" role="alert">{$t(filterError)}</span>
	{/if}

	<label class="flex flex-col gap-1">
		{$t('flow.collection.query.sortField')}
		<select
			class={inputClass}
			value={query.sort?.field_id ?? ''}
			onchange={(event) => void setSort(event.currentTarget.value, query.sort?.direction ?? 'asc')}
		>
			<option value="">{$t('flow.collection.query.noSort')}</option>
			{#each filterable as field (field.fieldId)}
				<option value={field.fieldId}>{field.label}</option>
			{/each}
		</select>
	</label>
	{#if query.sort}
		<label class="flex flex-col gap-1">
			{$t('flow.collection.query.direction')}
			<select
				class={inputClass}
				value={query.sort.direction}
				onchange={(event) =>
					void setSort(
						query.sort?.field_id ?? '',
						event.currentTarget.value === 'desc' ? 'desc' : 'asc'
					)}
			>
				<option value="asc">{$t('flow.collection.query.asc')}</option>
				<option value="desc">{$t('flow.collection.query.desc')}</option>
			</select>
		</label>
	{/if}

	{#if view?.kind === 'board' && selects.length > 0}
		<label class="flex flex-col gap-1">
			{$t('flow.collection.query.groupBy')}
			<select
				class={inputClass}
				value={groupField?.fieldId ?? ''}
				onchange={(event) =>
					void controller.setDraftQuery({ groupFieldId: event.currentTarget.value })}
			>
				{#each selects as field (field.fieldId)}
					<option value={field.fieldId}>{field.label}</option>
				{/each}
			</select>
		</label>
	{/if}

	{#if snapshot.draftQuery}
		<span class="text-amber-700 dark:text-amber-300" data-testid="collection-draft-query">
			{$t('flow.collection.query.draft')}
		</span>
		<button
			type="button"
			class="min-h-11 rounded px-3 text-sm hover:bg-slate-100 dark:hover:bg-slate-800"
			onclick={() => void controller.clearDraftQuery()}>{$t('flow.collection.query.reset')}</button
		>
	{/if}
	{#if snapshot.mode === 'schema' && view}
		<button
			type="button"
			class="min-h-11 rounded bg-blue-600 px-3 text-sm font-medium text-white hover:bg-blue-700 disabled:opacity-50"
			disabled={snapshot.schemaBusy}
			data-testid="collection-save-view"
			onclick={() => void saveView()}>{$t('flow.collection.query.saveView')}</button
		>
	{/if}
	{#if saveMessage}
		<span aria-live="polite">{$t(saveMessage)}</span>
	{/if}
</div>
