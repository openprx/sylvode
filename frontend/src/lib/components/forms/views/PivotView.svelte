<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import { Copy, FileDown, Filter, TableProperties } from '@lucide/svelte';
	import {
		fieldHasConditionalHidden,
		fieldHasConditionalReadOnly,
		recordFieldHiddenByCondition,
		recordFieldReadOnlyByCondition,
		recordsFieldHiddenByCondition,
		recordsFieldReadOnlyByCondition
	} from '#lib/forms/field-conditions.js';
	import { fieldLabel } from '#lib/forms/field-options.js';
	import { getFormsPageContext } from '#lib/forms/state/context.js';
	import {
		formatPivotDisplayValue,
		formatPivotTotalDisplayValue,
		pivotCellHeatmapStyle,
		pivotCellIntensity,
		pivotShowColumnTotals,
		pivotShowRowTotals
	} from '#lib/forms/views/pivot-format.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const {
		applyPivotDrilldownFilters,
		canApplyPivotDrilldownFilters,
		copyPivotCellValue,
		copyPivotDrilldownRecordIds,
		copyPivotTableMatrix,
		exportPivotDrilldownRecordsCsv,
		exportPivotTableMatrixCsv,
		focusPivotDrilldownRecords,
		pivotCellRecords,
		pivotColumnRecords,
		pivotDrilldownKey,
		pivotRowRecords,
		selectPivotDrilldown
	} = fp.viewPivot;
	const { selectRecord } = fp.recordPanel;
</script>

