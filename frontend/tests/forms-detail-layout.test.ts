/**
 * Detail layout: sections, header fields and highlights as admins configure them.
 *
 * Run standalone: `bun tests/forms-detail-layout.test.ts`
 */

import type { FormField, FormRecord } from '../src/lib/api/forms';
import {
	buildDetailSections,
	headerFields,
	sectionFieldsForRecord
} from '../src/lib/forms/detail-layout';
import { Suite, assertDeepEqual, assertEqual, finish } from './support/harness';

const suite = new Suite('forms-detail-layout');
const labels = { main: 'Main', other: 'Other fields' };
const fields: FormField[] = [
	{ key: 'customer', type: 'text' },
	{ key: 'status', type: 'single_select' },
	{ key: 'total', type: 'amount' },
	{ key: 'notes', type: 'textarea' },
	{ key: 'secret', type: 'text', visibility: { detail: false } }
];
const keys = (list: FormField[]) => list.map((field) => field.key);

function record(values: Record<string, unknown>): FormRecord {
	return {
		id: 'r1',
		workspace_id: 'w',
		project_id: 'p',
		form_id: 'f',
		title: 'Record',
		values,
		source: {},
		created_at: '2026-01-01T00:00:00Z',
		updated_at: '2026-01-01T00:00:00Z'
	};
}

suite.check('no layout gives one Main section with every detail field', () => {
	const sections = buildDetailSections({ detail_layout: {} }, fields, labels);
	assertEqual(sections.length, 1, 'one section');
	assertEqual(sections[0].title, 'Main', 'title');
	assertDeepEqual(
		keys(sections[0].fields),
		['customer', 'status', 'total', 'notes'],
		'hidden field excluded'
	);
});

suite.check('sections keep their order and unplaced fields go to Other fields', () => {
	const sections = buildDetailSections(
		{
			detail_layout: {
				sections: [
					{
						key: 'pricing',
						title: 'Pricing',
						fields: ['total'],
						columns: 1,
						collapsible: true,
						default_collapsed: true
					},
					{ key: 'who', name: 'Who', fields: [{ key: 'customer' }, 'missing'] },
					{ key: 'empty', title: 'Empty', fields: ['missing'] }
				]
			}
		},
		fields,
		labels
	);
	assertDeepEqual(
		sections.map((section) => section.key),
		['pricing', 'who', 'additional'],
		'order'
	);
	assertEqual(sections[0].columns, 1, 'columns');
	assertEqual(sections[0].defaultCollapsed, true, 'collapsed');
	assertEqual(sections[1].title, 'Who', 'name fallback');
	assertEqual(sections[2].title, 'Other fields', 'other title');
	assertDeepEqual(keys(sections[2].fields), ['status', 'notes'], 'unplaced');
});

suite.check('default_collapsed needs collapsible', () => {
	const [section] = buildDetailSections(
		{
			detail_layout: { sections: [{ title: 'A', fields: ['customer'], default_collapsed: true }] }
		},
		fields,
		labels
	);
	assertEqual(section.defaultCollapsed, false, 'not collapsible');
	assertEqual(section.key, 'section_1', 'generated key');
});

suite.check('hide_empty_fields drops empty values for the record', () => {
	const [section] = buildDetailSections(
		{
			detail_layout: {
				sections: [{ title: 'A', fields: ['customer', 'status', 'total'], hide_empty_fields: true }]
			}
		},
		fields,
		labels
	);
	const shown = sectionFieldsForRecord(
		section,
		record({ customer: 'Lotus', status: '', total: { decimal: '' } })
	);
	assertDeepEqual(keys(shown), ['customer'], 'empty hidden');
});

suite.check('fields hidden by a condition are hidden', () => {
	const [section] = buildDetailSections({ detail_layout: {} }, fields, labels);
	const shown = sectionFieldsForRecord(section, record({}), (field) => field.key === 'notes');
	assertDeepEqual(keys(shown), ['customer', 'status', 'total'], 'conditional');
});

suite.check('header fields and at most four highlights', () => {
	const header = headerFields(
		{
			detail_layout: {
				header: { badge_field: 'status', subtitle_field: 'secret' },
				highlights: ['total', 'total', 'customer', 'notes', 'status', 'secret', 'missing']
			}
		},
		[...fields, { key: 'extra', type: 'text' }]
	);
	assertEqual(header.badge?.key, 'status', 'badge');
	assertEqual(header.subtitle, null, 'a field hidden from detail is not a subtitle');
	assertDeepEqual(keys(header.highlights), ['total', 'customer', 'notes', 'status'], 'highlights');
	assertDeepEqual(
		headerFields({ detail_layout: {} }, fields),
		{ badge: null, subtitle: null, highlights: [] },
		'none'
	);
});

finish(suite.result());
