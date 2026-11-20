import { formsApi } from '#lib/api/forms.js';
import { toast } from '#lib/stores/toast.js';
import type {
	DetailPageWidgetDensity,
	DetailPageWidgetEntry,
	DetailPageWidgetKey
} from '#lib/forms/page-types.js';
import {
	detailPageWidgetLimitText,
	detailPageWidgetPayload
} from '#lib/forms/detail-layout-drafts.js';
import { translate } from '#lib/forms/i18n.js';
import { isRecord } from '#lib/forms/record-format.js';
import { FormsController } from '#lib/forms/state/controller.js';

export class DetailLayoutSaveController extends FormsController {
	saveDetailHeader = async () => {
		if (!this.s.selectedForm) return;

		this.s.detailHeaderSaving = true;

		const response = await formsApi.update(this.s.selectedForm.id, {
			detail_layout: {
				...(this.s.selectedForm.detail_layout ?? {}),
				header: {
					subtitle_field: this.s.detailHeaderDraft.subtitleField || undefined,
					badge_field: this.s.detailHeaderDraft.badgeField || undefined
				}
			},
			expected_schema_version:
				this.s.formsSessionSchemaVersion ?? this.s.selectedForm.schema_version
		});

		this.s.detailHeaderSaving = false;

		if (response.code !== 0 || !response.data) {
			toast.error(response.message);

			return;
		}

		this.s.forms = this.s.forms.map((form) =>
			form.id === response.data?.id ? response.data : form
		);
		this.s.detailHeaderSyncedFingerprint = '';
		this.fp.detailLayoutEditor.syncDetailHeaderDraft(response.data);
		toast.success(translate('forms.detailHeaderSaved'));
	};

	saveDetailHighlights = async () => {
		if (!this.s.selectedForm) return;

		this.s.detailHighlightSaving = true;

		const response = await formsApi.update(this.s.selectedForm.id, {
			detail_layout: {
				...(this.s.selectedForm.detail_layout ?? {}),
				highlights: this.s.detailHighlightDraft.fields
			},
			expected_schema_version:
				this.s.formsSessionSchemaVersion ?? this.s.selectedForm.schema_version
		});

		this.s.detailHighlightSaving = false;

		if (response.code !== 0 || !response.data) {
			toast.error(response.message);

			return;
		}

		this.s.forms = this.s.forms.map((form) =>
			form.id === response.data?.id ? response.data : form
		);
		this.s.detailHighlightSyncedFingerprint = '';
		this.fp.detailLayoutEditor.syncDetailHighlightDraft(response.data);
		toast.success(translate('forms.detailHighlightsSaved'));
	};

	saveDetailPageComposition = async () => {
		if (!this.s.selectedForm) return;

		this.s.detailPageCompositionSaving = true;

		const response = await formsApi.update(this.s.selectedForm.id, {
			detail_layout: {
				...(this.s.selectedForm.detail_layout ?? {}),
				page: {
					...(isRecord(this.s.selectedForm.detail_layout?.page)
						? this.s.selectedForm.detail_layout.page
						: {}),
					widget_region: this.s.detailPageCompositionDraft.widgetRegion,
					field_columns: this.s.detailPageCompositionDraft.fieldColumns
				}
			},
			expected_schema_version:
				this.s.formsSessionSchemaVersion ?? this.s.selectedForm.schema_version
		});

		this.s.detailPageCompositionSaving = false;

		if (response.code !== 0 || !response.data) {
			toast.error(response.message);

			return;
		}

		this.s.forms = this.s.forms.map((form) =>
			form.id === response.data?.id ? response.data : form
		);
		this.s.detailPageCompositionSyncedFingerprint = '';
		this.fp.detailLayoutEditor.syncDetailPageCompositionDraft(response.data);
		toast.success(translate('forms.detailPageCompositionSaved'));
	};

	saveDetailLayoutSections = async () => {
		if (!this.s.selectedForm) return;

		const sections = this.fp.detailLayoutEditor.detailLayoutSectionPayload(
			this.s.detailLayoutSectionDrafts
		);

		this.s.detailLayoutSectionSaving = true;

		const response = await formsApi.update(this.s.selectedForm.id, {
			detail_layout: { ...(this.s.selectedForm.detail_layout ?? {}), sections },
			expected_schema_version:
				this.s.formsSessionSchemaVersion ?? this.s.selectedForm.schema_version
		});

		this.s.detailLayoutSectionSaving = false;

		if (response.code !== 0 || !response.data) {
			toast.error(response.message);

			return;
		}

		this.s.forms = this.s.forms.map((form) =>
			form.id === response.data?.id ? response.data : form
		);
		this.s.detailLayoutSectionSyncedFingerprint = '';
		this.fp.detailLayoutEditor.syncDetailLayoutSectionDrafts(response.data);
		toast.success(translate('forms.detailLayoutSectionsSaved'));
	};