<div class="overflow-x-auto p-4" data-view-renderer="pivot">
	{#if !s.pivotTable.rowField || !s.pivotTable.columnField || !s.pivotTable.valueField}
		<div
			class="rounded-card border border-dashed border-input bg-nav px-4 py-6 text-sm text-fg-muted"
		>
			{$t('forms.pivotConfigureFields')}
		</div>
	{:else}
		<div class="mb-3 flex flex-wrap items-center gap-2 text-xs text-fg-muted">
			<span data-pivot-row-field={s.pivotTable.rowField.key}
				>{$t('forms.pivotRowField')}: {fieldLabel(s.pivotTable.rowField)}</span
			>

			{#if fieldHasConditionalHidden(s.pivotTable.rowField)}
				<span
					class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
					data-pivot-row-field-conditional-hidden={s.pivotTable.rowField.key}
					>{$t('forms.fieldConditionHiddenBadge')}</span
				>
			{/if}

			{#if fieldHasConditionalReadOnly(s.pivotTable.rowField)}
				<span
					class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
					data-pivot-row-field-conditional-readonly={s.pivotTable.rowField.key}
					>{$t('forms.fieldConditionReadOnlyBadge')}</span
				>
			{/if}

			<span>·</span>

			<span data-pivot-column-field={s.pivotTable.columnField.key}
				>{$t('forms.pivotColumnField')}: {fieldLabel(s.pivotTable.columnField)}</span
			>

			{#if fieldHasConditionalHidden(s.pivotTable.columnField)}
				<span
					class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
					data-pivot-column-field-conditional-hidden={s.pivotTable.columnField.key}
					>{$t('forms.fieldConditionHiddenBadge')}</span
				>
			{/if}

			{#if fieldHasConditionalReadOnly(s.pivotTable.columnField)}
				<span
					class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
					data-pivot-column-field-conditional-readonly={s.pivotTable.columnField.key}
					>{$t('forms.fieldConditionReadOnlyBadge')}</span
				>
			{/if}

			<span>·</span>
			<span data-pivot-value-field={s.pivotTable.valueField.key}
				>{$t('forms.pivotValueField')}: {fieldLabel(s.pivotTable.valueField)}</span
			>

			{#if fieldHasConditionalHidden(s.pivotTable.valueField)}
				<span
					class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
					data-pivot-value-field-conditional-hidden={s.pivotTable.valueField.key}
					>{$t('forms.fieldConditionHiddenBadge')}</span
				>
			{/if}

			{#if fieldHasConditionalReadOnly(s.pivotTable.valueField)}
				<span
					class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
					data-pivot-value-field-conditional-readonly={s.pivotTable.valueField.key}
					>{$t('forms.fieldConditionReadOnlyBadge')}</span
				>
			{/if}

			<span>·</span>

			<span data-pivot-aggregate={s.pivotTable.aggregate}>
				{$t('forms.pivotAggregate')}: {s.pivotTable.aggregate === 'count'
					? $t('forms.pivotAggregateCount')
					: s.pivotTable.aggregate === 'avg'
						? $t('forms.pivotAggregateAvg')
						: $t('forms.pivotAggregateSum')}
			</span>

			<span>·</span>

			<span data-pivot-totals={s.pivotTable.totalsMode}>
				{$t('forms.pivotTotals')}: {s.pivotTable.totalsMode === 'rows'
					? $t('forms.pivotTotalsRows')
					: s.pivotTable.totalsMode === 'columns'
						? $t('forms.pivotTotalsColumns')
						: s.pivotTable.totalsMode === 'none'
							? $t('forms.pivotTotalsNone')
							: $t('forms.pivotTotalsAll')}
			</span>

			<span>·</span>

			<span data-pivot-value-display={s.pivotTable.valueDisplay}>
				{$t('forms.pivotValueDisplay')}: {s.pivotTable.valueDisplay === 'percent_row'
					? $t('forms.pivotValueDisplayPercentRow')
					: s.pivotTable.valueDisplay === 'percent_column'
						? $t('forms.pivotValueDisplayPercentColumn')
						: s.pivotTable.valueDisplay === 'percent_total'
							? $t('forms.pivotValueDisplayPercentTotal')
							: $t('forms.pivotValueDisplayValue')}
			</span>

			<span>·</span>

			<span data-pivot-sort={s.pivotTable.sortMode}>
				{$t('forms.pivotSort')}: {s.pivotTable.sortMode === 'row_total_desc'
					? $t('forms.pivotSortRowTotalDesc')
					: s.pivotTable.sortMode === 'row_total_asc'
						? $t('forms.pivotSortRowTotalAsc')
						: s.pivotTable.sortMode === 'column_total_desc'
							? $t('forms.pivotSortColumnTotalDesc')
							: s.pivotTable.sortMode === 'column_total_asc'
								? $t('forms.pivotSortColumnTotalAsc')
								: $t('forms.pivotSortLabel')}
			</span>

			<span>·</span>
			<span data-pivot-empty-buckets={s.pivotTable.emptyBuckets}
				>{$t('forms.pivotEmptyBuckets')}: {s.pivotTable.emptyBuckets
					? $t('forms.yes')
					: $t('forms.no')}</span
			>
			<span>·</span>

			<span data-pivot-hide-zero-buckets={s.pivotTable.hideZeroBuckets}
				>{$t('forms.pivotHideZeroBuckets')}: {s.pivotTable.hideZeroBuckets
					? $t('forms.yes')
					: $t('forms.no')}</span
			>

			<span>·</span>
			<span data-pivot-show-counts={s.pivotTable.showCounts}
				>{$t('forms.pivotShowCounts')}: {s.pivotTable.showCounts
					? $t('forms.yes')
					: $t('forms.no')}</span
			>
			<span>·</span>
			<span data-pivot-heatmap={s.pivotTable.heatmap}
				>{$t('forms.pivotHeatmap')}: {s.pivotTable.heatmap ? $t('forms.yes') : $t('forms.no')}</span
			>
			<span>·</span>

			<span data-pivot-row-limit={s.pivotTable.rowLimit}>
				{$t('forms.pivotRowLimit')}: {s.pivotTable.rowLimit > 0
					? $t('forms.pivotRowLimitValue', {
							values: { count: s.pivotTable.rowLimit }
						})
					: $t('forms.pivotRowLimitAll')}
			</span>

			<span>·</span>

			<span data-pivot-column-limit={s.pivotTable.columnLimit}>
				{$t('forms.pivotColumnLimit')}: {s.pivotTable.columnLimit > 0
					? $t('forms.pivotColumnLimitValue', {
							values: { count: s.pivotTable.columnLimit }
						})
					: $t('forms.pivotColumnLimitAll')}
			</span>
		</div>

		<div class="mb-3 flex flex-wrap justify-end gap-2">
			<button
				type="button"
				class="inline-flex items-center gap-1 rounded-control border border-input px-2.5 py-1 text-xs font-medium text-fg hover:bg-surface"
				onclick={copyPivotTableMatrix}
				data-pivot-copy-table
				data-pivot-table-copied={s.copiedPivotTableMatrix ? 'true' : undefined}
			>
				<Copy class="h-3.5 w-3.5" aria-hidden="true" />
				{$t('forms.copyPivotTable')}
			</button>

			<button
				type="button"
				class="inline-flex items-center gap-1 rounded-control border border-input px-2.5 py-1 text-xs font-medium text-fg hover:bg-surface"
				onclick={exportPivotTableMatrixCsv}
				data-pivot-export-table
				data-pivot-table-exported={s.exportedPivotTableMatrix ? 'true' : undefined}
			>
				<FileDown class="h-3.5 w-3.5" aria-hidden="true" />
				{$t('forms.exportPivotTable')}
			</button>
		</div>

		<table class="min-w-full divide-y divide-line text-sm">
			<thead class="bg-nav text-left text-xs uppercase tracking-wide text-fg-muted">
				<tr>
					<th class="px-4 py-3 font-medium">
						<div class="flex flex-wrap items-center gap-1.5">
							<span>{fieldLabel(s.pivotTable.rowField)}</span>

							{#if fieldHasConditionalHidden(s.pivotTable.rowField)}
								<span
									class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium text-fg-muted"
									data-pivot-table-row-field-conditional-hidden={s.pivotTable.rowField.key}
									>{$t('forms.fieldConditionHiddenBadge')}</span
								>
							{/if}

							{#if fieldHasConditionalReadOnly(s.pivotTable.rowField)}
								<span
									class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium text-fg-muted"
									data-pivot-table-row-field-conditional-readonly={s.pivotTable.rowField.key}
									>{$t('forms.fieldConditionReadOnlyBadge')}</span
								>
							{/if}
						</div>
					</th>

					{#each s.pivotTable.columns as column, index (index)}
						<th class="px-4 py-3 text-right font-medium" data-pivot-column={column.label}>
							<div class="flex flex-wrap items-center justify-end gap-1.5">
								<span>{column.label}</span>

								{#if recordsFieldHiddenByCondition(s.pivotTable.columnField, pivotColumnRecords(column))}
									<span
										class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium text-fg-muted"
										data-pivot-column-conditional-hidden={`${column.label}:${s.pivotTable.columnField.key}`}
										>{$t('forms.fieldConditionHiddenBadge')}</span
									>
								{/if}

								{#if recordsFieldReadOnlyByCondition(s.pivotTable.columnField, pivotColumnRecords(column))}
									<span
										class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium text-fg-muted"
										data-pivot-column-conditional-readonly={`${column.label}:${s.pivotTable.columnField.key}`}
										>{$t('forms.fieldConditionReadOnlyBadge')}</span
									>
								{/if}
							</div>
						</th>
					{/each}

					{#if pivotShowRowTotals(s.pivotTable.totalsMode)}
						<th class="px-4 py-3 text-right font-medium">{$t('forms.pivotTotal')}</th>
					{/if}
				</tr>
			</thead>

			<tbody class="divide-y divide-line">
				{#each s.pivotTable.rows as row, index (index)}
					<tr class="hover:bg-surface" data-pivot-row={row.label}>
						<th class="px-4 py-3 text-left font-medium text-fg">
							<div class="flex flex-wrap items-center gap-1.5">
								<span>{row.label}</span>

								{#if recordsFieldHiddenByCondition(s.pivotTable.rowField, pivotRowRecords(row))}
									<span
										class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium text-fg-muted"
										data-pivot-row-conditional-hidden={`${row.label}:${s.pivotTable.rowField.key}`}
										>{$t('forms.fieldConditionHiddenBadge')}</span
									>
								{/if}

								{#if recordsFieldReadOnlyByCondition(s.pivotTable.rowField, pivotRowRecords(row))}
									<span
										class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium text-fg-muted"
										data-pivot-row-conditional-readonly={`${row.label}:${s.pivotTable.rowField.key}`}
										>{$t('forms.fieldConditionReadOnlyBadge')}</span
									>
								{/if}
							</div>
						</th>

						{#each s.pivotTable.columns as column, index (index)}
							{@const cellValue = row.cells[column.key] ?? 0}

							<td
								class="px-4 py-3 text-right text-fg"
								data-pivot-cell={`${row.label}:${column.label}`}
							>
								<button
									type="button"
									class="flex w-full flex-col items-end rounded px-2 py-1 text-right font-medium text-fg hover:bg-surface"
									style={pivotCellHeatmapStyle(cellValue, s.pivotTable)}
									onclick={() => selectPivotDrilldown(row, column)}
									data-pivot-cell-button={`${row.label}:${column.label}`}
									data-pivot-cell-intensity={pivotCellIntensity(cellValue, s.pivotTable)}
								>
									<span
										>{formatPivotDisplayValue(
											cellValue,
											row.total,
											column.total,
											s.pivotTable
										)}</span
									>

									{#if s.pivotTable.showCounts}
										<span
											class="mt-0.5 text-[11px] font-normal text-fg-muted"
											data-pivot-cell-count={`${row.label}:${column.label}`}
											data-record-count={row.cellCounts[column.key] ?? 0}
											>{$t('forms.pivotCellRecordCount', {
												values: { count: row.cellCounts[column.key] ?? 0 }
											})}</span
										>
									{/if}

									{#if recordsFieldHiddenByCondition(s.pivotTable.rowField, pivotCellRecords(row, column))}
										<span
											class="mt-0.5 rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
											data-pivot-cell-row-conditional-hidden={`${row.label}:${column.label}:${s.pivotTable.rowField.key}`}
											>{$t('forms.fieldConditionHiddenBadge')}</span
										>
									{/if}

									{#if recordsFieldReadOnlyByCondition(s.pivotTable.columnField, pivotCellRecords(row, column))}
										<span
											class="mt-0.5 rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
											data-pivot-cell-column-conditional-readonly={`${row.label}:${column.label}:${s.pivotTable.columnField.key}`}
											>{$t('forms.fieldConditionReadOnlyBadge')}</span
										>
									{/if}
								</button>

								<button
									type="button"
									class="mt-1 inline-flex items-center rounded p-1 text-fg-muted hover:bg-surface hover:text-fg"
									onclick={() => copyPivotCellValue(row, column, cellValue)}
									aria-label={$t('forms.copyPivotCellValue')}
									data-pivot-cell-copy={`${row.label}:${column.label}`}
									data-pivot-cell-copied={s.copiedPivotCellKey === `${row.label}:${column.label}`
										? 'true'
										: undefined}><Copy class="h-3.5 w-3.5" aria-hidden="true" /></button
								>
							</td>
						{/each}

						{#if pivotShowRowTotals(s.pivotTable.totalsMode)}
							<td
								class="px-4 py-3 text-right font-semibold text-fg"
								data-pivot-row-total={row.label}
							>
								<button
									type="button"
									class="inline-flex justify-end rounded px-2 py-1 text-right font-semibold text-fg hover:bg-surface"
									onclick={() => selectPivotDrilldown(row, null)}
									data-pivot-row-total-button={row.label}
									>{formatPivotTotalDisplayValue(row.total, 'row', s.pivotTable)}</button
								>
							</td>
						{/if}
					</tr>
				{/each}
			</tbody>

			{#if pivotShowColumnTotals(s.pivotTable.totalsMode)}
				<tfoot class="bg-nav text-sm font-semibold text-fg">
					<tr>
						<th class="px-4 py-3 text-left">{$t('forms.pivotTotal')}</th>

						{#each s.pivotTable.columns as column, index (index)}
							<td class="px-4 py-3 text-right" data-pivot-column-total={column.label}>
								<button
									type="button"
									class="inline-flex justify-end rounded px-2 py-1 text-right font-semibold text-fg hover:bg-surface"
									onclick={() => selectPivotDrilldown(null, column)}
									data-pivot-column-total-button={column.label}
									>{formatPivotTotalDisplayValue(column.total, 'column', s.pivotTable)}</button
								>
							</td>
						{/each}

						{#if pivotShowRowTotals(s.pivotTable.totalsMode)}
							<td class="px-4 py-3 text-right" data-pivot-grand-total>
								<button
									type="button"
									class="inline-flex justify-end rounded px-2 py-1 text-right font-semibold text-fg hover:bg-surface"
									onclick={() => selectPivotDrilldown(null, null)}
									data-pivot-grand-total-button
									>{formatPivotTotalDisplayValue(
										s.pivotTable.grandTotal,
										'grand',
										s.pivotTable
									)}</button
								>
							</td>
						{/if}
					</tr>
				</tfoot>
			{/if}
		</table>

		{#if s.pivotDrilldown}
			<section
				class="mt-4 rounded-card border border-line bg-raised p-4"
				data-pivot-drilldown={`${s.pivotDrilldown.rowLabel}:${s.pivotDrilldown.columnLabel}`}
				data-pivot-drilldown-copied={s.copiedPivotDrilldownKey === pivotDrilldownKey()
					? `${s.pivotDrilldown.rowLabel}:${s.pivotDrilldown.columnLabel}`
					: undefined}
				data-pivot-drilldown-exported={s.exportedPivotDrilldownKey === pivotDrilldownKey()
					? `${s.pivotDrilldown.rowLabel}:${s.pivotDrilldown.columnLabel}`
					: undefined}
				data-pivot-drilldown-filters-applied={s.appliedPivotDrilldownFilterKey ===
				pivotDrilldownKey()
					? `${s.pivotDrilldown.rowLabel}:${s.pivotDrilldown.columnLabel}`
					: undefined}
				data-record-count={s.pivotDrilldownRecords.length}
			>
				<div class="mb-3 flex items-start justify-between gap-3">
					<div>
						<h3 class="text-sm font-semibold text-fg">
							{$t('forms.pivotDrilldown')}
						</h3>

						<p class="mt-1 text-xs text-fg-muted">
							{$t('forms.pivotDrilldownFor', {
								values: {
									row: s.pivotDrilldown.rowLabel,
									column: s.pivotDrilldown.columnLabel,
									count: s.pivotDrilldownRecords.length
								}
							})}
						</p>
					</div>

					<div class="flex shrink-0 items-center gap-2">
						<button
							type="button"
							class="inline-flex items-center gap-1 rounded-control border border-input px-2.5 py-1 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
							onclick={copyPivotDrilldownRecordIds}
							disabled={s.pivotDrilldownRecords.length === 0}
							data-pivot-drilldown-copy-ids
						>
							<Copy class="h-3.5 w-3.5" aria-hidden="true" />
							{$t('forms.copyPivotDrilldownRecordIds')}
						</button>

						<button
							type="button"
							class="inline-flex items-center gap-1 rounded-control border border-input px-2.5 py-1 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
							onclick={exportPivotDrilldownRecordsCsv}
							disabled={s.pivotDrilldownRecords.length === 0}
							data-pivot-drilldown-export-csv
							data-pivot-drilldown-exported={s.exportedPivotDrilldownKey === pivotDrilldownKey()
								? 'true'
								: undefined}
						>
							<FileDown class="h-3.5 w-3.5" aria-hidden="true" />
							{$t('forms.exportPivotDrilldownRecords')}
						</button>

						<button
							type="button"
							class="inline-flex items-center gap-1 rounded-control border border-input px-2.5 py-1 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
							onclick={applyPivotDrilldownFilters}
							disabled={!canApplyPivotDrilldownFilters()}
							data-pivot-drilldown-apply-filters
							data-pivot-drilldown-filters-applied={s.appliedPivotDrilldownFilterKey ===
							pivotDrilldownKey()
								? 'true'
								: undefined}
						>
							<Filter class="h-3.5 w-3.5" aria-hidden="true" />
							{$t('forms.applyPivotDrilldownFilters')}
						</button>

						<button
							type="button"
							class="inline-flex items-center gap-1 rounded-control border border-input px-2.5 py-1 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
							onclick={focusPivotDrilldownRecords}
							disabled={s.pivotDrilldownRecords.length === 0}
							data-pivot-drilldown-focus
						>
							<TableProperties class="h-3.5 w-3.5" aria-hidden="true" />
							{$t('forms.focusPivotDrilldown')}
						</button>

						<button
							type="button"
							class="rounded-control p-1.5 text-fg-muted hover:bg-surface hover:text-fg"
							onclick={() => (s.pivotDrilldown = null)}
							aria-label={$t('common.close')}>x</button
						>
					</div>
				</div>

				{#if s.pivotDrilldownRecords.length === 0}
					<p class="text-sm text-fg-muted">
						{$t('forms.pivotDrilldownEmpty')}
					</p>
				{:else}
					<div class="divide-y divide-line">
						{#each s.pivotDrilldownRecords as record (record.id)}
							<div
								class="flex items-center justify-between gap-3 py-2"
								data-pivot-drilldown-record={record.id}
							>
								<div class="min-w-0">
									<div class="truncate text-sm font-medium text-fg">
										{record.title}
									</div>

									<div class="mt-0.5 text-xs text-fg-muted">
										{record.id}
									</div>

									<div class="mt-1 flex flex-wrap items-center gap-1.5">
										{#if recordFieldHiddenByCondition(s.pivotTable.rowField, record)}
											<span
												class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
												data-pivot-drilldown-row-conditional-hidden={`${record.id}:${s.pivotTable.rowField.key}`}
												>{$t('forms.fieldConditionHiddenBadge')}</span
											>
										{/if}

										{#if recordFieldReadOnlyByCondition(s.pivotTable.rowField, record)}
											<span
												class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
												data-pivot-drilldown-row-conditional-readonly={`${record.id}:${s.pivotTable.rowField.key}`}
												>{$t('forms.fieldConditionReadOnlyBadge')}</span
											>
										{/if}

										{#if recordFieldHiddenByCondition(s.pivotTable.columnField, record)}
											<span
												class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
												data-pivot-drilldown-column-conditional-hidden={`${record.id}:${s.pivotTable.columnField.key}`}
												>{$t('forms.fieldConditionHiddenBadge')}</span
											>
										{/if}

										{#if recordFieldReadOnlyByCondition(s.pivotTable.columnField, record)}
											<span
												class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
												data-pivot-drilldown-column-conditional-readonly={`${record.id}:${s.pivotTable.columnField.key}`}
												>{$t('forms.fieldConditionReadOnlyBadge')}</span
											>
										{/if}
									</div>
								</div>

								<button
									type="button"
									class="shrink-0 rounded-control border border-input px-2.5 py-1 text-xs font-medium text-fg hover:bg-surface"
									onclick={() => selectRecord(record)}
									data-pivot-drilldown-open={record.id}>{$t('forms.openRecord')}</button
								>
							</div>
						{/each}
					</div>
				{/if}
			</section>
		{/if}
	{/if}
</div>
