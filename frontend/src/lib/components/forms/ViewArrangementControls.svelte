<script lang="ts" module>
	export type SimpleFilterOperator = 'contains' | 'equals' | 'not_equals' | 'not_empty';
	export interface SimpleFilter {
		field: string;
		operator: SimpleFilterOperator;
		value: string;
		disabled?: boolean;
	}
	export interface SortChoice {
		field: string;
		direction: 'asc' | 'desc';
	}
</script>

<script lang="ts">
	/**
	 * Filter, Sort and Fields: the quick arrangement controls of the view bar. Every change
	 * applies to the list immediately. The filter editor edits the first filter group only; a view
	 * with more groups or an expression points to "Advanced filters…" instead.
	 */
	import { t } from 'svelte-i18n';
	import { ArrowDown, ArrowUp, ArrowUpDown, Columns2, ListFilter, Plus, X } from '@lucide/svelte';
	import type { FormField } from '#lib/api/forms.js';
	import Popover from './Popover.svelte';

	interface Props {
		fields: FormField[];
		sortableKeys: string[];
		filters: SimpleFilter[];
		filterLogic: 'all' | 'any';
		/** True when the view's filters cannot be edited here (several groups or an expression). */
		advanced: boolean;
		sort: SortChoice | null;
		columns: string[];
		hiddenByPermission: Set<string>;
		onfilters: (filters: SimpleFilter[], logic: 'all' | 'any') => void;
		onsort: (sort: SortChoice | null) => void;
		oncolumns: (columns: string[]) => void;
		onadvanced: () => void;
	}

	let {
		fields,
		sortableKeys,
		filters,
		filterLogic,
		advanced,
		sort,
		columns,
		hiddenByPermission,
		onfilters,
		onsort,
		oncolumns,
		onadvanced
	}: Props = $props();

	let open = $state<'filter' | 'sort' | 'fields' | null>(null);
	let filterButton = $state<HTMLButtonElement | null>(null);
	let sortButton = $state<HTMLButtonElement | null>(null);
	let fieldsButton = $state<HTMLButtonElement | null>(null);

	const activeFilters = $derived(filters.filter((filter) => !filter.disabled && filter.field));
	const fieldLabel = (field: FormField) => field.label?.trim() || field.key.replaceAll('_', ' ');
	const systemSortKeys = $derived([
		{ key: 'title', label: $t('forms.views.titleField') },
		{ key: 'created_at', label: $t('forms.views.createdField') },
		{ key: 'updated_at', label: $t('forms.views.updatedField') }
	]);
	const sortOptions = $derived([
		...systemSortKeys,
		...fields
			.filter((field) => sortableKeys.includes(field.key))
			.map((field) => ({ key: field.key, label: fieldLabel(field) }))
	]);
	const sortLabel = $derived(
		sort ? (sortOptions.find((option) => option.key === sort.field)?.label ?? sort.field) : ''
	);
	const orderedColumns = $derived(
		(() => {
			const shown = columns.filter((key) => fields.some((field) => field.key === key));
			const rest = fields.map((field) => field.key).filter((key) => !shown.includes(key));
			return [...shown, ...rest];
		})()
	);

	function toggle(name: 'filter' | 'sort' | 'fields') {
		open = open === name ? null : name;
	}

	function updateFilter(index: number, patch: Partial<SimpleFilter>) {
		onfilters(
			filters.map((filter, position) => (position === index ? { ...filter, ...patch } : filter)),
			filterLogic
		);
	}

	function addFilter() {
		onfilters(
			[...filters, { field: fields[0]?.key ?? '', operator: 'contains', value: '' }],
			filterLogic
		);
	}

	function removeFilter(index: number) {
		onfilters(
			filters.filter((_, position) => position !== index),
			filterLogic
		);
	}

	function setColumn(key: string, shown: boolean) {
		const next = shown
			? orderedColumns.filter((item) => item === key || columns.includes(item))
			: columns.filter((item) => item !== key);
		oncolumns(next);
	}

	function moveColumn(key: string, delta: -1 | 1) {
		const order = [...orderedColumns];
		const index = order.indexOf(key);
		const target = index + delta;
		if (index < 0 || target < 0 || target >= order.length) return;
		[order[index], order[target]] = [order[target], order[index]];
		oncolumns(order.filter((item) => columns.includes(item)));
	}

	function onColumnKey(event: KeyboardEvent, key: string) {
		if (!event.altKey || (event.key !== 'ArrowUp' && event.key !== 'ArrowDown')) return;
		event.preventDefault();
		moveColumn(key, event.key === 'ArrowUp' ? -1 : 1);
	}

	const toolButton =
		'inline-flex h-8 items-center gap-1.5 rounded-md px-2 text-sm text-fg hover:bg-slate-100 dark:hover:bg-slate-800';
	const control =
		'h-8 rounded-md border border-slate-300 bg-white px-2 text-sm dark:border-slate-600 dark:bg-slate-800';
</script>

