import type { UniversalForm } from '#lib/api/forms.js';
import type {
	DetailHeaderDraft,
	DetailHighlightDraft,
	DetailLayoutSectionDensity,
	DetailLayoutSectionDraft,
	DetailPageCompositionDraft
} from '#lib/forms/page-types.js';
import { parseFields } from '#lib/forms/designer-schema.js';
import {
	detailLayoutSectionDescription,
	detailPageFieldColumns,
	detailPageWidgetRegion
} from '#lib/forms/detail-layout-drafts.js';
import {
	detailLayoutFieldKeys,
	detailSectionColumns as detailLayoutSectionColumns
} from '#lib/forms/detail-layout.js';
import { translate } from '#lib/forms/i18n.js';
import { isRecord } from '#lib/forms/record-format.js';
import { FormsController } from '#lib/forms/state/controller.js';

export class DetailLayoutEditorController extends FormsController {
	detailLayoutFingerprint = (form: UniversalForm | null = this.s.selectedForm): string => {
		if (!form) return '';

		return `${form.id}:${form.schema_version}:${JSON.stringify(form.detail_layout?.sections ?? [])}`;
	};

	detailHeaderFingerprint = (form: UniversalForm | null = this.s.selectedForm): string => {
		if (!form) return '';

		return `${form.id}:${form.schema_version}:${JSON.stringify(form.detail_layout?.header ?? {})}`;
	};

	detailHighlightFingerprint = (form: UniversalForm | null = this.s.selectedForm): string => {
		if (!form) return '';

		return `${form.id}:${form.schema_version}:${JSON.stringify(form.detail_layout?.highlights ?? [])}`;
	};

	detailPageCompositionFingerprint = (form: UniversalForm | null = this.s.selectedForm): string => {
		if (!form) return '';

		return `${form.id}:${form.schema_version}:${JSON.stringify(form.detail_layout?.page ?? {})}`;
	};

	detailPageCompositionDraftFromForm = (
		form: UniversalForm | null = this.s.selectedForm
	): DetailPageCompositionDraft => {
		if (!form) return { widgetRegion: 'sidebar', fieldColumns: 2 };

		const page = isRecord(form.detail_layout?.page) ? form.detail_layout.page : {};

		return {
			widgetRegion: detailPageWidgetRegion(page.widget_region),
			fieldColumns: detailPageFieldColumns(page.field_columns)
		};
	};

	syncDetailPageCompositionDraft = (form: UniversalForm | null = this.s.selectedForm) => {
		const fingerprint = this.detailPageCompositionFingerprint(form);

		if (!form) {
			this.s.detailPageCompositionDraft = { widgetRegion: 'sidebar', fieldColumns: 2 };
			this.s.detailPageCompositionSyncedFingerprint = '';

			return;
		}

		if (this.s.detailPageCompositionSyncedFingerprint === fingerprint) return;

		this.s.detailPageCompositionDraft = this.detailPageCompositionDraftFromForm(form);
		this.s.detailPageCompositionSyncedFingerprint = fingerprint;
	};

	detailHeaderDraftFromForm = (
		form: UniversalForm | null = this.s.selectedForm
	): DetailHeaderDraft => {
		if (!form) return { subtitleField: '', badgeField: '' };

		const fieldKeys = new Set(parseFields(form.schema).map((field) => field.key));
		const header = isRecord(form.detail_layout?.header) ? form.detail_layout.header : {};
		const subtitleField =
			typeof header.subtitle_field === 'string' && fieldKeys.has(header.subtitle_field)
				? header.subtitle_field
				: '';
		const badgeField =
			typeof header.badge_field === 'string' && fieldKeys.has(header.badge_field)
				? header.badge_field
				: '';

		return { subtitleField, badgeField };
	};

	syncDetailHeaderDraft = (form: UniversalForm | null = this.s.selectedForm) => {
		const fingerprint = this.detailHeaderFingerprint(form);

		if (!form) {
			this.s.detailHeaderDraft = { subtitleField: '', badgeField: '' };
			this.s.detailHeaderSyncedFingerprint = '';

			return;
		}

		if (this.s.detailHeaderSyncedFingerprint === fingerprint) return;

		this.s.detailHeaderDraft = this.detailHeaderDraftFromForm(form);
		this.s.detailHeaderSyncedFingerprint = fingerprint;
	};

