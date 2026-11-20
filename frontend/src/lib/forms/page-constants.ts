import type { FormFieldType, FormPermissionAction } from '#lib/api/forms.js';
import type { DetailLayoutMode, DetailPageWidgetKey } from '#lib/forms/page-types.js';

export const fieldTypeOptions: FormFieldType[] = [
	'text',
	'phone',
	'email',
	'address',
	'location',
	'scan',
	'signature',
	'autonumber',
	'member',
	'textarea',
	'rich_text',
	'number',
	'integer',
	'amount',
	'rating',
	'progress',
	'date',
	'datetime',
	'single_select',
	'multi_select',
	'boolean',
	'relation',
	'child_table',
	'attachment',
	'image',
	'formula',
	'ai_summary'
];

export const formPermissionActions: FormPermissionAction[] = [
	'form.view',
	'form.design',
	'record.create',
	'record.update',
	'record.delete',
	'record.export'
];

export const detailPageWidgetOptions: {
	key: DetailPageWidgetKey;
	labelKey: string;
	descriptionKey: string;
}[] = [
	{
		key: 'children',
		labelKey: 'forms.detailPageWidgetChildren',
		descriptionKey: 'forms.detailPageWidgetChildrenDescription'
	},

	{
		key: 'links',
		labelKey: 'forms.detailPageWidgetLinks',
		descriptionKey: 'forms.detailPageWidgetLinksDescription'
	},

	{
		key: 'comments',
		labelKey: 'forms.detailPageWidgetComments',
		descriptionKey: 'forms.detailPageWidgetCommentsDescription'
	},

	{
		key: 'attachments',
		labelKey: 'forms.detailPageWidgetAttachments',
		descriptionKey: 'forms.detailPageWidgetAttachmentsDescription'
	},

	{
		key: 'events',
		labelKey: 'forms.detailPageWidgetEvents',
		descriptionKey: 'forms.detailPageWidgetEventsDescription'
	}
];

export const detailPageWidgetKeys = new Set(detailPageWidgetOptions.map((option) => option.key));

export const detailLayoutModes: Array<{ key: DetailLayoutMode; labelKey: string }> = [
	{ key: 'header', labelKey: 'forms.detailHeader' },
	{ key: 'highlights', labelKey: 'forms.detailHighlights' },
	{ key: 'composition', labelKey: 'forms.detailPageComposition' },
	{ key: 'sections', labelKey: 'forms.detailLayoutSections' },
	{ key: 'widgets', labelKey: 'forms.detailPageWidgets' }
];

export const recordPageSize = 20;

export const SORTABLE_TYPES = new Set([
	'amount',
	'number',
	'integer',
	'boolean',
	'date',
	'datetime',
	'text',
	'textarea',
	'rich_text',
	'single_select',
	'ai_summary'
]);
