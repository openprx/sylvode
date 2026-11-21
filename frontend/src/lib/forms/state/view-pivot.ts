import type { FormField, FormRecord } from '#lib/api/forms.js';
import { csvDownloadBlob } from '#lib/utils/csv.js';
import * as precedence from '#lib/forms/view-precedence.js';
import { toast } from '#lib/stores/toast.js';
import type { PivotTable, ViewFilterDraft, ViewFilterLogic } from '#lib/forms/page-types.js';
import { translate } from '#lib/forms/i18n.js';
import { recordAmountCurrency, recordNumericValue } from '#lib/forms/record-format.js';
import { FormsController } from '#lib/forms/state/controller.js';
import {
	viewConfigBoolean,
	viewConfigString,
	viewPivotAggregate,
	viewPivotColumnLimit,
	viewPivotRowLimit,
	viewPivotSortMode,
	viewPivotTotalsMode,
	viewPivotValueDisplay
} from '#lib/forms/view-config.js';
import {
	formatPivotDisplayValue,
	pivotAggregateValue,
	pivotConfiguredBuckets,
	pivotFieldBucket,
	pivotTableMatrixCsv,
	pivotTableMatrixText,
	sortPivotBuckets
} from '#lib/forms/views/pivot-format.js';

export class ViewPivotController extends FormsController {
	buildPivotTable = (sourceRecords: FormRecord[]): PivotTable => {
		const rowField = this.viewPivotRowField();
		const columnField = this.viewPivotColumnField();
		const valueField = this.viewPivotValueField();
		const aggregate = viewPivotAggregate(this.s.selectedView?.config?.pivot_aggregate);
		const totalsMode = viewPivotTotalsMode(this.s.selectedView?.config?.pivot_totals);
		const valueDisplay = viewPivotValueDisplay(this.s.selectedView?.config?.pivot_value_display);
		const sortMode = viewPivotSortMode(this.s.selectedView?.config?.pivot_sort);
		const emptyBuckets = viewConfigBoolean(this.s.selectedView, 'pivot_empty_buckets');
		const hideZeroBuckets = viewConfigBoolean(this.s.selectedView, 'pivot_hide_zero_buckets');
		const showCounts = viewConfigBoolean(this.s.selectedView, 'pivot_show_counts');
		const heatmap = viewConfigBoolean(this.s.selectedView, 'pivot_heatmap');
		const rowLimit = viewPivotRowLimit(this.s.selectedView?.config?.pivot_row_limit);
		const columnLimit = viewPivotColumnLimit(this.s.selectedView?.config?.pivot_column_limit);

		if (!rowField || !columnField || !valueField) {
			return {
				rowField,
				columnField,
				valueField,
				valueCurrency: '',
				aggregate,
				totalsMode,
				valueDisplay,
				sortMode,
				emptyBuckets,
				hideZeroBuckets,
				showCounts,
				heatmap,
				rowLimit,
				columnLimit,
				maxCellValue: 0,
				columns: [],
				rows: [],
				grandTotal: 0
			};
		}

		type PivotAccumulator = { sum: number; count: number };

		const rowMap = new Map<
			string,
			{
				key: string;
				label: string;
				total: PivotAccumulator;
				cells: Record<string, PivotAccumulator>;
			}
		>();

		const columnMap = new Map<string, { key: string; label: string; total: PivotAccumulator }>();
		const grandTotal: PivotAccumulator = { sum: 0, count: 0 };
		let valueCurrency = '';

		const ensureRow = (bucket: { key: string; label: string }) => {
			if (!rowMap.has(bucket.key)) {
				rowMap.set(bucket.key, {
					key: bucket.key,
					label: bucket.label,
					total: { sum: 0, count: 0 },
					cells: {}
				});
			}

			return rowMap.get(bucket.key);
		};

		const ensureColumn = (bucket: { key: string; label: string }) => {
			if (!columnMap.has(bucket.key)) {
				columnMap.set(bucket.key, {
					key: bucket.key,
					label: bucket.label,
					total: { sum: 0, count: 0 }
				});
			}

			return columnMap.get(bucket.key);
		};

		if (emptyBuckets) {
			for (const bucket of pivotConfiguredBuckets(rowField)) ensureRow(bucket);
			for (const bucket of pivotConfiguredBuckets(columnField)) ensureColumn(bucket);
		}

		for (const record of sourceRecords) {
			const rowBucket = pivotFieldBucket(record, rowField);
			const columnBucket = pivotFieldBucket(record, columnField);
			const value = recordNumericValue(record, valueField);

			if (!valueCurrency) valueCurrency = recordAmountCurrency(record, valueField);

			const row = ensureRow(rowBucket);
			const column = ensureColumn(columnBucket);

			if (!row || !column) continue;
			if (!row.cells[columnBucket.key]) row.cells[columnBucket.key] = { sum: 0, count: 0 };

			row.cells[columnBucket.key].sum += value;
			row.cells[columnBucket.key].count += 1;
			row.total.sum += value;
			row.total.count += 1;
			column.total.sum += value;
			column.total.count += 1;
			grandTotal.sum += value;
			grandTotal.count += 1;
		}

		const rows = sortPivotBuckets(
			Array.from(rowMap.values()).map((row) => ({
				key: row.key,
				label: row.label,
				total: pivotAggregateValue(row.total.sum, row.total.count, aggregate),
				cells: Object.fromEntries(
					Object.entries(row.cells).map(([key, cell]) => [
						key,
						pivotAggregateValue(cell.sum, cell.count, aggregate)
					])
				),
				cellCounts: Object.fromEntries(
					Object.entries(row.cells).map(([key, cell]) => [key, cell.count])
				)
			})),
			sortMode,
			'row'
		);

		const columns = sortPivotBuckets(
			Array.from(columnMap.values()).map((column) => ({
				key: column.key,
				label: column.label,
				total: pivotAggregateValue(column.total.sum, column.total.count, aggregate)
			})),
			sortMode,
			'column'
		);

		const filteredRows = hideZeroBuckets
			? rows.filter((row) => Math.abs(row.total) > Number.EPSILON)
			: rows;

		const filteredColumns = hideZeroBuckets
			? columns.filter((column) => Math.abs(column.total) > Number.EPSILON)
			: columns;

		const visibleColumns =
			columnLimit > 0 ? filteredColumns.slice(0, columnLimit) : filteredColumns;

		const visibleRows = rowLimit > 0 ? filteredRows.slice(0, rowLimit) : filteredRows;
		const maxCellValue = Math.max(
			0,
			...visibleRows.flatMap((row) =>
				visibleColumns.map((column) => Math.abs(row.cells[column.key] ?? 0))
			)
		);

		return {
			rowField,
			columnField,
			valueField,
			valueCurrency,
			aggregate,
			totalsMode,
			valueDisplay,
			sortMode,
			emptyBuckets,
			hideZeroBuckets,
			showCounts,
			heatmap,
			rowLimit,
			columnLimit,
			maxCellValue,
			columns: visibleColumns,
			rows: visibleRows,
			grandTotal: pivotAggregateValue(grandTotal.sum, grandTotal.count, aggregate)
		};
	};