	detailHighlightDraftFromForm = (
		form: UniversalForm | null = this.s.selectedForm
	): DetailHighlightDraft => {
		if (!form) return { fields: [] };

		const fields = parseFields(form.schema).filter((field) => field.visibility?.detail !== false);
		const validFieldKeys = new Set(fields.map((field) => field.key));
		const rawHighlights = Array.isArray(form.detail_layout?.highlights)
			? form.detail_layout.highlights
			: [];
		const highlightFields: string[] = [];

		for (const fieldKey of rawHighlights) {
			if (
				typeof fieldKey === 'string' &&
				validFieldKeys.has(fieldKey) &&
				!highlightFields.includes(fieldKey)
			) {
				highlightFields.push(fieldKey);
			}
		}

		return { fields: highlightFields.slice(0, 4) };
	};

	syncDetailHighlightDraft = (form: UniversalForm | null = this.s.selectedForm) => {
		const fingerprint = this.detailHighlightFingerprint(form);

		if (!form) {
			this.s.detailHighlightDraft = { fields: [] };
			this.s.detailHighlightSyncedFingerprint = '';

			return;
		}

		if (this.s.detailHighlightSyncedFingerprint === fingerprint) return;

		this.s.detailHighlightDraft = this.detailHighlightDraftFromForm(form);
		this.s.detailHighlightSyncedFingerprint = fingerprint;
	};

	detailLayoutSectionDraftsFromForm = (
		form: UniversalForm | null = this.s.selectedForm
	): DetailLayoutSectionDraft[] => {
		if (!form) return [];

		const detailFields = parseFields(form.schema).filter(
			(field) => field.visibility?.detail !== false
		);
		const detailFieldKeys = detailFields.map((field) => field.key);
		const validFieldKeys = new Set(detailFieldKeys);
		const usedFieldKeys = new Set<string>();
		const rawSections = Array.isArray(form.detail_layout?.sections)
			? form.detail_layout.sections
			: [];

		const drafts = rawSections
			.filter(isRecord)
			.map((section, index) => {
				const key =
					typeof section.key === 'string' && section.key.trim().length > 0
						? section.key.trim()
						: `section_${index + 1}`;

				const title =
					typeof section.title === 'string' && section.title.trim().length > 0
						? section.title.trim()
						: typeof section.name === 'string' && section.name.trim().length > 0
							? section.name.trim()
							: `${translate('forms.detailSectionMain')} ${index + 1}`;

				const description = detailLayoutSectionDescription(section.description);
				const columns = detailLayoutSectionColumns(section.columns);
				const density: DetailLayoutSectionDensity =
					section.density === 'compact' ? 'compact' : 'comfortable';
				const hideEmptyFields = section.hide_empty_fields === true;
				const collapsible = section.collapsible === true;
				const defaultCollapsed = collapsible && section.default_collapsed === true;

				const sectionFields = detailLayoutFieldKeys(section).filter((fieldKey) => {
					if (!validFieldKeys.has(fieldKey) || usedFieldKeys.has(fieldKey)) return false;

					usedFieldKeys.add(fieldKey);

					return true;
				});

				return {
					key,
					title,
					description,
					columns,
					density,
					hideEmptyFields,
					collapsible,
					defaultCollapsed,
					fields: sectionFields
				};
			})
			.filter(
				(section) =>
					section.fields.length > 0 ||
					section.title.trim().length > 0 ||
					section.description.trim().length > 0
			);

		const unplacedFields = detailFieldKeys.filter((fieldKey) => !usedFieldKeys.has(fieldKey));

		if (drafts.length === 0) {
			return [
				{
					key: 'main',
					title: translate('forms.detailSectionMain'),
					description: '',
					columns: 2,
					density: 'comfortable',
					hideEmptyFields: false,
					collapsible: false,
					defaultCollapsed: false,
					fields: detailFieldKeys
				}
			];
		}

		if (unplacedFields.length > 0) {
			drafts.push({
				key: this.nextDetailLayoutSectionKey(drafts, 'additional'),
				title: translate('forms.detailSectionAdditional'),
				description: '',
				columns: 2,
				density: 'comfortable',
				hideEmptyFields: false,
				collapsible: false,
				defaultCollapsed: false,
				fields: unplacedFields
			});
		}

		return drafts;
	};

