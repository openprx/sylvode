import type { FormRecord } from '#lib/api/forms.js';
import { toast } from '#lib/stores/toast.js';
import type {
	ParsedViewFilter,
	ViewFilterDraft,
	ViewFilterGroupDraft
} from '#lib/forms/page-types.js';
import { fieldLabel, fieldType } from '#lib/forms/field-options.js';
import { translate } from '#lib/forms/i18n.js';
import { NUMERIC_FIELD_TYPES, csvExportCell } from '#lib/utils/csv.js';
import { displayValue, recordMatchesViewFilter } from '#lib/forms/record-format.js';
import { FormsController } from '#lib/forms/state/controller.js';
import {
	filterOperatorSummary,
	viewDraftFilterExpressionRecordIdsText
} from '#lib/forms/view-draft-parse.js';

export class FilterExpressionExportController extends FormsController {
	parsedViewDraftFilter = (filter: ViewFilterDraft): ParsedViewFilter | null => {
		if (!filter.field) return null;
		if (filter.operator !== 'not_empty' && !filter.value.trim()) return null;

		const field = this.fp.viewRecords.fieldByKey(filter.field);

		if (!field) return null;

		return {
			field,
			operator: filter.operator,
			value: filter.operator === 'not_empty' ? '' : filter.value.trim()
		};
	};

	viewDraftFilterMatchCount = (filter: ViewFilterDraft): number | null => {
		const parsed = this.parsedViewDraftFilter(filter);

		if (!parsed) return null;

		return this.s.filterPreviewRecords.filter((record) => recordMatchesViewFilter(record, parsed))
			.length;
	};

	viewDraftFilterGroupMatchCount = (group: ViewFilterGroupDraft): number | null => {
		const parsedFilters = group.filters
			.filter((filter) => !filter.disabled)
			.map(this.parsedViewDraftFilter)
			.filter((filter): filter is ParsedViewFilter => Boolean(filter));

		if (parsedFilters.length === 0) return null;

		return this.s.filterPreviewRecords.filter((record) => {
			const matches = parsedFilters.map((filter) => recordMatchesViewFilter(record, filter));

			return group.logic === 'any' ? matches.some(Boolean) : matches.every(Boolean);
		}).length;
	};

	viewDraftFilterExpressionRecords = (): FormRecord[] | null => {
		const groups = this.fp.viewDraftFilters
			.visibleViewDraftFilterGroups()
			.filter((group) => !group.disabled)
			.map((group) => ({
				logic: group.logic,
				filters: group.filters
					.filter((filter) => !filter.disabled)
					.map(this.parsedViewDraftFilter)
					.filter((filter): filter is ParsedViewFilter => Boolean(filter))
			}))
			.filter((group) => group.filters.length > 0);

		if (groups.length === 0) return null;

		return this.s.filterPreviewRecords.filter((record) => {
			const groupMatches = groups.map((group) => {
				const matches = group.filters.map((filter) => recordMatchesViewFilter(record, filter));

				return group.logic === 'any' ? matches.some(Boolean) : matches.every(Boolean);
			});

			return this.s.viewDraft.filterRootLogic === 'any'
				? groupMatches.some(Boolean)
				: groupMatches.every(Boolean);
		});
	};

	recordsCsv = (records: FormRecord[]): string => {
		const exportFields =
			this.s.visibleFields.length > 0 ? this.s.visibleFields : this.s.fields.slice(0, 8);
		const header = ['Record ID', 'Title', ...exportFields.map(fieldLabel)]
			.map((cell) => csvExportCell(cell))
			.join(',');
		const numericColumns = [
			false,
			false,
			...exportFields.map((field) => NUMERIC_FIELD_TYPES.has(fieldType(field)))
		];

		const rows = records.map((record) =>
			[
				record.id,
				record.title,
				...exportFields.map((field) => {
					const value = displayValue(record.values[field.key]);

					return value === '-' ? '' : value;
				})
			]
				.map((cell, index) => csvExportCell(cell, numericColumns[index]))
				.join(',')
		);

		return [header, ...rows].join('\n');
	};