	buildPivotDrilldownRecords = (sourceRecords: FormRecord[]): FormRecord[] => {
		const drilldown = this.s.pivotDrilldown;

		if (!drilldown || !this.s.pivotTable.rowField || !this.s.pivotTable.columnField) return [];

		return sourceRecords.filter((record) => {
			const rowBucket = pivotFieldBucket(record, this.s.pivotTable.rowField);
			const columnBucket = pivotFieldBucket(record, this.s.pivotTable.columnField);
			const rowMatches = drilldown.rowKey === null || rowBucket.key === drilldown.rowKey;
			const columnMatches =
				drilldown.columnKey === null || columnBucket.key === drilldown.columnKey;

			return rowMatches && columnMatches;
		});
	};

	pivotRowRecords = (row: { key: string }): FormRecord[] => {
		if (!this.s.pivotTable.rowField) return [];

		return this.s.viewRecords.filter(
			(record) => pivotFieldBucket(record, this.s.pivotTable.rowField).key === row.key
		);
	};

	pivotColumnRecords = (column: { key: string }): FormRecord[] => {
		if (!this.s.pivotTable.columnField) return [];

		return this.s.viewRecords.filter(
			(record) => pivotFieldBucket(record, this.s.pivotTable.columnField).key === column.key
		);
	};

