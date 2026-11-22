<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import { ArrowLeftRight } from '@lucide/svelte';
	import { fieldLabel } from '#lib/forms/field-options.js';
	import { selectValue } from '#lib/forms/record-format.js';
	import {
		viewPivotAggregate,
		viewPivotTotalsMode,
		viewPivotValueDisplay,
		viewPivotSortMode,
		viewPivotRowLimit,
		viewPivotColumnLimit
	} from '#lib/forms/view-config.js';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const {
		viewDraftPivotDimensionFieldCandidates,
		viewDraftPivotValueFieldCandidates,
		swapViewDraftPivotAxes
	} = fp.viewDraftFields;
</script>

<label class="space-y-1">
	<span class="text-xs font-medium uppercase text-fg-muted">{$t('forms.pivotRowField')}</span>

	<select
		value={s.viewDraft.pivotRowField}
		onchange={(event) => {
			const pivotRowField = selectValue(event);

			s.viewDraft = {
				...s.viewDraft,
				pivotRowField,
				pivotColumnField:
					s.viewDraft.pivotColumnField === pivotRowField ? '' : s.viewDraft.pivotColumnField
			};
		}}
		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
	>
		<option value="">{$t('forms.viewNoRule')}</option>

		{#each viewDraftPivotDimensionFieldCandidates() as field (field.key)}
			<option value={field.key}>{fieldLabel(field)}</option>
		{/each}
	</select>
</label>

<label class="space-y-1">
	<span class="text-xs font-medium uppercase text-fg-muted">{$t('forms.pivotColumnField')}</span>

	<select
		value={s.viewDraft.pivotColumnField}
		onchange={(event) =>
			(s.viewDraft = {
				...s.viewDraft,
				pivotColumnField: selectValue(event)
			})}
		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
	>
		<option value="">{$t('forms.viewNoRule')}</option>

		{#each viewDraftPivotDimensionFieldCandidates().filter((field) => field.key !== s.viewDraft.pivotRowField) as field (field.key)}
			<option value={field.key}>{fieldLabel(field)}</option>
		{/each}
	</select>
</label>

<label class="space-y-1">
	<span class="text-xs font-medium uppercase text-fg-muted">{$t('forms.pivotValueField')}</span>

	<select
		value={s.viewDraft.pivotValueField}
		onchange={(event) =>
			(s.viewDraft = {
				...s.viewDraft,
				pivotValueField: selectValue(event)
			})}
		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
	>
		<option value="">{$t('forms.viewNoRule')}</option>

		{#each viewDraftPivotValueFieldCandidates() as field (field.key)}
			<option value={field.key}>{fieldLabel(field)}</option>
		{/each}
	</select>
</label>

<label class="space-y-1">
	<span class="text-xs font-medium uppercase text-fg-muted">{$t('forms.pivotAggregate')}</span>

	<select
		value={s.viewDraft.pivotAggregate}
		onchange={(event) =>
			(s.viewDraft = {
				...s.viewDraft,
				pivotAggregate: viewPivotAggregate(selectValue(event))
			})}
		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
	>
		<option value="sum">{$t('forms.pivotAggregateSum')}</option>
		<option value="count">{$t('forms.pivotAggregateCount')}</option>
		<option value="avg">{$t('forms.pivotAggregateAvg')}</option>
	</select>
</label>

<label class="space-y-1">
	<span class="text-xs font-medium uppercase text-fg-muted">{$t('forms.pivotTotals')}</span>

	<select
		value={s.viewDraft.pivotTotals}
		onchange={(event) =>
			(s.viewDraft = {
				...s.viewDraft,
				pivotTotals: viewPivotTotalsMode(selectValue(event))
			})}
		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
	>
		<option value="all">{$t('forms.pivotTotalsAll')}</option>
		<option value="rows">{$t('forms.pivotTotalsRows')}</option>
		<option value="columns">{$t('forms.pivotTotalsColumns')}</option>
		<option value="none">{$t('forms.pivotTotalsNone')}</option>
	</select>
</label>

<label class="space-y-1">
	<span class="text-xs font-medium uppercase text-fg-muted">{$t('forms.pivotValueDisplay')}</span>

	<select
		value={s.viewDraft.pivotValueDisplay}
		onchange={(event) =>
			(s.viewDraft = {
				...s.viewDraft,
				pivotValueDisplay: viewPivotValueDisplay(selectValue(event))
			})}
		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
	>
		<option value="value">{$t('forms.pivotValueDisplayValue')}</option>
		<option value="percent_row">{$t('forms.pivotValueDisplayPercentRow')}</option>
		<option value="percent_column">{$t('forms.pivotValueDisplayPercentColumn')}</option>
		<option value="percent_total">{$t('forms.pivotValueDisplayPercentTotal')}</option>
	</select>
</label>

<label class="space-y-1">
	<span class="text-xs font-medium uppercase text-fg-muted">{$t('forms.pivotSort')}</span>

	<select
		value={s.viewDraft.pivotSort}
		onchange={(event) =>
			(s.viewDraft = {
				...s.viewDraft,
				pivotSort: viewPivotSortMode(selectValue(event))
			})}
		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
	>
		<option value="label">{$t('forms.pivotSortLabel')}</option>
		<option value="row_total_desc">{$t('forms.pivotSortRowTotalDesc')}</option>
		<option value="row_total_asc">{$t('forms.pivotSortRowTotalAsc')}</option>
		<option value="column_total_desc">{$t('forms.pivotSortColumnTotalDesc')}</option>
		<option value="column_total_asc">{$t('forms.pivotSortColumnTotalAsc')}</option>
	</select>
</label>

<label class="space-y-1">
	<span class="text-xs font-medium uppercase text-fg-muted">{$t('forms.pivotRowLimit')}</span>

	<select
		value={String(s.viewDraft.pivotRowLimit)}
		onchange={(event) =>
			(s.viewDraft = {
				...s.viewDraft,
				pivotRowLimit: viewPivotRowLimit(selectValue(event))
			})}
		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
	>
		<option value="0">{$t('forms.pivotRowLimitAll')}</option>
		<option value="2">{$t('forms.pivotRowLimitTwo')}</option>
		<option value="5">{$t('forms.pivotRowLimitFive')}</option>
		<option value="10">{$t('forms.pivotRowLimitTen')}</option>
	</select>
</label>

<label class="space-y-1">
	<span class="text-xs font-medium uppercase text-fg-muted">{$t('forms.pivotColumnLimit')}</span>

	<select
		value={String(s.viewDraft.pivotColumnLimit)}
		onchange={(event) =>
			(s.viewDraft = {
				...s.viewDraft,
				pivotColumnLimit: viewPivotColumnLimit(selectValue(event))
			})}
		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
	>
		<option value="0">{$t('forms.pivotColumnLimitAll')}</option>
		<option value="2">{$t('forms.pivotColumnLimitTwo')}</option>
		<option value="5">{$t('forms.pivotColumnLimitFive')}</option>
		<option value="10">{$t('forms.pivotColumnLimitTen')}</option>
	</select>
</label>

<label
	class="flex items-center gap-2 rounded-control border border-line bg-nav px-3 py-2 text-sm text-fg"
>
	<input
		type="checkbox"
		class="h-4 w-4 rounded border-input accent-primary"
		checked={s.viewDraft.pivotEmptyBuckets}
		data-pivot-empty-buckets-control
		onchange={(event) =>
			(s.viewDraft = {
				...s.viewDraft,
				pivotEmptyBuckets:
					event.currentTarget instanceof HTMLInputElement ? event.currentTarget.checked : false
			})}
	/>

	<span class="min-w-0 truncate">{$t('forms.pivotEmptyBuckets')}</span>
</label>

<label
	class="flex items-center gap-2 rounded-control border border-line bg-nav px-3 py-2 text-sm text-fg"
>
	<input
		type="checkbox"
		class="h-4 w-4 rounded border-input accent-primary"
		checked={s.viewDraft.pivotHideZeroBuckets}
		data-pivot-hide-zero-buckets-control
		onchange={(event) =>
			(s.viewDraft = {
				...s.viewDraft,
				pivotHideZeroBuckets:
					event.currentTarget instanceof HTMLInputElement ? event.currentTarget.checked : false
			})}
	/>

	<span class="min-w-0 truncate">{$t('forms.pivotHideZeroBuckets')}</span>
</label>

<label
	class="flex items-center gap-2 rounded-control border border-line bg-nav px-3 py-2 text-sm text-fg"
>
	<input
		type="checkbox"
		class="h-4 w-4 rounded border-input accent-primary"
		checked={s.viewDraft.pivotShowCounts}
		data-pivot-show-counts-control
		onchange={(event) =>
			(s.viewDraft = {
				...s.viewDraft,
				pivotShowCounts:
					event.currentTarget instanceof HTMLInputElement ? event.currentTarget.checked : false
			})}
	/>

	<span class="min-w-0 truncate">{$t('forms.pivotShowCounts')}</span>
</label>

<label
	class="flex items-center gap-2 rounded-control border border-line bg-nav px-3 py-2 text-sm text-fg"
>
	<input
		type="checkbox"
		class="h-4 w-4 rounded border-input accent-primary"
		checked={s.viewDraft.pivotHeatmap}
		data-pivot-heatmap-control
		onchange={(event) =>
			(s.viewDraft = {
				...s.viewDraft,
				pivotHeatmap:
					event.currentTarget instanceof HTMLInputElement ? event.currentTarget.checked : false
			})}
	/>

	<span class="min-w-0 truncate">{$t('forms.pivotHeatmap')}</span>
</label>

<div class="flex items-end">
	<button
		type="button"
		disabled={!s.viewDraft.pivotRowField || !s.viewDraft.pivotColumnField}
		class="inline-flex h-10 items-center gap-2 rounded-control border border-input bg-raised px-3 text-sm font-medium text-fg hover:bg-surface disabled:cursor-not-allowed disabled:bg-surface disabled:text-fg-muted"
		data-pivot-swap-axes
		onclick={swapViewDraftPivotAxes}
	>
		<ArrowLeftRight class="h-4 w-4" aria-hidden="true" />
		{$t('forms.pivotSwapAxes')}
	</button>
</div>
