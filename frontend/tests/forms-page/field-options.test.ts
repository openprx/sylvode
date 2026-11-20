/**
 * Field labels, types, options, placeholders and form summaries (`src/lib/forms/field-options.ts`).
 *
 * Run standalone: `bun tests/forms-page/field-options.test.ts`
 */
import { Suite, assertDeepEqual, assertEqual, finish } from '../support/harness';
import { installSvelteSsr } from '../support/svelte-ssr';
import type { FormField, UniversalForm } from '../../src/lib/api/forms';

await installSvelteSsr();
const m = await import('../../src/lib/forms/field-options');

const suite = new Suite('forms-page/field-options');

function field(key: string, extra: Partial<FormField> = {}): FormField {
	return { key, type: 'text', ...extra };
}

function form(fields: Array<Record<string, unknown>>): UniversalForm {
	return {
		id: 'form-1',
		workspace_id: 'ws',
		project_id: 'pr',
		key: 'orders',
		name: 'Orders',
		description: '',
		title_template: '{id}',
		schema: { fields },
		detail_layout: {},
		schema_version: 1,
		created_at: '2026-01-01T00:00:00Z',
		updated_at: '2026-01-01T00:00:00Z'
	};
}

suite.check('fieldLabel uses the trimmed label or a humanised key', () => {
	assertEqual(
		m.fieldLabel(field('order_no', { label: '  Order number ' })),
		'Order number',
		'label'
	);
	assertEqual(m.fieldLabel(field('order_no')), 'order no', 'key without label');
	assertEqual(m.fieldLabel(field('order_no', { label: '   ' })), 'order no', 'blank label');
	assertEqual(m.fieldLabel(field('a_b_c', { label: '' })), 'a b c', 'every underscore replaced');
});

suite.check('fieldType defaults to text', () => {
	assertEqual(m.fieldType({ key: 'a' }), 'text', 'absent');
	assertEqual(m.fieldType({ key: 'a', type: 'amount' }), 'amount', 'present');
});

suite.check('option list readers default to empty arrays', () => {
	assertDeepEqual(m.fieldOptions({ key: 'a' }), [], 'options absent');
	assertDeepEqual(m.fieldOptions(field('a', { options: ['x', 'y'] })), ['x', 'y'], 'options');
	assertDeepEqual(m.fieldDisabledOptions({ key: 'a' }), [], 'disabled absent');
	assertDeepEqual(
		m.fieldDisabledOptions(field('a', { option_disabled: ['x'] })),
		['x'],
		'disabled'
	);
	assertDeepEqual(m.fieldArchivedOptions({ key: 'a' }), [], 'archived absent');
	assertDeepEqual(
		m.fieldArchivedOptions(field('a', { option_archived: ['y'] })),
		['y'],
		'archived'
	);
});

suite.check('optionDisabled and optionArchived test membership', () => {
	const f = field('a', {
		options: ['x', 'y', 'z'],
		option_disabled: ['x'],
		option_archived: ['y']
	});
	assertEqual(m.optionDisabled(f, 'x'), true, 'disabled hit');
	assertEqual(m.optionDisabled(f, 'y'), false, 'disabled miss');
	assertEqual(m.optionArchived(f, 'y'), true, 'archived hit');
	assertEqual(m.optionArchived(f, 'x'), false, 'archived miss');
	assertEqual(m.optionDisabled({ key: 'a' }, 'x'), false, 'no list');
	assertEqual(m.optionArchived({ key: 'a' }, 'x'), false, 'no list');
});

suite.check('recordSelectOptions hides archived options unless selected', () => {
	const f = field('k', { options: ['a', 'b', 'c'], option_archived: ['b', 'c'] });
	assertDeepEqual(m.recordSelectOptions(f, {}), ['a'], 'nothing selected');
	assertDeepEqual(m.recordSelectOptions(f, { k: 'b' }), ['a', 'b'], 'selected string');
	assertDeepEqual(
		m.recordSelectOptions(f, { k: ['c', 7, 'zzz'] }),
		['a', 'c'],
		'selected array, non-strings ignored'
	);
	assertDeepEqual(m.recordSelectOptions(f, { other: 'b' }), ['a'], 'other field selected');
	assertDeepEqual(m.recordSelectOptions(f, { k: 5 }), ['a'], 'non-string value');
	assertDeepEqual(
		m.recordSelectOptions(field('k', { options: ['a', 'b'] }), {}),
		['a', 'b'],
		'no archived options'
	);
	assertDeepEqual(m.recordSelectOptions({ key: 'k' }, {}), [], 'no options');
});