	pivotCellRecords = (row: { key: string }, column: { key: string }): FormRecord[] => {
		if (!this.s.pivotTable.rowField || !this.s.pivotTable.columnField) return [];

		return this.s.viewRecords.filter(
			(record) =>
				pivotFieldBucket(record, this.s.pivotTable.rowField).key === row.key &&
				pivotFieldBucket(record, this.s.pivotTable.columnField).key === column.key
		);
	};

	selectPivotDrilldown = (
		row: { key: string; label: string } | null,
		column: { key: string; label: string } | null
	) => {
		this.s.pivotDrilldown = {
			rowKey: row?.key ?? null,
			rowLabel: row?.label ?? translate('forms.pivotTotal'),
			columnKey: column?.key ?? null,
			columnLabel: column?.label ?? translate('forms.pivotTotal')
		};

		this.s.copiedPivotDrilldownKey = null;
		this.s.exportedPivotDrilldownKey = null;
		this.s.appliedPivotDrilldownFilterKey = null;
	};

	pivotDrilldownKey = (): string | null => {
		if (!this.s.pivotDrilldown) return null;

		return `${this.s.pivotDrilldown.rowKey ?? '__all__'}:${this.s.pivotDrilldown.columnKey ?? '__all__'}`;
	};

	pivotDrilldownFilterDrafts = (): ViewFilterDraft[] => {
		if (!this.s.pivotDrilldown) return [];

		const filters: ViewFilterDraft[] = [];

		if (
			this.s.pivotDrilldown.rowKey !== null &&
			this.s.pivotTable.rowField &&
			this.s.pivotDrilldown.rowLabel !== translate('forms.unspecified')
		) {
			filters.push({
				field: this.s.pivotTable.rowField.key,
				operator: 'equals',
				value: this.s.pivotDrilldown.rowLabel
			});
		}

		if (
			this.s.pivotDrilldown.columnKey !== null &&
			this.s.pivotTable.columnField &&
			this.s.pivotDrilldown.columnLabel !== translate('forms.unspecified')
		) {
			filters.push({
				field: this.s.pivotTable.columnField.key,
				operator: 'equals',
				value: this.s.pivotDrilldown.columnLabel
			});
		}

		return filters;
	};

	canApplyPivotDrilldownFilters = (): boolean => {
		const filters = this.pivotDrilldownFilterDrafts();

		if (filters.length === 0) return false;

		const currentGroups = this.fp.viewDraftFilters
			.visibleViewDraftFilterGroups()
			.filter((group) => group.filters.length > 0);
		const currentFilterCount = currentGroups.reduce(
			(count, group) => count + group.filters.length,
			0
		);

		if (currentFilterCount + filters.length > 10) return false;
		if (currentGroups.length < 5) return true;

		const lastGroup = currentGroups[currentGroups.length - 1];

		return Boolean(lastGroup && lastGroup.filters.length + filters.length <= 5);
	};

	applyPivotDrilldownFilters = () => {
		if (!this.canApplyPivotDrilldownFilters()) return;

		const filters = this.pivotDrilldownFilterDrafts();

		const currentGroups = this.fp.viewDraftFilters
			.visibleViewDraftFilterGroups()
			.filter((group) => group.filters.length > 0)
			.map((group) => ({
				logic: group.logic,
				disabled: group.disabled || undefined,
				filters: group.filters.map((filter) => ({ ...filter }))
			}));

		const nextGroups =
			currentGroups.length === 0
				? [{ logic: 'all' as ViewFilterLogic, filters }]
				: currentGroups.length < 5
					? [...currentGroups, { logic: 'all' as ViewFilterLogic, filters }]
					: currentGroups.map((group, index) =>
							index === currentGroups.length - 1
								? { ...group, filters: [...group.filters, ...filters] }
								: group
						);

		this.fp.viewDraftFilters.setViewDraftFilterGroups(nextGroups);
		this.fp.viewDraftFilters.updateViewDraftFilterRootLogic('all');
		this.s.appliedPivotDrilldownFilterKey = this.pivotDrilldownKey();
		toast.success(translate('forms.pivotDrilldownFiltersApplied'));
	};