	syncDetailLayoutSectionDrafts = (form: UniversalForm | null = this.s.selectedForm) => {
		const fingerprint = this.detailLayoutFingerprint(form);

		if (!form) {
			this.s.detailLayoutSectionDrafts = [];
			this.s.detailLayoutSectionSyncedFingerprint = '';

			return;
		}

		if (this.s.detailLayoutSectionSyncedFingerprint === fingerprint) return;

		this.s.detailLayoutSectionDrafts = this.detailLayoutSectionDraftsFromForm(form);
		this.s.detailLayoutSectionSyncedFingerprint = fingerprint;
	};

	nextDetailLayoutSectionKey = (
		sections: DetailLayoutSectionDraft[] = this.s.detailLayoutSectionDrafts,
		prefix = 'section'
	): string => {
		const used = new Set(sections.map((section) => section.key));
		let index = sections.length + 1;
		let key = `${prefix}_${index}`;

		while (used.has(key)) {
			index += 1;
			key = `${prefix}_${index}`;
		}

		return key;
	};

	detailLayoutSectionPayload = (sections: DetailLayoutSectionDraft[]) => {
		const validFieldKeys = new Set(
			this.s.fields.filter((field) => field.visibility?.detail !== false).map((field) => field.key)
		);
		const usedFieldKeys = new Set<string>();

		return sections
			.map((section, index) => {
				const sectionFields = section.fields.filter((fieldKey) => {
					if (!validFieldKeys.has(fieldKey) || usedFieldKeys.has(fieldKey)) return false;

					usedFieldKeys.add(fieldKey);

					return true;
				});

				return {
					key: section.key.trim() || `section_${index + 1}`,
					title: section.title.trim() || `${translate('forms.detailSectionMain')} ${index + 1}`,
					...(section.description.trim() ? { description: section.description.trim() } : {}),
					...(section.columns !== 2 ? { columns: section.columns } : {}),
					...(section.density === 'compact' ? { density: 'compact' } : {}),
					...(section.hideEmptyFields ? { hide_empty_fields: true } : {}),
					...(section.collapsible ? { collapsible: true } : {}),
					...(section.collapsible && section.defaultCollapsed ? { default_collapsed: true } : {}),
					fields: sectionFields
				};
			})
			.filter((section) => section.fields.length > 0);
	};

	detailLayoutFieldAssignedTo = (fieldKey: string): string => {
		return (
			this.s.detailLayoutSectionDrafts.find((section) => section.fields.includes(fieldKey))?.key ??
			''
		);
	};

	updateDetailLayoutSectionTitle = (index: number, title: string) => {
		this.s.detailLayoutSectionDrafts = this.s.detailLayoutSectionDrafts.map(
			(section, sectionIndex) => (sectionIndex === index ? { ...section, title } : section)
		);
	};

	updateDetailLayoutSectionDescription = (index: number, description: string) => {
		this.s.detailLayoutSectionDrafts = this.s.detailLayoutSectionDrafts.map(
			(section, sectionIndex) => (sectionIndex === index ? { ...section, description } : section)
		);
	};

	updateDetailLayoutSectionColumns = (index: number, columns: 1 | 2 | 3) => {
		this.s.detailLayoutSectionDrafts = this.s.detailLayoutSectionDrafts.map(
			(section, sectionIndex) => (sectionIndex === index ? { ...section, columns } : section)
		);
	};

	updateDetailLayoutSectionDensity = (index: number, density: DetailLayoutSectionDensity) => {
		this.s.detailLayoutSectionDrafts = this.s.detailLayoutSectionDrafts.map(
			(section, sectionIndex) => (sectionIndex === index ? { ...section, density } : section)
		);
	};

	updateDetailLayoutSectionHideEmptyFields = (index: number, hideEmptyFields: boolean) => {
		this.s.detailLayoutSectionDrafts = this.s.detailLayoutSectionDrafts.map(
			(section, sectionIndex) =>
				sectionIndex === index ? { ...section, hideEmptyFields } : section
		);
	};