	persistDetailPageWidgets = async (entries: DetailPageWidgetEntry[]) => {
		if (!this.s.selectedForm) return;

		this.s.detailPageWidgetSaving = true;

		const response = await formsApi.update(this.s.selectedForm.id, {
			detail_layout: {
				...(this.s.selectedForm.detail_layout ?? {}),
				widgets: detailPageWidgetPayload(entries)
			},
			expected_schema_version:
				this.s.formsSessionSchemaVersion ?? this.s.selectedForm.schema_version
		});

		this.s.detailPageWidgetSaving = false;

		if (response.code !== 0 || !response.data) {
			toast.error(response.message);

			return;
		}

		this.s.forms = this.s.forms.map((form) =>
			form.id === response.data?.id ? response.data : form
		);
		toast.success(translate('forms.detailPageWidgetsSaved'));
	};

	setDetailPageWidgetEnabled = async (key: DetailPageWidgetKey, enabled: boolean) => {
		const entries = this.s.selectedDetailPageWidgetEntries.map((entry) =>
			entry.key === key ? { ...entry, enabled } : entry
		);

		await this.persistDetailPageWidgets(entries);
	};

	setDetailPageWidgetTitle = async (key: DetailPageWidgetKey, title: string) => {
		const entries = this.s.selectedDetailPageWidgetEntries.map((entry) =>
			entry.key === key ? { ...entry, title } : entry
		);

		await this.persistDetailPageWidgets(entries);
	};

	setDetailPageWidgetDescription = async (key: DetailPageWidgetKey, description: string) => {
		const entries = this.s.selectedDetailPageWidgetEntries.map((entry) =>
			entry.key === key ? { ...entry, description } : entry
		);

		await this.persistDetailPageWidgets(entries);
	};

	setDetailPageWidgetEmptyText = async (key: DetailPageWidgetKey, emptyText: string) => {
		const entries = this.s.selectedDetailPageWidgetEntries.map((entry) =>
			entry.key === key ? { ...entry, emptyText } : entry
		);

		await this.persistDetailPageWidgets(entries);
	};

	setDetailPageWidgetShowCount = async (key: DetailPageWidgetKey, showCount: boolean) => {
		const entries = this.s.selectedDetailPageWidgetEntries.map((entry) =>
			entry.key === key ? { ...entry, showCount } : entry
		);

		await this.persistDetailPageWidgets(entries);
	};

	setDetailPageWidgetHideEmpty = async (key: DetailPageWidgetKey, hideEmpty: boolean) => {
		const entries = this.s.selectedDetailPageWidgetEntries.map((entry) =>
			entry.key === key ? { ...entry, hideEmpty } : entry
		);

		await this.persistDetailPageWidgets(entries);
	};

	setDetailPageWidgetCollapsible = async (key: DetailPageWidgetKey, collapsible: boolean) => {
		const entries = this.s.selectedDetailPageWidgetEntries.map((entry) =>
			entry.key === key
				? {
						...entry,
						collapsible,
						defaultCollapsed: collapsible ? entry.defaultCollapsed : false
					}
				: entry
		);

		await this.persistDetailPageWidgets(entries);
	};

	setDetailPageWidgetDefaultCollapsed = async (
		key: DetailPageWidgetKey,
		defaultCollapsed: boolean
	) => {
		const entries = this.s.selectedDetailPageWidgetEntries.map((entry) =>
			entry.key === key
				? {
						...entry,
						defaultCollapsed: entry.collapsible && defaultCollapsed
					}
				: entry
		);

		await this.persistDetailPageWidgets(entries);
	};

	setDetailPageWidgetDensity = async (
		key: DetailPageWidgetKey,
		density: DetailPageWidgetDensity
	) => {
		const entries = this.s.selectedDetailPageWidgetEntries.map((entry) =>
			entry.key === key ? { ...entry, density } : entry
		);

		await this.persistDetailPageWidgets(entries);
	};

	setDetailPageWidgetLimit = async (key: DetailPageWidgetKey, limit: string) => {
		const entries = this.s.selectedDetailPageWidgetEntries.map((entry) =>
			entry.key === key ? { ...entry, limit: detailPageWidgetLimitText(limit) } : entry
		);

		await this.persistDetailPageWidgets(entries);
	};

	moveDetailPageWidget = async (key: DetailPageWidgetKey, direction: -1 | 1) => {
		const entries = [...this.s.selectedDetailPageWidgetEntries];
		const index = entries.findIndex((entry) => entry.key === key);
		const nextIndex = index + direction;

		if (index < 0 || nextIndex < 0 || nextIndex >= entries.length) return;

		[entries[index], entries[nextIndex]] = [entries[nextIndex], entries[index]];
		await this.persistDetailPageWidgets(entries);
	};
}