	viewDraftFilterExpressionCsv = (records: FormRecord[]): string => {
		return this.recordsCsv(records);
	};

	viewDraftFilterExpressionConfig = (): Record<string, unknown> | null => {
		return this.fp.viewDraftFilters.viewFilterExpressionConfigFromGroups(
			this.fp.viewDraftFilters.validViewFilterGroups()
		);
	};

	viewDraftFilterExpressionJsonText = (): string => {
		const expression = this.viewDraftFilterExpressionConfig();

		return expression ? JSON.stringify(expression, null, 2) : '';
	};

	viewDraftFilterExpressionSummaryText = (): string => {
		const groups = this.fp.viewDraftFilters.validViewFilterGroups();

		if (groups.length === 0) return '';

		const rootLogic = this.s.viewDraft.filterRootLogic === 'any' ? 'ANY group' : 'ALL groups';
		const lines = [`Filter expression: ${rootLogic}`];

		for (const [groupIndex, group] of groups.entries()) {
			const groupLogic = group.logic === 'any' ? 'ANY condition' : 'ALL conditions';

			lines.push(`Group ${groupIndex + 1}${group.disabled ? ' [disabled]' : ''}: ${groupLogic}`);

			for (const [filterIndex, filter] of group.filters.entries()) {
				const field = this.fp.viewRecords.fieldByKey(filter.field);
				const label = field ? fieldLabel(field) : filter.field;
				const value = filter.operator === 'not_empty' ? '' : ` "${filter.value}"`;

				lines.push(
					`  ${groupIndex + 1}.${filterIndex + 1}${filter.disabled ? ' [disabled]' : ''}: ${label} ${filterOperatorSummary(filter.operator)}${value}`
				);
			}
		}

		return lines.join('\n');
	};

	viewDraftFilterExpressionJson = (records: FormRecord[]): string => {
		return JSON.stringify(
			{
				form_id: this.s.selectedForm?.id ?? null,
				form_key: this.s.selectedForm?.key ?? null,
				record_count: records.length,
				record_ids: records.map((record) => record.id),
				filter_expression: this.viewDraftFilterExpressionConfig(),
				records: records.map((record) => this.fp.viewCards.cardRecordDetailsPayload(record))
			},
			null,
			2
		);
	};

	copyViewDraftFilterExpressionRecordIds = async (records: FormRecord[] | null) => {
		const recordIds = viewDraftFilterExpressionRecordIdsText(records);

		if (!recordIds) return;

		const clipboard = typeof navigator !== 'undefined' ? navigator.clipboard : null;

		try {
			if (clipboard?.writeText) {
				await clipboard.writeText(recordIds);
			}
		} catch {
			// Clipboard permissions are browser-dependent; keep filter preview operations non-blocking.
		}

		this.s.copiedFilterExpressionRecordIds = recordIds;
		toast.success(translate('forms.filterExpressionRecordIdsCopied'));
	};

	copyViewDraftFilterExpressionJson = async (expressionJson: string) => {
		if (!expressionJson) return;

		const clipboard = typeof navigator !== 'undefined' ? navigator.clipboard : null;

		try {
			if (clipboard?.writeText) {
				await clipboard.writeText(expressionJson);
			}
		} catch {
			// Clipboard permissions are browser-dependent; keep filter preview operations non-blocking.
		}

		this.s.copiedFilterExpressionJson = expressionJson;
		toast.success(translate('forms.filterExpressionJsonCopied'));
	};

	copyViewDraftFilterExpressionSummary = async (expressionSummary: string) => {
		if (!expressionSummary) return;

		const clipboard = typeof navigator !== 'undefined' ? navigator.clipboard : null;

		try {
			if (clipboard?.writeText) {
				await clipboard.writeText(expressionSummary);
			}
		} catch {
			// Clipboard permissions are browser-dependent; keep filter preview operations non-blocking.
		}

		this.s.copiedFilterExpressionSummary = expressionSummary;
		toast.success(translate('forms.filterExpressionSummaryCopied'));
	};