	updateDetailLayoutSectionCollapsible = (index: number, collapsible: boolean) => {
		this.s.detailLayoutSectionDrafts = this.s.detailLayoutSectionDrafts.map(
			(section, sectionIndex) =>
				sectionIndex === index
					? {
							...section,
							collapsible,
							defaultCollapsed: collapsible ? section.defaultCollapsed : false
						}
					: section
		);
	};

	updateDetailLayoutSectionDefaultCollapsed = (index: number, defaultCollapsed: boolean) => {
		this.s.detailLayoutSectionDrafts = this.s.detailLayoutSectionDrafts.map(
			(section, sectionIndex) =>
				sectionIndex === index
					? {
							...section,
							defaultCollapsed: section.collapsible && defaultCollapsed
						}
					: section
		);
	};

	addDetailLayoutSection = () => {
		const key = this.nextDetailLayoutSectionKey();

		this.s.detailLayoutSectionDrafts = [
			...this.s.detailLayoutSectionDrafts,
			{
				key,
				title: `${translate('forms.detailSectionMain')} ${this.s.detailLayoutSectionDrafts.length + 1}`,
				description: '',
				columns: 2,
				density: 'comfortable',
				hideEmptyFields: false,
				collapsible: false,
				defaultCollapsed: false,
				fields: []
			}
		];
	};

	removeDetailLayoutSection = (index: number) => {
		if (this.s.detailLayoutSectionDrafts.length <= 1) return;

		this.s.detailLayoutSectionDrafts = this.s.detailLayoutSectionDrafts.filter(
			(_, sectionIndex) => sectionIndex !== index
		);
	};

	moveDetailLayoutSection = (index: number, direction: -1 | 1) => {
		const nextIndex = index + direction;

		if (nextIndex < 0 || nextIndex >= this.s.detailLayoutSectionDrafts.length) return;

		const next = [...this.s.detailLayoutSectionDrafts];

		[next[index], next[nextIndex]] = [next[nextIndex], next[index]];
		this.s.detailLayoutSectionDrafts = next;
	};

	setDetailLayoutSectionField = (index: number, fieldKey: string, checked: boolean) => {
		this.s.detailLayoutSectionDrafts = this.s.detailLayoutSectionDrafts.map(
			(section, sectionIndex) => {
				const withoutField = section.fields.filter((key) => key !== fieldKey);

				if (sectionIndex !== index) return { ...section, fields: withoutField };

				return {
					...section,
					fields: checked ? [...withoutField, fieldKey] : withoutField
				};
			}
		);
	};

	moveDetailLayoutSectionField = (index: number, fieldKey: string, direction: -1 | 1) => {
		const section = this.s.detailLayoutSectionDrafts[index];

		if (!section) return;

		const fieldIndex = section.fields.indexOf(fieldKey);
		const nextFieldIndex = fieldIndex + direction;

		if (fieldIndex < 0 || nextFieldIndex < 0 || nextFieldIndex >= section.fields.length) return;

		const nextFields = [...section.fields];

		[nextFields[fieldIndex], nextFields[nextFieldIndex]] = [
			nextFields[nextFieldIndex],
			nextFields[fieldIndex]
		];
		this.s.detailLayoutSectionDrafts = this.s.detailLayoutSectionDrafts.map((item, sectionIndex) =>
			sectionIndex === index ? { ...item, fields: nextFields } : item
		);
	};

	setDetailHeaderDraft = (patch: Partial<DetailHeaderDraft>) => {
		this.s.detailHeaderDraft = { ...this.s.detailHeaderDraft, ...patch };
	};

	setDetailHighlightField = (fieldKey: string, checked: boolean) => {
		const nextFields = this.s.detailHighlightDraft.fields.filter((key) => key !== fieldKey);

		if (checked && nextFields.length < 4) nextFields.push(fieldKey);

		this.s.detailHighlightDraft = { fields: nextFields };
	};

	moveDetailHighlightField = (fieldKey: string, direction: -1 | 1) => {
		const fields = [...this.s.detailHighlightDraft.fields];
		const index = fields.indexOf(fieldKey);
		const nextIndex = index + direction;

		if (index < 0 || nextIndex < 0 || nextIndex >= fields.length) return;

		[fields[index], fields[nextIndex]] = [fields[nextIndex], fields[index]];
		this.s.detailHighlightDraft = { fields };
	};
}
