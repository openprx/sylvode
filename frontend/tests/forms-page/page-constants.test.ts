/**
 * Option lists and constants of the forms page. They are data, so the tests pin the exact
 * contents that other code (designer, permissions, detail layout, list paging) depends on.
 *
 * Run standalone: `bun tests/forms-page/page-constants.test.ts`
 */
import { Suite, assert, assertDeepEqual, assertEqual, finish } from '../support/harness';
import { installSvelteSsr } from '../support/svelte-ssr';

await installSvelteSsr();
const constants = await import('../../src/lib/forms/page-constants');

const suite = new Suite('forms-page/page-constants');

suite.check(
	'fieldTypeOptions lists the 27 field types in designer order without duplicates',
	() => {
		assertDeepEqual(
			constants.fieldTypeOptions,
			[
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
			],
			'field types'
		);
		assertEqual(constants.fieldTypeOptions.length, 27, 'count');
		assertEqual(new Set(constants.fieldTypeOptions).size, 27, 'no duplicates');
	}
);

suite.check('formPermissionActions lists the six actions', () => {
	assertDeepEqual(
		constants.formPermissionActions,
		[
			'form.view',
			'form.design',
			'record.create',
			'record.update',
			'record.delete',
			'record.export'
		],
		'actions'
	);
});

suite.check(
	'detailPageWidgetOptions pairs every widget with its label and description keys',
	() => {
		assertDeepEqual(
			constants.detailPageWidgetOptions,
			[
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
			],
			'widget options'
		);
	}
);

suite.check('detailPageWidgetKeys is the set of the option keys', () => {
	assert(constants.detailPageWidgetKeys instanceof Set, 'a Set');
	assertEqual(constants.detailPageWidgetKeys.size, 5, 'five keys');
	assertDeepEqual(
		Array.from(constants.detailPageWidgetKeys),
		constants.detailPageWidgetOptions.map((option) => option.key),
		'same keys in the same order'
	);
	assert(constants.detailPageWidgetKeys.has('comments'), 'has comments');
	assert(!constants.detailPageWidgetKeys.has('bogus' as never), 'does not have an unknown key');
});

suite.check('detailLayoutModes lists the five editor modes in order', () => {
	assertDeepEqual(
		constants.detailLayoutModes,
		[
			{ key: 'header', labelKey: 'forms.detailHeader' },
			{ key: 'highlights', labelKey: 'forms.detailHighlights' },
			{ key: 'composition', labelKey: 'forms.detailPageComposition' },
			{ key: 'sections', labelKey: 'forms.detailLayoutSections' },
			{ key: 'widgets', labelKey: 'forms.detailPageWidgets' }
		],
		'modes'
	);
});

suite.check('recordPageSize is 20', () => {
	assertEqual(constants.recordPageSize, 20, 'page size');
});

suite.check('SORTABLE_TYPES holds exactly the eleven sortable field types', () => {
	assertDeepEqual(
		Array.from(constants.SORTABLE_TYPES).sort(),
		[
			'ai_summary',
			'amount',
			'boolean',
			'date',
			'datetime',
			'integer',
			'number',
			'rich_text',
			'single_select',
			'text',
			'textarea'
		],
		'sortable types'
	);
	for (const type of [
		'multi_select',
		'relation',
		'child_table',
		'attachment',
		'image',
		'formula',
		'signature'
	]) {
		assert(!constants.SORTABLE_TYPES.has(type), `${type} is not sortable`);
	}
	for (const type of constants.SORTABLE_TYPES) {
		assert(
			(constants.fieldTypeOptions as string[]).includes(type),
			`${type} is one of the designer field types`
		);
	}
});

finish(suite.result());