	copyViewDraftFilterExpressionJsonRecords = async (records: FormRecord[] | null) => {
		if (!records || records.length === 0) return;

		const json = this.viewDraftFilterExpressionJson(records);
		const clipboard = typeof navigator !== 'undefined' ? navigator.clipboard : null;

		try {
			if (clipboard?.writeText) {
				await clipboard.writeText(json);
			}
		} catch {
			// Clipboard permissions are browser-dependent; keep filter preview operations non-blocking.
		}

		this.s.copiedFilterExpressionJsonRecordIds = viewDraftFilterExpressionRecordIdsText(records);
		toast.success(translate('forms.filterExpressionRecordsJsonCopied'));
	};

	copyViewDraftFilterExpressionCsvRecords = async (records: FormRecord[] | null) => {
		if (!records || records.length === 0) return;

		const csv = this.viewDraftFilterExpressionCsv(records);
		const clipboard = typeof navigator !== 'undefined' ? navigator.clipboard : null;

		try {
			if (clipboard?.writeText) {
				await clipboard.writeText(csv);
			}
		} catch {
			// Clipboard permissions are browser-dependent; keep filter preview operations non-blocking.
		}

		this.s.copiedFilterExpressionCsvRecordIds = viewDraftFilterExpressionRecordIdsText(records);
		toast.success(translate('forms.filterExpressionRecordsCsvCopied'));
	};

	exportViewDraftFilterExpressionConfigJson = (expressionJson: string) => {
		if (!this.s.selectedForm || !expressionJson) return;

		const blob = new Blob([expressionJson], { type: 'application/json;charset=utf-8' });
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');

		anchor.href = url;
		anchor.download = `${this.s.selectedForm.key}-filter-expression.json`;
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);
		this.s.exportedFilterExpressionJson = expressionJson;
		toast.success(translate('forms.filterExpressionJsonExported'));
	};

	exportViewDraftFilterExpressionSummary = (expressionSummary: string) => {
		if (!this.s.selectedForm || !expressionSummary) return;

		const blob = new Blob([expressionSummary], { type: 'text/plain;charset=utf-8' });
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');

		anchor.href = url;
		anchor.download = `${this.s.selectedForm.key}-filter-expression-summary.txt`;
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);
		this.s.exportedFilterExpressionSummary = expressionSummary;
		toast.success(translate('forms.filterExpressionSummaryExported'));
	};

	focusViewDraftFilterExpressionRecords = (records: FormRecord[] | null) => {
		const recordIds = records?.map((record) => record.id) ?? [];

		if (recordIds.length === 0) return;

		this.s.focusedRecordIds = recordIds;
		toast.success(translate('forms.focusedRecords', { values: { count: recordIds.length } }));
	};

	exportViewDraftFilterExpressionCsv = (records: FormRecord[] | null) => {
		if (!this.s.selectedForm || !records || records.length === 0) return;

		const csv = this.viewDraftFilterExpressionCsv(records);
		const blob = new Blob([csv], { type: 'text/csv;charset=utf-8' });
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');

		anchor.href = url;
		anchor.download = `${this.s.selectedForm.key}-filter-expression-matches.csv`;
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);
		this.s.exportedFilterExpressionRecordIds = viewDraftFilterExpressionRecordIdsText(records);
		toast.success(translate('forms.filterExpressionRecordsExported'));
	};

	exportViewDraftFilterExpressionJson = (records: FormRecord[] | null) => {
		if (!this.s.selectedForm || !records || records.length === 0) return;

		const json = this.viewDraftFilterExpressionJson(records);
		const blob = new Blob([json], { type: 'application/json;charset=utf-8' });
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');

		anchor.href = url;
		anchor.download = `${this.s.selectedForm.key}-filter-expression-matches.json`;
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);
		this.s.exportedFilterExpressionJsonRecordIds = viewDraftFilterExpressionRecordIdsText(records);
		toast.success(translate('forms.filterExpressionRecordsJsonExported'));
	};
}
