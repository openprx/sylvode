import { copyText } from '#lib/clipboard.js';
import type { FormField, FormRecord, FormView } from '#lib/api/forms.js';
import * as precedence from '#lib/forms/view-precedence.js';
import { toast } from '#lib/stores/toast.js';
import { safeImageUrl } from '#lib/utils/safe-url.js';
import type { ViewFilterDraft, ViewFilterLogic } from '#lib/forms/page-types.js';
import { recordMediaUrl } from '#lib/forms/attachment-files.js';
import { translate } from '#lib/forms/i18n.js';
import { cardFieldHasDisplayValue, recordDisplay } from '#lib/forms/record-format.js';
import { FormsController } from '#lib/forms/state/controller.js';
import {
	orderedFieldsByKeys,
	viewCardFieldKeys,
	viewConfigString
} from '#lib/forms/view-config.js';
import { safeExportSlug } from '#lib/forms/zip.js';

export class ViewCardsController extends FormsController {
	copyRecordId = async (record: FormRecord) => {
		if (!(await copyText(record.id))) {
			toast.error(translate('forms.copyFailed'));
			return;
		}

		this.s.copiedCardRecordId = record.id;
		toast.success(translate('forms.recordIdCopied'));
	};

	cardRecordDetailsPayload = (record: FormRecord) => {
		return {
			id: record.id,
			form_id: record.form_id,
			form_key: this.s.selectedForm?.key ?? null,
			title: record.title,
			values: record.values,
			source: record.source,
			schema_version: record.schema_version ?? null,
			created_by: record.created_by ?? null,
			updated_by: record.updated_by ?? null,
			archived_at: record.archived_at ?? null,
			created_at: record.created_at,
			updated_at: record.updated_at
		};
	};

	cardRecordDetailsJson = (record: FormRecord): string => {
		return JSON.stringify(this.cardRecordDetailsPayload(record), null, 2);
	};

