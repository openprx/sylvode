<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import {
		ArrowDown,
		ArrowLeftRight,
		ArrowUp,
		Copy,
		Eye,
		EyeOff,
		FileDown,
		FileText,
		Link,
		Plus,
		TableProperties,
		X
	} from '@lucide/svelte';
	import Button from '#lib/components/Button.svelte';
	import { fieldLabel } from '#lib/forms/field-options.js';
	import type { ViewFilterLogic, ViewFilterOperator } from '#lib/forms/page-types.js';
	import { inputValue, selectValue } from '#lib/forms/record-format.js';
	import { getFormsPageContext } from '#lib/forms/state/context.js';
	import {
		viewDraftFilterExpressionRecordIdsText,
		filterLabelSuffix
	} from '#lib/forms/view-draft-parse.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const {
		updateViewDraftFilterRootLogic,
		updateViewDraftFilterGroupLogic,
		toggleViewDraftFilterGroupDisabled,
		addViewDraftFilterGroup,
		removeViewDraftFilterGroup,
		canDuplicateViewDraftFilterGroup,
		duplicateViewDraftFilterGroup,
		canMergeViewDraftFilterGroup,
		mergeViewDraftFilterGroupWithPrevious,
		moveViewDraftFilterGroup,
		visibleViewDraftFilterGroups,
		hasReachedFilterLimit,
		updateViewDraftFilter,
		toggleViewDraftFilterDisabled,
		addViewDraftFilter,
		removeViewDraftFilter,
		duplicateViewDraftFilter,
		canSplitViewDraftFilterToGroup,
		splitViewDraftFilterToGroup,
		moveViewDraftFilter
	} = fp.viewDraftFilters;
	const {
		viewDraftFilterMatchCount,
		viewDraftFilterGroupMatchCount,
		viewDraftFilterExpressionRecords,
		viewDraftFilterExpressionJsonText,
		viewDraftFilterExpressionSummaryText,
		copyViewDraftFilterExpressionRecordIds,
		copyViewDraftFilterExpressionJson,
		copyViewDraftFilterExpressionSummary,
		copyViewDraftFilterExpressionJsonRecords,
		copyViewDraftFilterExpressionCsvRecords,
		exportViewDraftFilterExpressionConfigJson,
		exportViewDraftFilterExpressionSummary,
		focusViewDraftFilterExpressionRecords,
		exportViewDraftFilterExpressionCsv,
		exportViewDraftFilterExpressionJson
	} = fp.filterExpressionExport;
</script>