suite.check('optionDescription and optionGroup trim and fall back to the trimmed option', () => {
	const f = field('k', {
		option_descriptions: { a: ' Alpha ', b: '   ' },
		option_groups: { a: ' Group ', c: 'C' }
	});
	assertEqual(m.optionDescription(f, 'a'), 'Alpha', 'description');
	assertEqual(m.optionDescription(f, ' a '), 'Alpha', 'option is looked up trimmed');
	assertEqual(m.optionDescription(f, 'b'), '', 'blank description');
	assertEqual(m.optionDescription(f, 'zz'), '', 'unknown option');
	assertEqual(m.optionDescription({ key: 'k' }, 'a'), '', 'no descriptions');
	assertEqual(m.optionGroup(f, 'a'), 'Group', 'group');
	assertEqual(m.optionGroup(f, ' c '), 'C', 'trimmed lookup');
	assertEqual(m.optionGroup(f, 'zz'), '', 'unknown option');
	assertEqual(m.optionGroup({ key: 'k' }, 'a'), '', 'no groups');
});

suite.check('optionDisplayLabel prefixes the group', () => {
	const f = field('k', { option_groups: { a: 'Active' } });
	assertEqual(m.optionDisplayLabel(f, 'a'), 'Active / a', 'grouped');
	assertEqual(m.optionDisplayLabel(f, 'b'), 'b', 'ungrouped');
});

suite.check('sanitizeOptionColor accepts only 3 or 6 digit hex colours', () => {
	const table: Array<[unknown, string]> = [
		['#fff', '#fff'],
		['#FFFFFF', '#FFFFFF'],
		['#ff00aa', '#ff00aa'],
		[' #abc ', '#abc'],
		['#abcd', ''],
		['#abcde', ''],
		['#1234567', ''],
		['#12345g', ''],
		['fff', ''],
		['red', ''],
		['rgb(0,0,0)', ''],
		['', ''],
		[5, ''],
		[null, ''],
		[undefined, '']
	];
	for (const [input, expected] of table) {
		assertEqual(m.sanitizeOptionColor(input), expected, `colour ${JSON.stringify(input)}`);
	}
});

suite.check('fieldPlaceholder picks explicit text, currency or option list', () => {
	assertEqual(
		m.fieldPlaceholder(field('a', { placeholder: 'Type here' })),
		'Type here',
		'explicit'
	);
	assertEqual(m.fieldPlaceholder(field('a')), '', 'text without placeholder');
	assertEqual(
		m.fieldPlaceholder(field('a', { type: 'amount', amount: { currency: 'USD' } })),
		'USD',
		'amount currency'
	);
	assertEqual(m.fieldPlaceholder(field('a', { type: 'amount' })), 'CNY', 'amount default currency');
	assertEqual(
		m.fieldPlaceholder(field('a', { type: 'multi_select', options: ['x', 'y'] })),
		'x, y',
		'multi select lists options'
	);
	assertEqual(
		m.fieldPlaceholder(field('a', { type: 'multi_select' })),
		'',
		'multi select without options'
	);
	assertEqual(
		m.fieldPlaceholder(
			field('a', { type: 'amount', placeholder: 'Total', amount: { currency: 'USD' } })
		),
		'Total',
		'explicit beats currency'
	);
	assertEqual(
		m.fieldPlaceholder(field('a', { type: 'single_select', options: ['x'] })),
		'',
		'single select'
	);
	assertEqual(m.fieldPlaceholder({ key: 'a' }), '', 'type absent');
});

suite.check('formListFields keeps parsed fields that are not hidden from lists', () => {
	const f = form([
		{ key: 'a' },
		{ key: 'b', visibility: { list: false } },
		{ key: 'c', visibility: { list: true } },
		{ key: 'd', visibility: { detail: false } },
		{ label: 'no key' }
	]);
	assertDeepEqual(
		m.formListFields(f).map((item) => item.key),
		['a', 'c', 'd'],
		'visible keys'
	);
	assertDeepEqual(m.formListFields(form([])), [], 'no fields');
});

suite.check('formFieldSummary joins the first three labels', () => {
	const f = form([
		{ key: 'order_no', label: 'Order' },
		{ key: 'hidden', visibility: { list: false } },
		{ key: 'customer_name' },
		{ key: 'qty', label: ' Quantity ' },
		{ key: 'extra', label: 'Extra' }
	]);
	assertEqual(m.formFieldSummary(f), 'Order / customer name / Quantity', 'three visible labels');
	assertEqual(m.formFieldSummary(form([{ key: 'a', label: 'A' }])), 'A', 'one label');
	assertEqual(m.formFieldSummary(form([])), 'forms.noFields', 'no fields');
	assertEqual(
		m.formFieldSummary(form([{ key: 'a', visibility: { list: false } }])),
		'forms.noFields',
		'all hidden'
	);
});

finish(suite.result());