	exportCardRecordDetails = (record: FormRecord) => {
		if (!this.s.selectedForm) return;

		const blob = new Blob([this.cardRecordDetailsJson(record)], {
			type: 'application/json;charset=utf-8'
		});
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');

		anchor.href = url;
		anchor.download = `${this.s.selectedForm.key}-${record.id}-record.json`;
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);
		this.s.exportedCardRecordId = record.id;
		toast.success(translate('forms.recordExported'));
	};

	cardGroupRecordsDetailsJson = (group: {
		key: string;
		label: string;
		records: FormRecord[];
	}): string => {
		return JSON.stringify(
			{
				form_id: this.s.selectedForm?.id ?? null,
				form_key: this.s.selectedForm?.key ?? null,
				group_by: this.s.cardGroupField?.key ?? null,
				group_key: group.key,
				group_label: group.label,
				record_count: group.records.length,
				records: group.records.map((record) => this.cardRecordDetailsPayload(record))
			},
			null,
			2
		);
	};

	exportCardGroupRecords = (group: { key: string; label: string; records: FormRecord[] }) => {
		if (!this.s.selectedForm || group.records.length === 0) return;

		const blob = new Blob([this.cardGroupRecordsDetailsJson(group)], {
			type: 'application/json;charset=utf-8'
		});
		const url = URL.createObjectURL(blob);
		const anchor = document.createElement('a');

		anchor.href = url;
		anchor.download = `${this.s.selectedForm.key}-${this.s.cardGroupField?.key ?? 'group'}-${safeExportSlug(group.key)}-records.json`;
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		URL.revokeObjectURL(url);
		this.s.exportedCardGroupRecordsKey = this.cardGroupFilterKey(group);
		toast.success(
			translate('forms.cardGroupRecordsExported', { values: { count: group.records.length } })
		);
	};

	updateCardRecordSelectField = async (record: FormRecord, field: FormField, value: string) => {
		if (!value || record.values[field.key] === value) return;

		this.s.updatingCardRecordId = record.id;

		const response = await this.fp.session.updateRecordWithVersion(record.id, {
			values: { ...record.values, [field.key]: value },
			source: { type: 'web', origin: 'card' }
		});

		this.s.updatingCardRecordId = null;

		if (response.code !== 0 || !response.data) {
			toast.error(response.message);

			return;
		}

		this.fp.records.replaceLocalRecord(response.data);
		toast.success(translate('forms.cardFieldUpdated'));
		await this.fp.loaders.loadAggregates();
	};

	cardGroupCollapseKey = (group: { key: string }): string => {
		return `${this.s.selectedViewId || this.s.selectedView?.id || 'card'}:${group.key}`;
	};

	cardGroupCollapsed = (group: { key: string }): boolean => {
		return this.s.collapsedCardGroupKeys.includes(this.cardGroupCollapseKey(group));
	};

	toggleCardGroup = (group: { key: string }) => {
		const key = this.cardGroupCollapseKey(group);

		this.s.collapsedCardGroupKeys = this.s.collapsedCardGroupKeys.includes(key)
			? this.s.collapsedCardGroupKeys.filter((item) => item !== key)
			: [...this.s.collapsedCardGroupKeys, key];
	};

	cardGroupFilterKey = (group: { key: string; label: string }): string => {
		return `${this.s.cardGroupField?.key ?? '__none__'}:${group.key}:${group.label}`;
	};

	copyCardGroupRecordIds = async (group: { key: string; label: string; records: FormRecord[] }) => {
		if (group.records.length === 0) return;

		if (!(await copyText(group.records.map((record) => record.id).join('\n')))) {
			toast.error(translate('forms.copyFailed'));
			return;
		}

		this.s.copiedCardGroupRecordIdsKey = this.cardGroupFilterKey(group);
		toast.success(
			translate('forms.cardGroupRecordIdsCopied', { values: { count: group.records.length } })
		);
	};

	focusCardGroupRecords = (group: { records: FormRecord[] }) => {
		if (group.records.length === 0) return;

		this.s.focusedRecordIds = group.records.map((record) => record.id);

		this.fp.views.showView(precedence.ALL_RECORDS_VIEW_ID, { remember: false });

		toast.success(translate('forms.focusedRecords', { values: { count: group.records.length } }));
	};

	cardGroupFilterDraft = (group: { label: string }): ViewFilterDraft | null => {
		if (!this.s.cardGroupField) return null;

		return {
			field: this.s.cardGroupField.key,
			operator: 'equals',
			value: group.label
		};
	};

	canApplyCardGroupFilter = (group: { label: string }): boolean => {
		const filter = this.cardGroupFilterDraft(group);

		if (!filter) return false;

		const currentGroups = this.fp.viewDraftFilters
			.visibleViewDraftFilterGroups()
			.filter((item) => item.filters.length > 0);
		const currentFilterCount = currentGroups.reduce(
			(count, item) => count + item.filters.length,
			0
		);

		if (currentFilterCount >= 10) return false;
		if (currentGroups.length < 5) return true;

		const lastGroup = currentGroups[currentGroups.length - 1];

		return Boolean(lastGroup && lastGroup.filters.length < 5);
	};

	applyCardGroupFilter = (group: { key: string; label: string }) => {
		if (!this.canApplyCardGroupFilter(group)) return;

		const filter = this.cardGroupFilterDraft(group);

		if (!filter) return;

		const currentGroups = this.fp.viewDraftFilters
			.visibleViewDraftFilterGroups()
			.filter((item) => item.filters.length > 0)
			.map((item) => ({
				logic: item.logic,
				disabled: item.disabled || undefined,
				filters: item.filters.map((currentFilter) => ({ ...currentFilter }))
			}));

		const nextGroups =
			currentGroups.length === 0
				? [{ logic: 'all' as ViewFilterLogic, filters: [filter] }]
				: currentGroups.length < 5
					? [...currentGroups, { logic: 'all' as ViewFilterLogic, filters: [filter] }]
					: currentGroups.map((item, index) =>
							index === currentGroups.length - 1
								? { ...item, filters: [...item.filters, filter] }
								: item
						);

		this.fp.viewDraftFilters.setViewDraftFilterGroups(nextGroups);
		this.fp.viewDraftFilters.updateViewDraftFilterRootLogic('all');
		this.s.appliedCardGroupFilterKey = this.cardGroupFilterKey(group);
		toast.success(translate('forms.cardGroupFilterApplied'));
	};

	orderedCardFieldsForView = (view: FormView | null): FormField[] => {
		return orderedFieldsByKeys(this.s.visibleFields, viewCardFieldKeys(view));
	};

	cardDisplayFields = (record: FormRecord): FormField[] => {
		if (!this.s.selectedCardHideEmptyFields) return this.s.cardFields;

		return this.s.cardFields.filter((field) => cardFieldHasDisplayValue(record, field));
	};

	viewCardTitleField = (): FormField | null => {
		const configured = this.fp.viewRecords.fieldByKey(
			viewConfigString(this.s.selectedView, 'card_title_field')
		);

		if (configured) return configured;

		return null;
	};

	viewCardSubtitleField = (): FormField | null => {
		const configured = this.fp.viewRecords.fieldByKey(
			viewConfigString(this.s.selectedView, 'card_subtitle_field')
		);

		if (configured && configured.key !== this.viewCardTitleField()?.key) return configured;

		return null;
	};

	viewCardBadgeField = (): FormField | null => {
		const configured = this.fp.viewRecords.fieldByKey(
			viewConfigString(this.s.selectedView, 'card_badge_field')
		);
		const titleField = this.viewCardTitleField();
		const subtitleField = this.viewCardSubtitleField();

		if (configured && configured.key !== titleField?.key && configured.key !== subtitleField?.key) {
			return configured;
		}

		return null;
	};

	viewCardCoverField = (): FormField | null => {
		const configured = this.fp.viewRecords.fieldByKey(
			viewConfigString(this.s.selectedView, 'card_cover_field')
		);

		if (configured && ['image', 'attachment'].includes(configured.type ?? 'text'))
			return configured;

		return null;
	};

	cardTitle = (record: FormRecord): string => {
		const value = recordDisplay(record, this.viewCardTitleField());

		return value && value !== '-' ? value : record.title;
	};

	cardSubtitle = (record: FormRecord): string => {
		const value = recordDisplay(record, this.viewCardSubtitleField());

		return value && value !== '-' ? value : record.id.slice(0, 8);
	};

	cardBadge = (record: FormRecord): string => {
		const value = recordDisplay(record, this.viewCardBadgeField());

		return value && value !== '-' ? value : '';
	};

	cardCoverUrl = (record: FormRecord): string => {
		return safeImageUrl(recordMediaUrl(record, this.viewCardCoverField()));
	};
}