<div class="space-y-2">
	<div class="flex items-center justify-between gap-3">
		<h5 class="text-xs font-semibold uppercase text-fg-muted">
			{$t('forms.filterConditions')}
		</h5>

		<div class="flex flex-wrap gap-2">
			<Button
				size="sm"
				variant="secondary"
				onclick={() => addViewDraftFilter(0)}
				disabled={hasReachedFilterLimit()}
			>
				<Plus class="mr-2 h-4 w-4" aria-hidden="true" />
				{$t('forms.addFilter')}
			</Button>

			<Button
				size="sm"
				variant="secondary"
				onclick={addViewDraftFilterGroup}
				disabled={s.viewDraft.filterGroups.length >= 5}
			>
				<Plus class="mr-2 h-4 w-4" aria-hidden="true" />
				{$t('forms.addFilterGroup')}
			</Button>
		</div>
	</div>

	{#if visibleViewDraftFilterGroups().every((group) => group.filters.length === 0)}
		<p
			class="rounded-card border border-dashed border-input bg-raised px-3 py-2 text-xs text-fg-muted"
		>
			{$t('forms.noFilterConditions')}
		</p>
	{/if}

	{#if visibleViewDraftFilterGroups().some((group) => group.filters.length > 0)}
		{@const expressionMatchingRecords = viewDraftFilterExpressionRecords()}
		{@const expressionMatchCount = expressionMatchingRecords?.length ?? null}
		{@const expressionRecordIdsText =
			viewDraftFilterExpressionRecordIdsText(expressionMatchingRecords)}
		{@const expressionJsonText = viewDraftFilterExpressionJsonText()}
		{@const expressionSummaryText = viewDraftFilterExpressionSummaryText()}

		<div class="flex flex-wrap items-end gap-2">
			<label class="block max-w-xs flex-1 space-y-1">
				<span class="text-xs font-medium uppercase text-fg-muted"
					>{$t('forms.filterGroupLogic')}</span
				>

				<select
					value={s.viewDraft.filterRootLogic}
					onchange={(event) =>
						updateViewDraftFilterRootLogic(selectValue(event) as ViewFilterLogic)}
					class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
				>
					<option value="all">{$t('forms.filterGroupsMatchAll')}</option>
					<option value="any">{$t('forms.filterGroupsMatchAny')}</option>
				</select>
			</label>

			<span
				class="rounded-control border border-line bg-surface px-2.5 py-2 text-xs font-semibold text-fg-muted"
				data-filter-expression-match-count
				data-record-count={expressionMatchCount ?? ''}
			>
				{expressionMatchCount === null
					? $t('forms.filterMatchCountPending')
					: $t('forms.filterExpressionMatchCount', {
							values: { count: expressionMatchCount }
						})}
			</span>

			<button
				type="button"
				class="inline-flex items-center gap-1 rounded-control border border-input bg-raised px-2.5 py-2 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
				onclick={() => copyViewDraftFilterExpressionRecordIds(expressionMatchingRecords)}
				disabled={!expressionRecordIdsText}
				aria-label={$t('forms.copyFilterExpressionRecordIds')}
				data-filter-expression-copy-ids
				data-filter-expression-copied={s.copiedFilterExpressionRecordIds === expressionRecordIdsText
					? 'true'
					: undefined}
			>
				<Copy class="h-3.5 w-3.5" aria-hidden="true" />
				{$t('forms.copyFilterExpressionRecordIds')}
			</button>

			<button
				type="button"
				class="inline-flex items-center gap-1 rounded-control border border-input bg-raised px-2.5 py-2 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
				onclick={() => copyViewDraftFilterExpressionJson(expressionJsonText)}
				disabled={!expressionJsonText}
				aria-label={$t('forms.copyFilterExpressionJson')}
				data-filter-expression-copy-json
				data-filter-expression-json-copied={s.copiedFilterExpressionJson === expressionJsonText
					? 'true'
					: undefined}
			>
				<FileText class="h-3.5 w-3.5" aria-hidden="true" />
				{$t('forms.copyFilterExpressionJson')}
			</button>

			<button
				type="button"
				class="inline-flex items-center gap-1 rounded-control border border-input bg-raised px-2.5 py-2 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
				onclick={() => exportViewDraftFilterExpressionConfigJson(expressionJsonText)}
				disabled={!expressionJsonText}
				aria-label={$t('forms.exportFilterExpressionJson')}
				data-filter-expression-export-config-json
				data-filter-expression-config-json-exported={s.exportedFilterExpressionJson ===
				expressionJsonText
					? 'true'
					: undefined}
			>
				<FileDown class="h-3.5 w-3.5" aria-hidden="true" />
				{$t('forms.exportFilterExpressionJson')}
			</button>

			<button
				type="button"
				class="inline-flex items-center gap-1 rounded-control border border-input bg-raised px-2.5 py-2 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
				onclick={() => copyViewDraftFilterExpressionSummary(expressionSummaryText)}
				disabled={!expressionSummaryText}
				aria-label={$t('forms.copyFilterExpressionSummary')}
				data-filter-expression-copy-summary
				data-filter-expression-summary-copied={s.copiedFilterExpressionSummary ===
				expressionSummaryText
					? 'true'
					: undefined}
			>
				<FileText class="h-3.5 w-3.5" aria-hidden="true" />
				{$t('forms.copyFilterExpressionSummary')}
			</button>

			<button
				type="button"
				class="inline-flex items-center gap-1 rounded-control border border-input bg-raised px-2.5 py-2 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
				onclick={() => exportViewDraftFilterExpressionSummary(expressionSummaryText)}
				disabled={!expressionSummaryText}
				aria-label={$t('forms.exportFilterExpressionSummary')}
				data-filter-expression-export-summary
				data-filter-expression-summary-exported={s.exportedFilterExpressionSummary ===
				expressionSummaryText
					? 'true'
					: undefined}
			>
				<FileDown class="h-3.5 w-3.5" aria-hidden="true" />
				{$t('forms.exportFilterExpressionSummary')}
			</button>

			<button
				type="button"
				class="inline-flex items-center gap-1 rounded-control border border-input bg-raised px-2.5 py-2 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
				onclick={() => focusViewDraftFilterExpressionRecords(expressionMatchingRecords)}
				disabled={!expressionRecordIdsText}
				data-filter-expression-focus-records
				data-record-count={expressionMatchCount ?? ''}
			>
				<TableProperties class="h-3.5 w-3.5" aria-hidden="true" />
				{$t('forms.focusFilterExpressionRecords')}
			</button>

			<button
				type="button"
				class="inline-flex items-center gap-1 rounded-control border border-input bg-raised px-2.5 py-2 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
				onclick={() => exportViewDraftFilterExpressionCsv(expressionMatchingRecords)}
				disabled={!expressionRecordIdsText}
				data-filter-expression-export-csv
				data-filter-expression-exported={s.exportedFilterExpressionRecordIds ===
				expressionRecordIdsText
					? 'true'
					: undefined}
				data-record-count={expressionMatchCount ?? ''}
			>
				<FileDown class="h-3.5 w-3.5" aria-hidden="true" />
				{$t('forms.exportFilterExpressionRecords')}
			</button>

			<button
				type="button"
				class="inline-flex items-center gap-1 rounded-control border border-input bg-raised px-2.5 py-2 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
				onclick={() => copyViewDraftFilterExpressionCsvRecords(expressionMatchingRecords)}
				disabled={!expressionRecordIdsText}
				data-filter-expression-copy-matches-csv
				data-filter-expression-matches-csv-copied={s.copiedFilterExpressionCsvRecordIds ===
				expressionRecordIdsText
					? 'true'
					: undefined}
				data-record-count={expressionMatchCount ?? ''}
			>
				<Copy class="h-3.5 w-3.5" aria-hidden="true" />
				{$t('forms.copyFilterExpressionRecordsCsv')}
			</button>

			<button
				type="button"
				class="inline-flex items-center gap-1 rounded-control border border-input bg-raised px-2.5 py-2 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
				onclick={() => copyViewDraftFilterExpressionJsonRecords(expressionMatchingRecords)}
				disabled={!expressionRecordIdsText}
				data-filter-expression-copy-matches-json
				data-filter-expression-matches-json-copied={s.copiedFilterExpressionJsonRecordIds ===
				expressionRecordIdsText
					? 'true'
					: undefined}
				data-record-count={expressionMatchCount ?? ''}
			>
				<Copy class="h-3.5 w-3.5" aria-hidden="true" />
				{$t('forms.copyFilterExpressionRecordsJson')}
			</button>

			<button
				type="button"
				class="inline-flex items-center gap-1 rounded-control border border-input bg-raised px-2.5 py-2 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
				onclick={() => exportViewDraftFilterExpressionJson(expressionMatchingRecords)}
				disabled={!expressionRecordIdsText}
				data-filter-expression-export-json
				data-filter-expression-json-exported={s.exportedFilterExpressionJsonRecordIds ===
				expressionRecordIdsText
					? 'true'
					: undefined}
				data-record-count={expressionMatchCount ?? ''}
			>
				<FileText class="h-3.5 w-3.5" aria-hidden="true" />
				{$t('forms.exportFilterExpressionRecordsJson')}
			</button>
		</div>
	{/if}

	{#each visibleViewDraftFilterGroups() as group, groupIndex (groupIndex)}
		{@const groupMatchCount = viewDraftFilterGroupMatchCount(group)}

		<div
			class={`space-y-2 rounded-card border border-line bg-raised p-2 ${
				group.disabled ? 'opacity-70 ring-1 ring-warning' : ''
			}`}
			data-filter-group-disabled-state={group.disabled ? 'true' : 'false'}
		>
			<div class="flex flex-wrap items-end gap-2">
				<label class="min-w-36 flex-1 space-y-1">
					<span class="text-xs font-medium uppercase text-fg-muted"
						>{$t('forms.filterGroup')} {groupIndex + 1}</span
					>

					<select
						value={group.logic}
						onchange={(event) =>
							updateViewDraftFilterGroupLogic(groupIndex, selectValue(event) as ViewFilterLogic)}
						class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
					>
						<option value="all">{$t('forms.filterMatchAll')}</option>
						<option value="any">{$t('forms.filterMatchAny')}</option>
					</select>
				</label>

				<Button
					size="sm"
					variant="secondary"
					onclick={() => addViewDraftFilter(groupIndex)}
					disabled={hasReachedFilterLimit()}
				>
					<Plus class="mr-2 h-4 w-4" aria-hidden="true" />
					{$t('forms.addFilter')}
				</Button>

				<span
					class="self-end rounded-control border border-line bg-nav px-2.5 py-2 text-xs font-medium text-fg-muted"
					data-filter-group-match-count={groupIndex + 1}
					data-record-count={groupMatchCount ?? ''}
				>
					{groupMatchCount === null
						? $t('forms.filterMatchCountPending')
						: $t('forms.filterMatchCount', {
								values: { count: groupMatchCount }
							})}
				</span>

				<div class="flex items-center rounded-control border border-line bg-nav p-0.5">
					<button
						type="button"
						class="rounded p-1.5 text-fg-muted hover:bg-raised hover:text-fg"
						onclick={() => toggleViewDraftFilterGroupDisabled(groupIndex)}
						aria-label={`${
							group.disabled ? $t('forms.enableFilterGroup') : $t('forms.disableFilterGroup')
						} ${groupIndex + 1}`}
						title={group.disabled ? $t('forms.enableFilterGroup') : $t('forms.disableFilterGroup')}
						data-filter-group-toggle-disabled={groupIndex + 1}
						data-filter-group-disabled={group.disabled ? 'true' : 'false'}
					>
						{#if group.disabled}
							<Eye class="h-4 w-4" aria-hidden="true" />
						{:else}
							<EyeOff class="h-4 w-4" aria-hidden="true" />
						{/if}
					</button>

					<button
						type="button"
						class="rounded p-1.5 text-fg-muted hover:bg-raised hover:text-fg disabled:bg-surface disabled:text-fg-muted"
						onclick={() => mergeViewDraftFilterGroupWithPrevious(groupIndex)}
						disabled={!canMergeViewDraftFilterGroup(groupIndex)}
						aria-label={`${$t('forms.mergeFilterGroup')} ${groupIndex + 1}`}
						data-filter-group-merge-previous={groupIndex + 1}
						><Link class="h-4 w-4" aria-hidden="true" /></button
					>

					<button
						type="button"
						class="rounded p-1.5 text-fg-muted hover:bg-raised hover:text-fg disabled:bg-surface disabled:text-fg-muted"
						onclick={() => duplicateViewDraftFilterGroup(groupIndex)}
						disabled={!canDuplicateViewDraftFilterGroup(groupIndex)}
						aria-label={`${$t('forms.duplicateFilterGroup')} ${groupIndex + 1}`}
						data-filter-group-duplicate={groupIndex + 1}
						><Copy class="h-4 w-4" aria-hidden="true" /></button
					>

					<button
						type="button"
						class="rounded p-1.5 text-fg-muted hover:bg-raised hover:text-fg disabled:bg-surface disabled:text-fg-muted"
						onclick={() => moveViewDraftFilterGroup(groupIndex, -1)}
						disabled={groupIndex === 0}
						aria-label={`${$t('forms.moveFilterGroupUp')} ${groupIndex + 1}`}
						data-filter-group-move-up={groupIndex + 1}
						><ArrowUp class="h-4 w-4" aria-hidden="true" /></button
					>

					<button
						type="button"
						class="rounded p-1.5 text-fg-muted hover:bg-raised hover:text-fg disabled:bg-surface disabled:text-fg-muted"
						onclick={() => moveViewDraftFilterGroup(groupIndex, 1)}
						disabled={groupIndex >= visibleViewDraftFilterGroups().length - 1}
						aria-label={`${$t('forms.moveFilterGroupDown')} ${groupIndex + 1}`}
						data-filter-group-move-down={groupIndex + 1}
						><ArrowDown class="h-4 w-4" aria-hidden="true" /></button
					>
				</div>

				{#if s.viewDraft.filterGroups.length > 1}
					<button
						type="button"
						class="rounded-control p-2 text-fg-muted hover:bg-surface hover:text-danger"
						onclick={() => removeViewDraftFilterGroup(groupIndex)}
						aria-label={$t('forms.removeFilterGroup')}
						data-filter-group-remove={groupIndex + 1}
						><X class="h-4 w-4" aria-hidden="true" /></button
					>
				{/if}
			</div>

			{#each group.filters as filter, filterIndex (filterIndex)}
				{@const filterMatchCount = viewDraftFilterMatchCount(filter)}

				<div
					class={`grid gap-2 rounded-card border border-line bg-nav p-2 sm:grid-cols-[minmax(0,1fr)_150px_minmax(0,1fr)_auto_auto_auto_auto_auto_auto] ${
						filter.disabled ? 'opacity-70 ring-1 ring-warning' : ''
					}`}
					data-filter-disabled-state={filter.disabled ? 'true' : 'false'}
				>
					<label class="space-y-1">
						<span class="text-xs font-medium uppercase text-fg-muted"
							>{$t('forms.filterField')}
							{filterLabelSuffix(groupIndex, filterIndex)}</span
						>

						<select
							value={filter.field}
							onchange={(event) =>
								updateViewDraftFilter(filterIndex, { field: selectValue(event) }, groupIndex)}
							class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
						>
							<option value="">{$t('forms.viewNoRule')}</option>

							{#each s.fields.filter((field) => field.visibility?.list !== false) as field (field.key)}
								<option value={field.key}>{fieldLabel(field)}</option>
							{/each}
						</select>
					</label>

					<label class="space-y-1">
						<span class="text-xs font-medium uppercase text-fg-muted"
							>{$t('forms.filterOperator')}
							{filterLabelSuffix(groupIndex, filterIndex)}</span
						>

						<select
							value={filter.operator}
							disabled={!filter.field}
							onchange={(event) =>
								updateViewDraftFilter(
									filterIndex,
									{
										operator: selectValue(event) as ViewFilterOperator
									},
									groupIndex
								)}
							class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
						>
							<option value="contains">{$t('forms.filterContains')}</option>
							<option value="equals">{$t('forms.filterEquals')}</option>
							<option value="not_equals">{$t('forms.filterNotEquals')}</option>
							<option value="not_empty">{$t('forms.filterNotEmpty')}</option>
						</select>
					</label>

					<label class="space-y-1">
						<span class="text-xs font-medium uppercase text-fg-muted"
							>{$t('forms.filterValue')}
							{filterLabelSuffix(groupIndex, filterIndex)}</span
						>

						<input
							class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
							value={filter.value}
							disabled={!filter.field || filter.operator === 'not_empty'}
							oninput={(event) =>
								updateViewDraftFilter(filterIndex, { value: inputValue(event) }, groupIndex)}
						/>
					</label>

					<span
						class="self-end rounded-control border border-line bg-raised px-2.5 py-2 text-xs font-medium text-fg-muted"
						data-filter-match-count={`${groupIndex + 1}.${filterIndex + 1}`}
						data-record-count={filterMatchCount ?? ''}
					>
						{filterMatchCount === null
							? $t('forms.filterMatchCountPending')
							: $t('forms.filterMatchCount', {
									values: { count: filterMatchCount }
								})}
					</span>

					<button
						type="button"
						class="self-end rounded-control p-2 text-fg-muted hover:bg-surface hover:text-fg"
						onclick={() => toggleViewDraftFilterDisabled(groupIndex, filterIndex)}
						aria-label={`${filter.disabled ? $t('forms.enableFilter') : $t('forms.disableFilter')} ${filterLabelSuffix(groupIndex, filterIndex)}`}
						title={filter.disabled ? $t('forms.enableFilter') : $t('forms.disableFilter')}
						data-filter-toggle-disabled={`${groupIndex + 1}.${filterIndex + 1}`}
						data-filter-disabled={filter.disabled ? 'true' : 'false'}
					>
						{#if filter.disabled}
							<Eye class="h-4 w-4" aria-hidden="true" />
						{:else}
							<EyeOff class="h-4 w-4" aria-hidden="true" />
						{/if}
					</button>

					<button
						type="button"
						class="self-end rounded-control p-2 text-fg-muted hover:bg-surface hover:text-danger"
						onclick={() => removeViewDraftFilter(filterIndex, groupIndex)}
						aria-label={$t('forms.removeFilter')}
						data-filter-remove={`${groupIndex + 1}.${filterIndex + 1}`}
						><X class="h-4 w-4" aria-hidden="true" /></button
					>

					<button
						type="button"
						class="self-end rounded-control p-2 text-fg-muted hover:bg-surface hover:text-fg disabled:bg-surface disabled:text-fg-muted"
						onclick={() => duplicateViewDraftFilter(groupIndex, filterIndex)}
						disabled={hasReachedFilterLimit()}
						aria-label={`${$t('forms.duplicateFilter')} ${filterLabelSuffix(groupIndex, filterIndex)}`}
						data-filter-duplicate={`${groupIndex + 1}.${filterIndex + 1}`}
						><Copy class="h-4 w-4" aria-hidden="true" /></button
					>

					<button
						type="button"
						class="self-end rounded-control p-2 text-fg-muted hover:bg-surface hover:text-fg disabled:bg-surface disabled:text-fg-muted"
						onclick={() => splitViewDraftFilterToGroup(groupIndex, filterIndex)}
						disabled={!canSplitViewDraftFilterToGroup(groupIndex, filterIndex)}
						aria-label={`${$t('forms.splitFilterToGroup')} ${filterLabelSuffix(groupIndex, filterIndex)}`}
						data-filter-split-group={`${groupIndex + 1}.${filterIndex + 1}`}
						><ArrowLeftRight class="h-4 w-4" aria-hidden="true" /></button
					>

					<div
						class="self-end flex items-center rounded-control border border-line bg-raised p-0.5"
					>
						<button
							type="button"
							class="rounded p-1.5 text-fg-muted hover:bg-surface hover:text-fg disabled:bg-surface disabled:text-fg-muted"
							onclick={() => moveViewDraftFilter(groupIndex, filterIndex, -1)}
							disabled={filterIndex === 0}
							aria-label={`${$t('forms.moveFilterUp')} ${filterLabelSuffix(groupIndex, filterIndex)}`}
							data-filter-move-up={`${groupIndex + 1}.${filterIndex + 1}`}
							><ArrowUp class="h-4 w-4" aria-hidden="true" /></button
						>

						<button
							type="button"
							class="rounded p-1.5 text-fg-muted hover:bg-surface hover:text-fg disabled:bg-surface disabled:text-fg-muted"
							onclick={() => moveViewDraftFilter(groupIndex, filterIndex, 1)}
							disabled={filterIndex >= group.filters.length - 1}
							aria-label={`${$t('forms.moveFilterDown')} ${filterLabelSuffix(groupIndex, filterIndex)}`}
							data-filter-move-down={`${groupIndex + 1}.${filterIndex + 1}`}
							><ArrowDown class="h-4 w-4" aria-hidden="true" /></button
						>
					</div>
				</div>
			{/each}
		</div>
	{/each}
</div>