<button
	type="button"
	bind:this={filterButton}
	class="{toolButton} {activeFilters.length > 0 ? 'text-blue-700 dark:text-blue-300' : ''}"
	aria-haspopup="dialog"
	aria-expanded={open === 'filter'}
	data-view-filter-button={activeFilters.length}
	onclick={() => toggle('filter')}
>
	<ListFilter class="h-4 w-4" aria-hidden="true" />
	{activeFilters.length > 0
		? $t('forms.views.filterCount', { values: { count: activeFilters.length } })
		: $t('forms.views.filter')}
</button>
<button
	type="button"
	bind:this={sortButton}
	class="{toolButton} {sort ? 'text-blue-700 dark:text-blue-300' : ''}"
	aria-haspopup="dialog"
	aria-expanded={open === 'sort'}
	data-view-sort-button={sort ? `${sort.field}:${sort.direction}` : ''}
	onclick={() => toggle('sort')}
>
	<ArrowUpDown class="h-4 w-4" aria-hidden="true" />
	{#if sort}
		<span class="max-w-[12rem] truncate">{sortLabel}</span>
		{#if sort.direction === 'desc'}<ArrowDown
				class="h-3.5 w-3.5"
				aria-label={$t('forms.views.descending')}
			/>{:else}<ArrowUp class="h-3.5 w-3.5" aria-label={$t('forms.views.ascending')} />{/if}
	{:else}
		{$t('forms.views.sort')}
	{/if}
</button>
<button
	type="button"
	bind:this={fieldsButton}
	class={toolButton}
	aria-haspopup="dialog"
	aria-expanded={open === 'fields'}
	data-view-fields-button
	onclick={() => toggle('fields')}
>
	<Columns2 class="h-4 w-4" aria-hidden="true" />
	{$t('forms.views.fields')}
</button>

<Popover
	open={open === 'filter'}
	anchor={filterButton}
	onclose={() => (open = null)}
	label={$t('forms.views.filter')}
	width={420}
	testId="view-filter"
>
	<div class="space-y-2 p-1">
		{#if advanced}
			<p class="text-sm text-fg-muted">
				{$t('forms.views.advancedFiltersInUse')}
			</p>
		{:else}
			{#if filters.length >= 2}
				<div class="flex gap-1" role="radiogroup" aria-label={$t('forms.filterConditions')}>
					{#each [['all', $t('forms.views.matchAll')], ['any', $t('forms.views.matchAny')]] as [value, label] (value)}
						<button
							type="button"
							role="radio"
							aria-checked={filterLogic === value}
							class="h-7 rounded-md px-2 text-xs font-medium {filterLogic === value
								? 'bg-blue-600 text-white'
								: 'bg-slate-100 text-fg dark:bg-slate-800'}"
							onclick={() => onfilters(filters, value as 'all' | 'any')}>{label}</button
						>
					{/each}
				</div>
			{/if}
			{#if filters.length === 0}
				<p class="text-sm text-fg-muted">{$t('forms.views.noFilters')}</p>
			{/if}
			{#each filters as filter, index (index)}
				<div class="flex flex-wrap items-center gap-1.5" data-view-filter-row={index}>
					<select
						class="{control} min-w-0 flex-1"
						aria-label={$t('forms.filterField')}
						value={filter.field}
						onchange={(event) => updateFilter(index, { field: event.currentTarget.value })}
					>
						{#each fields as field (field.key)}
							<option value={field.key}>{fieldLabel(field)}</option>
						{/each}
					</select>
					<select
						class={control}
						aria-label={$t('forms.filterOperator')}
						value={filter.operator}
						onchange={(event) =>
							updateFilter(index, { operator: event.currentTarget.value as SimpleFilterOperator })}
					>
						<option value="contains">{$t('forms.filterContains')}</option>
						<option value="equals">{$t('forms.filterEquals')}</option>
						<option value="not_equals">{$t('forms.filterNotEquals')}</option>
						<option value="not_empty">{$t('forms.filterNotEmpty')}</option>
					</select>
					{#if filter.operator !== 'not_empty'}
						<input
							class="{control} min-w-0 flex-1"
							aria-label={$t('forms.filterValue')}
							value={filter.value}
							oninput={(event) => updateFilter(index, { value: event.currentTarget.value })}
						/>
					{/if}
					<button
						type="button"
						class="inline-flex h-8 w-8 items-center justify-center rounded-md text-slate-500 hover:bg-slate-100 dark:hover:bg-slate-800"
						aria-label={$t('forms.views.removeCondition')}
						onclick={() => removeFilter(index)}><X class="h-4 w-4" aria-hidden="true" /></button
					>
				</div>
			{/each}
			<button
				type="button"
				class="inline-flex h-8 items-center gap-1 rounded-md px-2 text-sm text-blue-700 hover:bg-blue-50 disabled:opacity-50 dark:text-blue-300 dark:hover:bg-blue-950/40"
				disabled={fields.length === 0}
				data-view-filter-add
				onclick={addFilter}
				><Plus class="h-4 w-4" aria-hidden="true" />{$t('forms.views.addCondition')}</button
			>
		{/if}
		<div class="border-t border-line pt-2">
			<button
				type="button"
				class="h-8 rounded-md px-2 text-sm text-fg hover:bg-slate-100 dark:hover:bg-slate-800"
				data-view-filter-advanced
				onclick={() => {
					open = null;
					onadvanced();
				}}>{$t('forms.views.advancedFilters')}</button
			>
		</div>
	</div>
</Popover>

<Popover
	open={open === 'sort'}
	anchor={sortButton}
	onclose={() => (open = null)}
	label={$t('forms.views.sort')}
	width={300}
	testId="view-sort"
>
	<div class="space-y-2 p-1">
		<label class="block space-y-1">
			<span class="text-xs font-medium text-fg-muted"
				>{$t('forms.views.sortField')}</span
			>
			<select
				class="{control} w-full"
				value={sort?.field ?? ''}
				data-view-sort-field
				onchange={(event) => {
					const field = event.currentTarget.value;
					onsort(field ? { field, direction: sort?.direction ?? 'asc' } : null);
				}}
			>
				<option value="">{$t('forms.views.sortNone')}</option>
				{#each sortOptions as option (option.key)}
					<option value={option.key}>{option.label}</option>
				{/each}
			</select>
		</label>
		<div class="flex gap-1" role="radiogroup" aria-label={$t('forms.views.sort')}>
			{#each [['asc', $t('forms.views.ascending')], ['desc', $t('forms.views.descending')]] as [value, label] (value)}
				<button
					type="button"
					role="radio"
					aria-checked={sort?.direction === value}
					disabled={!sort}
					class="h-8 flex-1 rounded-md px-2 text-sm font-medium disabled:opacity-50 {sort?.direction ===
					value
						? 'bg-blue-600 text-white'
						: 'bg-slate-100 text-fg dark:bg-slate-800'}"
					data-view-sort-direction={value}
					onclick={() => sort && onsort({ field: sort.field, direction: value as 'asc' | 'desc' })}
					>{label}</button
				>
			{/each}
		</div>
		{#if sort}
			<button
				type="button"
				class="h-8 rounded-md px-2 text-sm text-fg hover:bg-slate-100 dark:hover:bg-slate-800"
				onclick={() => onsort(null)}>{$t('forms.views.clear')}</button
			>
		{/if}
	</div>
</Popover>

<Popover
	open={open === 'fields'}
	anchor={fieldsButton}
	onclose={() => (open = null)}
	label={$t('forms.views.fields')}
	width={300}
	testId="view-fields"
>
	<div class="space-y-1 p-1">
		<div class="flex gap-1 pb-1">
			<button
				type="button"
				class="h-7 rounded-md px-2 text-xs text-fg hover:bg-slate-100 dark:hover:bg-slate-800"
				onclick={() => oncolumns(orderedColumns.filter((key) => !hiddenByPermission.has(key)))}
				>{$t('forms.views.showAll')}</button
			>
		</div>
		<ul class="space-y-0.5">
			{#each orderedColumns as key, index (key)}
				{@const field = fields.find((item) => item.key === key)}
				{#if field}
					{@const blocked = hiddenByPermission.has(key)}
					<li
						class="flex min-h-8 items-center gap-2 rounded-md px-1 hover:bg-slate-50 dark:hover:bg-slate-800/60"
					>
						<input
							type="checkbox"
							id={`view-column-${key}`}
							class="h-4 w-4 rounded border-slate-300"
							checked={columns.includes(key)}
							disabled={blocked || (columns.length === 1 && columns.includes(key))}
							onchange={(event) => setColumn(key, event.currentTarget.checked)}
							onkeydown={(event) => onColumnKey(event, key)}
							data-view-column-toggle={key}
						/>
						<label for={`view-column-${key}`} class="min-w-0 flex-1 truncate text-sm">
							{fieldLabel(field)}
							{#if blocked}
								<span class="block text-xs text-slate-500"
									>{$t('forms.views.hiddenByPermission')}</span
								>
							{/if}
						</label>
						<button
							type="button"
							class="inline-flex h-6 w-6 items-center justify-center rounded text-slate-500 hover:bg-slate-100 disabled:opacity-30 dark:hover:bg-slate-800"
							aria-label={$t('forms.views.moveUp', { values: { field: fieldLabel(field) } })}
							disabled={index === 0 || !columns.includes(key)}
							onclick={() => moveColumn(key, -1)}
							><ArrowUp class="h-3.5 w-3.5" aria-hidden="true" /></button
						>
						<button
							type="button"
							class="inline-flex h-6 w-6 items-center justify-center rounded text-slate-500 hover:bg-slate-100 disabled:opacity-30 dark:hover:bg-slate-800"
							aria-label={$t('forms.views.moveDown', { values: { field: fieldLabel(field) } })}
							disabled={index === orderedColumns.length - 1 || !columns.includes(key)}
							onclick={() => moveColumn(key, 1)}
							><ArrowDown class="h-3.5 w-3.5" aria-hidden="true" /></button
						>
					</li>
				{/if}
			{/each}
		</ul>
	</div>
</Popover>