	copyPivotCellValue = async (
		row: { key: string; label: string; total: number },
		column: { key: string; label: string; total: number },
		value: number
	) => {
		const text = formatPivotDisplayValue(value, row.total, column.total, this.s.pivotTable);
		const clipboard = typeof navigator !== 'undefined' ? navigator.clipboard : null;

		try {
			if (clipboard?.writeText) {
				await clipboard.writeText(text);
			}
		} catch {
			// Clipboard permissions are browser-dependent; keep pivot operations non-blocking.
		}

		this.s.copiedPivotCellKey = `${row.label}:${column.label}`;
		toast.success(translate('forms.pivotCellCopied'));
	};

	copyPivotDrilldownRecordIds = async () => {
		if (!this.s.pivotDrilldown || this.s.pivotDrilldownRecords.length === 0) return;

		const recordIds = this.s.pivotDrilldownRecords.map((record) => record.id);
		const clipboard = typeof navigator !== 'undefined' ? navigator.clipboard : null;

		try {
			if (clipboard?.writeText) {
				await clipboard.writeText(recordIds.join('\n'));
			}
		} catch {
			// Clipboard permissions are browser-dependent; keep pivot operations non-blocking.
		}

		this.s.copiedPivotDrilldownKey = this.pivotDrilldownKey();
		toast.success(translate('forms.pivotDrilldownIdsCopied'));
	};

	exportPivotDrilldownRecordsCsv = () => {
		if (!this.s.selectedForm || !this.s.pivotDrilldown || this.s.pivotDrilldownRecords.length === 0)
			return;

		const csv = this.fp.filterExpressionExport.recordsCsv(this.s.pivotDrilldownRecords);
		const blob = csvDownloadBlob(csv);
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');

		anchor.href = url;
		anchor.download = `${this.s.selectedForm.key}-pivot-drilldown-records.csv`;
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);
		this.s.exportedPivotDrilldownKey = this.pivotDrilldownKey();
		toast.success(translate('forms.pivotDrilldownExported'));
	};

	copyPivotTableMatrix = async () => {
		const text = pivotTableMatrixText(this.s.pivotTable);

		if (!text) return;

		const clipboard = typeof navigator !== 'undefined' ? navigator.clipboard : null;

		try {
			if (clipboard?.writeText) {
				await clipboard.writeText(text);
			}
		} catch {
			// Clipboard permissions are browser-dependent; keep pivot operations non-blocking.
		}

		this.s.copiedPivotTableMatrix = true;
		toast.success(translate('forms.pivotTableCopied'));
	};

	exportPivotTableMatrixCsv = () => {
		const csv = pivotTableMatrixCsv(this.s.pivotTable);

		if (!this.s.selectedForm || !csv) return;

		const blob = csvDownloadBlob(csv);
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');

		anchor.href = url;
		anchor.download = `${this.s.selectedForm.key}-pivot-matrix.csv`;
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);
		this.s.exportedPivotTableMatrix = true;
		toast.success(translate('forms.pivotTableExported'));
	};

	focusPivotDrilldownRecords = () => {
		const recordIds = this.s.pivotDrilldownRecords.map((record) => record.id);

		if (recordIds.length === 0) return;

		this.s.focusedRecordIds = recordIds;
		this.s.pivotDrilldown = null;

		this.fp.views.showView(precedence.ALL_RECORDS_VIEW_ID, { remember: false });

		toast.success(translate('forms.pivotDrilldownFocused'));
	};

	viewPivotRowField = (): FormField | null => {
		return this.fp.viewRecords.fieldByKey(viewConfigString(this.s.selectedView, 'pivot_row_field'));
	};

	viewPivotColumnField = (): FormField | null => {
		const rowField = this.viewPivotRowField();
		const configured = this.fp.viewRecords.fieldByKey(
			viewConfigString(this.s.selectedView, 'pivot_column_field')
		);

		if (configured && configured.key !== rowField?.key) return configured;

		return null;
	};

	viewPivotValueField = (): FormField | null => {
		const configured = this.fp.viewRecords.fieldByKey(
			viewConfigString(this.s.selectedView, 'pivot_value_field')
		);

		if (configured && ['amount', 'number', 'integer'].includes(configured.type ?? 'text'))
			return configured;

		return null;
	};
}
