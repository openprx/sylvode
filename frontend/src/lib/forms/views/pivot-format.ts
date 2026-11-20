import type { FormField, FormRecord } from '#lib/api/forms.js';
import type {
	PivotAggregate,
	PivotSortMode,
	PivotTable,
	PivotTotalsMode
} from '#lib/forms/page-types.js';
import { fieldLabel } from '#lib/forms/field-options.js';
import { translate } from '#lib/forms/i18n.js';
import { csvCell } from '#lib/forms/import-csv.js';
import { displayValue, isRecord } from '#lib/forms/record-format.js';

export function pivotShowRowTotals(mode: PivotTotalsMode): boolean {
	return mode === 'all' || mode === 'rows';
}

export function pivotShowColumnTotals(mode: PivotTotalsMode): boolean {
	return mode === 'all' || mode === 'columns';
}

export function pivotCellIntensity(value: number, table: PivotTable): number {
	if (!table.heatmap || table.maxCellValue <= 0) return 0;

	const ratio = Math.min(Math.abs(value) / table.maxCellValue, 1);

	return Math.round(ratio * 100);
}

export function pivotCellHeatmapStyle(value: number, table: PivotTable): string {
	const intensity = pivotCellIntensity(value, table);

	if (intensity <= 0) return '';

	// A sequential ramp from 6% to 24% of the chart-1 token over the cell's own ground, so it
	// follows the theme. Proposed formula: the specification defines no ramp token.
	const share = (6 + (intensity / 100) * 18).toFixed(1);

	return `background-color: color-mix(in srgb, var(--rf-chart-1) ${share}%, transparent);`;
}

export function pivotFieldBucket(
	record: FormRecord,
	field: FormField | null
): { key: string; label: string } {
	const label = pivotFieldLabel(record, field);

	if (!field) return { key: '__unspecified__', label };

	const raw = record.values[field.key];

	if (typeof raw === 'string' || typeof raw === 'number' || typeof raw === 'boolean') {
		return { key: String(raw).toLowerCase(), label };
	}

	if (isRecord(raw)) {
		if (typeof raw.record_id === 'string') return { key: raw.record_id, label };

		if (typeof raw.value === 'string' || typeof raw.value === 'number') {
			return { key: String(raw.value).toLowerCase(), label };
		}
	}

	return { key: label.toLowerCase(), label };
}

export function pivotFieldLabel(record: FormRecord, field: FormField | null): string {
	if (!field) return translate('forms.unspecified');

	const value = displayValue(record.values[field.key]);

	return value && value !== '-' ? value : translate('forms.unspecified');
}

export function formatPivotValue(
	value: number,
	field: FormField | null,
	currency = '',
	aggregate: PivotAggregate = 'sum'
): string {
	if (aggregate !== 'count' && (field?.type ?? 'number') === 'amount') {
		const scale = typeof field?.amount?.scale === 'number' ? field.amount.scale : 2;
		const formatted = value.toFixed(scale);

		return currency ? `${formatted} ${currency}` : formatted;
	}

	const formatted = Number.isInteger(value) ? String(value) : value.toFixed(2);

	return formatted;
}

export function formatPivotPercent(value: number, denominator: number): string {
	if (Math.abs(denominator) < Number.EPSILON) return '0.00%';

	return `${((value / denominator) * 100).toFixed(2)}%`;
}

export function formatPivotDisplayValue(
	value: number,
	rowTotal = 0,
	columnTotal = 0,
	table: PivotTable
): string {
	if (table.valueDisplay === 'percent_row') return formatPivotPercent(value, rowTotal);
	if (table.valueDisplay === 'percent_column') return formatPivotPercent(value, columnTotal);
	if (table.valueDisplay === 'percent_total') return formatPivotPercent(value, table.grandTotal);

	return formatPivotValue(value, table.valueField, table.valueCurrency, table.aggregate);
}

export function formatPivotTotalDisplayValue(
	value: number,
	totalKind: 'row' | 'column' | 'grand',
	table: PivotTable
): string {
	if (table.valueDisplay === 'percent_row' && totalKind === 'row') {
		return formatPivotPercent(value, value);
	}

	if (table.valueDisplay === 'percent_column' && totalKind === 'column') {
		return formatPivotPercent(value, value);
	}

	if (table.valueDisplay !== 'value') return formatPivotPercent(value, table.grandTotal);

	return formatPivotValue(value, table.valueField, table.valueCurrency, table.aggregate);
}

export function pivotAggregateValue(sum: number, count: number, aggregate: PivotAggregate): number {
	if (aggregate === 'count') return count;
	if (aggregate === 'avg') return count > 0 ? sum / count : 0;

	return sum;
}

export function sortPivotBuckets<T extends { label: string; total: number }>(
	items: T[],
	sortMode: PivotSortMode,
	axis: 'row' | 'column'
): T[] {
	const totalSort =
		(axis === 'row' && (sortMode === 'row_total_desc' || sortMode === 'row_total_asc')) ||
		(axis === 'column' && (sortMode === 'column_total_desc' || sortMode === 'column_total_asc'));

	return [...items].sort((left, right) => {
		if (totalSort) {
			const totalDelta = sortMode.endsWith('_desc')
				? right.total - left.total
				: left.total - right.total;

			if (totalDelta !== 0) return totalDelta;
		}

		return left.label.localeCompare(right.label);
	});
}

export function pivotConfiguredBuckets(
	field: FormField | null
): Array<{ key: string; label: string }> {
	if (field?.type !== 'single_select' || !Array.isArray(field.options)) return [];

	const buckets = new Map<string, { key: string; label: string }>();

	for (const option of field.options) {
		const label = option.trim();

		if (!label) continue;

		const key = label.toLowerCase();

		if (!buckets.has(key)) buckets.set(key, { key, label });
	}

	return Array.from(buckets.values());
}

export function pivotTableMatrixRows(table: PivotTable): string[][] {
	if (!table.rowField || !table.columnField || !table.valueField) return [];

	const header = [
		fieldLabel(table.rowField),
		...table.columns.map((column) => column.label),
		...(pivotShowRowTotals(table.totalsMode) ? [translate('forms.pivotTotal')] : [])
	];

	const rows = table.rows.map((row) => [
		row.label,
		...table.columns.map((column) =>
			formatPivotDisplayValue(row.cells[column.key] ?? 0, row.total, column.total, table)
		),
		...(pivotShowRowTotals(table.totalsMode)
			? [formatPivotTotalDisplayValue(row.total, 'row', table)]
			: [])
	]);

	if (pivotShowColumnTotals(table.totalsMode)) {
		rows.push([
			translate('forms.pivotTotal'),
			...table.columns.map((column) => formatPivotTotalDisplayValue(column.total, 'column', table)),
			...(pivotShowRowTotals(table.totalsMode)
				? [formatPivotTotalDisplayValue(table.grandTotal, 'grand', table)]
				: [])
		]);
	}

	return [header, ...rows];
}

export function pivotTableMatrixText(table: PivotTable): string {
	return pivotTableMatrixRows(table)
		.map((row) => row.join('\t'))
		.join('\n');
}

export function pivotTableMatrixCsv(table: PivotTable): string {
	return pivotTableMatrixRows(table)
		.map((row) => row.map(csvCell).join(','))
		.join('\n');
}
