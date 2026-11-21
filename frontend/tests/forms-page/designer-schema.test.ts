/**
 * Schema drafts, option and variant text, formulas and defaults (`src/lib/forms/designer-schema.ts`).
 *
 * Run standalone: `bun tests/forms-page/designer-schema.test.ts`
 */
import { Suite, assert, assertDeepEqual, assertEqual, finish } from '../support/harness';
import { installSvelteSsr } from '../support/svelte-ssr';
import type { ScenarioTemplate } from '../../src/lib/api/projects';

await installSvelteSsr();
const m = await import('../../src/lib/forms/designer-schema');

const suite = new Suite('forms-page/designer-schema');

function throwsMessage(body: () => unknown): string {
	try {
		body();
	} catch (error) {
		return error instanceof Error ? error.message : String(error);
	}
	throw new Error('expected the call to throw');
}

function template(key: string): ScenarioTemplate {
	return {
		id: 't1',
		key,
		name: 'Template',
		description: '',
		industry: 'retail',
		project_type_key: 'forms',
		audience_label: '',
		workflow_template: {},
		field_schema: {},
		resource_schema: {},
		ai_roles: [],
		governance_policy: {},
		sample_data: {},
		created_at: '2026-01-01T00:00:00Z',
		updated_at: '2026-01-01T00:00:00Z'
	};
}

suite.check('parseFields returns nothing for a missing or malformed field list', () => {
	assertDeepEqual(m.parseFields({}), [], 'no fields key');
	assertDeepEqual(m.parseFields({ fields: 'nope' }), [], 'fields not an array');
	assertDeepEqual(m.parseFields({ fields: [null, 3, 'x', ['a']] }), [], 'non-object entries');
	assertDeepEqual(
		m.parseFields({ fields: [{ label: 'No key' }, { key: '' }, { key: 5 }] }),
		[],
		'entries without a string key'
	);
});

suite.check('parseFields applies defaults to a minimal field', () => {
	assertDeepEqual(
		m.parseFields({ fields: [{ key: 'a' }] }),
		[{ key: 'a', type: 'text', required: false, options: [] }],
		'minimal field (undefined members are omitted by JSON)'
	);
});

suite.check('parseFields normalises types, options and maps', () => {
	const [field] = m.parseFields({
		fields: [
			{
				field_id: 'f1',
				key: 'status',
				label: 'Status',
				type: 'select',
				required: 1,
				options: ['open', 3, 'done', null],
				option_colors: { open: '#fff', done: 7 },
				option_disabled: ['done', 1],
				option_archived: ['old', false],
				option_descriptions: { open: 'Open one', bad: {} },
				option_groups: { open: 'Active', bad: 2 },
				amount: { currency: 'USD', scale: 3 },
				placeholder: 'Pick',
				help_text: 'Help',
				default_value: 'open',
				validation: { max_length: 5 },
				visibility: { list: false },
				conditional: { hidden_when: { field: 'x', equals: '1' } },
				policy: { read_only: true },
				formula: { op: 'sum' },
				relation: { form_key: 'orders' },
				attachment: { multiple: true }
			}
		]
	});
	assert(field !== undefined, 'one field parsed');
	assertEqual(field.field_id, 'f1', 'field_id');
	assertEqual(field.type, 'single_select', 'legacy select becomes single_select');
	assertEqual(field.required, true, 'required is coerced');
	assertDeepEqual(field.options, ['open', 'done'], 'options keep strings only');
	assertDeepEqual(field.option_colors, { open: '#fff' }, 'colors keep string values');
	assertDeepEqual(field.option_disabled, ['done'], 'disabled');
	assertDeepEqual(field.option_archived, ['old'], 'archived');
	assertDeepEqual(field.option_descriptions, { open: 'Open one' }, 'descriptions');
	assertDeepEqual(field.option_groups, { open: 'Active' }, 'groups');
	assertDeepEqual(field.amount, { currency: 'USD', scale: 3 }, 'amount');
	assertEqual(field.placeholder, 'Pick', 'placeholder');
	assertEqual(field.help_text, 'Help', 'help text');
	assertEqual(field.default_value, 'open', 'default value');
	assertDeepEqual(field.validation, { max_length: 5 }, 'validation');
	assertDeepEqual(field.visibility, { list: false }, 'visibility');
	assertDeepEqual(field.conditional, { hidden_when: { field: 'x', equals: '1' } }, 'conditional');
	assertDeepEqual(field.policy, { read_only: true }, 'policy');
	assertDeepEqual(field.formula, { op: 'sum' }, 'formula');
	assertDeepEqual(field.relation, { form_key: 'orders' }, 'relation');
	assertDeepEqual(field.attachment, { multiple: true }, 'attachment');
});

suite.check('parseFields ignores members of the wrong shape and keeps other types', () => {
	const fields = m.parseFields({
		fields: [
			{
				key: 'a',
				type: 7,
				label: 4,
				option_colors: 'x',
				amount: 'x',
				validation: [1],
				policy: null
			},
			{ key: 'b', type: 'multi_select' },
			{ key: 'c', field_id: 9 }
		]
	});
	assertEqual(fields.length, 3, 'three fields');
	assertEqual(fields[0]?.type, 'text', 'non-string type defaults to text');
	assertEqual(fields[0]?.label, undefined, 'non-string label dropped');
	assertEqual(fields[0]?.option_colors, undefined, 'non-object colors dropped');
	assertEqual(fields[0]?.amount, undefined, 'non-object amount dropped');
	assertEqual(fields[0]?.validation, undefined, 'array validation dropped');
	assertEqual(fields[0]?.policy, undefined, 'null policy dropped');
	assertEqual(fields[1]?.type, 'multi_select', 'other types are kept');
	assertEqual(fields[2]?.field_id, undefined, 'non-string field_id dropped');
});

suite.check('blankFieldDraft returns the documented defaults as a fresh object', () => {
	assertDeepEqual(
		m.blankFieldDraft(),
		{
			key: '',
			label: '',
			type: 'text',
			required: false,
			optionsText: '',
			optionColorsText: '',
			optionDisabledText: '',
			optionArchivedText: '',
			optionDescriptionsText: '',
			optionGroupsText: '',
			currency: 'CNY',
			scale: '2',
			placeholder: '',
			helpText: '',
			defaultValue: '',
			validationMin: '',
			validationMax: '',
			validationPattern: '',
			validationMaxLength: '',
			listVisible: true,
			detailVisible: true,
			conditionalHiddenWhenText: '',
			conditionalReadOnlyWhenText: '',
			readOnly: false,
			formulaOp: 'add',
			formulaArgsText: '',
			formulaScale: '2',
			formulaRelationKey: '',
			formulaChildField: '',
			relationTargetType: 'form_record',
			relationFormKey: '',
			relationKey: 'related',
			relationType: 'relation',
			relationDisplayField: '',
			attachmentAcceptText: '',
			attachmentMaxSizeMb: '',
			attachmentMultiple: false,
			attachmentPreview: true,
			attachmentThumbnailFormat: 'png',
			attachmentPreviewFormat: 'png',
			attachmentVariantsText: '',
			attachmentStoragePolicy: 'local',
			attachmentRetentionDays: '',
			attachmentSignedUrlTtlMinutes: '10'
		},
		'defaults'
	);
	const first = m.blankFieldDraft();
	first.key = 'changed';
	assertEqual(m.blankFieldDraft().key, '', 'each call returns a new object');
});

suite.check('normalizeTemplateSchema fixes legacy types and fills version and fields', () => {
	assertDeepEqual(
		m.normalizeTemplateSchema({
			fields: [{ key: 'a', type: 'select' }, { key: 'b', type: 'text' }, 'raw', null]
		}),
		{
			fields: [{ key: 'a', type: 'single_select' }, { key: 'b', type: 'text' }, 'raw', null],
			version: 'openpr.form.schema.v1'
		},
		'select becomes single_select, non-objects are preserved, default version'
	);
	assertDeepEqual(
		m.normalizeTemplateSchema({ version: 'custom.v2', extra: true }),
		{ version: 'custom.v2', extra: true, fields: [] },
		'string version and extra members survive, missing fields become []'
	);
	assertEqual(
		m.normalizeTemplateSchema({ version: 2 }).version,
		'openpr.form.schema.v1',
		'non-string version replaced'
	);
	assertDeepEqual(
		m.normalizeTemplateSchema({ fields: 'x' }).fields,
		[],
		'non-array fields become []'
	);
});

suite.check('templateFormKey strips one trailing _default', () => {
	assertEqual(m.templateFormKey(template('order_default')), 'order', 'suffix removed');
	assertEqual(m.templateFormKey(template('order')), 'order', 'no suffix');
	assertEqual(
		m.templateFormKey(template('a_default_default')),
		'a_default',
		'only the last suffix'
	);
	assertEqual(m.templateFormKey(template('default_order')), 'default_order', 'prefix untouched');
	assertEqual(
		m.templateFormKey(template('order_defaults')),
		'order_defaults',
		'longer word untouched'
	);
});

suite.check('templateTitleTemplate prefers the first required field', () => {
	assertEqual(
		m.templateTitleTemplate({
			fields: [{ key: 'a' }, { key: 'b', required: true }, { key: 'c', required: true }]
		}),
		'{b}',
		'first required'
	);
	assertEqual(
		m.templateTitleTemplate({ fields: [{ key: 'a' }, { key: 'b' }] }),
		'{a}',
		'first field'
	);
	assertEqual(m.templateTitleTemplate({ fields: [] }), '{id}', 'no fields');
	assertEqual(m.templateTitleTemplate({}), '{id}', 'no schema fields');
});

suite.check('fieldDraftsFromSchema returns one blank draft for an empty schema', () => {
	assertDeepEqual(m.fieldDraftsFromSchema({}), [m.blankFieldDraft()], 'empty');
	assertDeepEqual(
		m.fieldDraftsFromSchema({ fields: [{ label: 'no key' }] }),
		[m.blankFieldDraft()],
		'no usable field'
	);
});

suite.check('fieldDraftsFromSchema converts a select field with options and conditions', () => {
	const drafts = m.fieldDraftsFromSchema({
		fields: [
			{
				field_id: 'f1',
				key: 'status',
				label: 'Status',
				type: 'select',
				required: true,
				options: ['open', 'done', 'old'],
				option_colors: { open: '#ff0000', done: 'red' },
				option_disabled: ['done'],
				option_archived: ['old'],
				option_descriptions: { open: ' Still open ' },
				option_groups: { open: 'Active' },
				placeholder: 'Pick',
				help_text: 'Help',
				default_value: 'open',
				visibility: { list: false },
				conditional: {
					hidden_when: { field: 'x', equals: '1' },
					read_only_when: { logic: 'any', conditions: [{ field: 'y', equals: '2' }] }
				},
				policy: { read_only: true }
			}
		]
	});
	assertDeepEqual(
		drafts,
		[
			{
				...m.blankFieldDraft(),
				field_id: 'f1',
				key: 'status',
				label: 'Status',
				type: 'single_select',
				required: true,
				optionsText: 'open\ndone\nold',
				optionColorsText: 'open: #ff0000',
				optionDisabledText: 'done',
				optionArchivedText: 'old',
				optionDescriptionsText: 'open: Still open',
				optionGroupsText: 'open: Active',
				placeholder: 'Pick',
				helpText: 'Help',
				defaultValue: 'open',
				listVisible: false,
				detailVisible: true,
				conditionalHiddenWhenText: 'x=1',
				conditionalReadOnlyWhenText: 'any:\ny=2',
				readOnly: true,
				attachmentPreview: false
			}
		],
		'select draft'
	);
});

suite.check('fieldDraftsFromSchema converts amount, validation and default values', () => {
	const drafts = m.fieldDraftsFromSchema({
		fields: [
			{
				key: 'price',
				type: 'amount',
				amount: { currency: 'USD', scale: 4 },
				default_value: 12.5,
				validation: { min: 0, max: '100', pattern: '^a', max_length: 10 }
			},
			{ key: 'flag', type: 'boolean', default_value: false },
			{ key: 'meta', type: 'text', default_value: { a: 1 } },
			{ key: 'none', type: 'text', default_value: null },
			{ key: 'zero', type: 'integer', default_value: 0, validation: { min: 0 } }
		]
	});
	const price = drafts[0];
	assertEqual(price?.label, 'price', 'label falls back to the key');
	assertEqual(price?.currency, 'USD', 'currency');
	assertEqual(price?.scale, '4', 'scale');
	assertEqual(price?.defaultValue, '12.5', 'number default is stringified');
	assertEqual(price?.validationMin, '0', 'zero min is kept');
	assertEqual(price?.validationMax, '100', 'string max');
	assertEqual(price?.validationPattern, '^a', 'pattern');
	assertEqual(price?.validationMaxLength, '10', 'max length');
	assertEqual(drafts[1]?.defaultValue, 'false', 'false default is stringified');
	assertEqual(drafts[2]?.defaultValue, '{"a":1}', 'object default is JSON');
	assertEqual(drafts[3]?.defaultValue, '', 'null default');
	assertEqual(drafts[4]?.defaultValue, '0', 'zero default');
	assertEqual(drafts[4]?.validationMin, '0', 'zero min');
	assertEqual(drafts[4]?.validationMax, '', 'absent max');
});

suite.check('fieldDraftsFromSchema converts formula, relation and attachment settings', () => {
	const drafts = m.fieldDraftsFromSchema({
		fields: [
			{
				key: 'total',
				type: 'number',
				formula: {
					op: 'sum',
					args: ['a', { field: 'b' }, { value: 'c' }, 3, { x: 1 }],
					scale: 0,
					relation_key: 'items',
					field: 'qty'
				}
			},
			{
				key: 'rel',
				type: 'relation',
				relation: {
					target_type: 'workspace_member',
					form_key: 'orders',
					relation_key: 'lines',
					relation_type: 'child',
					display_field: 'name'
				}
			},
			{
				key: 'doc',
				type: 'attachment',
				attachment: {
					accept: ['image/png', '.pdf'],
					max_size_mb: 5,
					multiple: true,
					preview: true,
					thumbnail_format: 'webp',
					preview_format: 'jpg',
					variants: [{ kind: 'thumb', max_dimension: 200, format: 'jpg' }],
					storage_policy: 's3',
					retention_days: 30,
					signed_url_ttl_minutes: 0
				}
			},
			{ key: 'img', type: 'image', attachment: {} },
			{ key: 'img2', type: 'image', attachment: { thumbnail: false } },
			{ key: 'img3', type: 'image' },
			{ key: 'file', type: 'attachment', attachment: { thumbnail: true } }
		]
	});
	const total = drafts[0];
	assertEqual(total?.formulaOp, 'sum', 'formula op');
	assertEqual(total?.formulaArgsText, 'a\nb\nc\n3\n{"x":1}', 'formula args');
	assertEqual(total?.formulaScale, '0', 'zero scale is kept');
	assertEqual(total?.formulaRelationKey, 'items', 'relation key');
	assertEqual(total?.formulaChildField, 'qty', 'child field');
	const rel = drafts[1];
	assertEqual(rel?.relationTargetType, 'workspace_member', 'target type');
	assertEqual(rel?.relationFormKey, 'orders', 'form key');
	assertEqual(rel?.relationKey, 'lines', 'relation key');
	assertEqual(rel?.relationType, 'child', 'relation type');
	assertEqual(rel?.relationDisplayField, 'name', 'display field');
	const doc = drafts[2];
	assertEqual(doc?.attachmentAcceptText, 'image/png\n.pdf', 'accept');
	assertEqual(doc?.attachmentMaxSizeMb, '5', 'max size');
	assertEqual(doc?.attachmentMultiple, true, 'multiple');
	assertEqual(doc?.attachmentPreview, true, 'preview');
	assertEqual(doc?.attachmentThumbnailFormat, 'webp', 'thumbnail format');
	assertEqual(doc?.attachmentPreviewFormat, 'jpg', 'preview format is copied verbatim');
	assertEqual(doc?.attachmentVariantsText, 'thumb:200:jpeg', 'variants');
	assertEqual(doc?.attachmentStoragePolicy, 's3', 'storage policy');
	assertEqual(doc?.attachmentRetentionDays, '30', 'retention');
	assertEqual(doc?.attachmentSignedUrlTtlMinutes, '0', 'zero ttl is kept');
	assertEqual(drafts[3]?.attachmentPreview, true, 'image field without a preference previews');
	assertEqual(drafts[4]?.attachmentPreview, false, 'thumbnail false is honoured on an image field');
	assertEqual(drafts[5]?.attachmentPreview, true, 'image field without attachment config previews');
	assertEqual(drafts[6]?.attachmentPreview, true, 'thumbnail true enables preview');
	assertEqual(drafts[0]?.attachmentSignedUrlTtlMinutes, '10', 'default ttl');
});

suite.check('parseOptionsText splits on newlines and commas', () => {
	assertDeepEqual(
		m.parseOptionsText('a, b\nc,,  d \n\n'),
		['a', 'b', 'c', 'd'],
		'mixed separators'
	);
	assertDeepEqual(m.parseOptionsText(''), [], 'empty');
	assertDeepEqual(m.parseOptionsText(' , \n '), [], 'only separators');
	assertDeepEqual(m.parseOptionsText('Beef Noodles'), ['Beef Noodles'], 'inner spaces kept');
});

suite.check('parseOptionColorsText keeps valid hex colours only', () => {
	assertDeepEqual(
		m.parseOptionColorsText('open: #ff0000\n done = #0F0 \nbad: red\n: #000\n\nnocolor\nodd: #12'),
		{ open: '#ff0000', done: '#0F0' },
		'valid lines'
	);
	assertDeepEqual(m.parseOptionColorsText('a:b:#fff'), {}, 'extra colon makes the colour invalid');
	assertDeepEqual(m.parseOptionColorsText(''), {}, 'empty');
});

suite.check('optionColorsToText writes sanitised colours and round trips', () => {
	assertEqual(m.optionColorsToText(null), '', 'null');
	assertEqual(m.optionColorsToText('x'), '', 'string');
	assertEqual(m.optionColorsToText([]), '', 'array');
	assertEqual(
		m.optionColorsToText({ a: ' #fff ', b: 'red', c: 5 }),
		'a: #fff',
		'trims and filters'
	);
	const text = 'open: #ff0000\ndone: #0F0';
	assertEqual(m.optionColorsToText(m.parseOptionColorsText(text)), text, 'round trip');
});

suite.check('parseOptionMetadataText reads option descriptions', () => {
	assertDeepEqual(
		m.parseOptionMetadataText(
			'open: Still open\nclosed = Done: really\n:nope\nnovalue:\nplain\n\n  x:  y  '
		),
		{ open: 'Still open', closed: 'Done: really', x: 'y' },
		'valid and invalid lines'
	);
	assertDeepEqual(m.parseOptionMetadataText(''), {}, 'empty');
});

suite.check('optionMetadataToText writes trimmed text and round trips', () => {
	assertEqual(m.optionMetadataToText(undefined), '', 'undefined');
	assertEqual(m.optionMetadataToText({ a: ' x ', b: '  ', c: 1 }), 'a: x', 'trims and filters');
	const text = 'open: Still open\nclosed: Done';
	assertEqual(m.optionMetadataToText(m.parseOptionMetadataText(text)), text, 'round trip');
});

suite.check('sanitizeAttachmentDerivativeFormat accepts the four image formats', () => {
	const table: Array<[string | null | undefined, string]> = [
		[undefined, 'png'],
		[null, 'png'],
		['', 'png'],
		['png', 'png'],
		['PNG', 'png'],
		['jpg', 'jpeg'],
		['JPG', 'jpeg'],
		['image/jpg', 'jpeg'],
		['image/jpeg', 'jpeg'],
		['jpeg', 'jpeg'],
		[' WEBP ', 'webp'],
		['gif', 'png'],
		['image/png', 'png']
	];
	for (const [input, expected] of table) {
		assertEqual(
			m.sanitizeAttachmentDerivativeFormat(input),
			expected,
			`format ${JSON.stringify(input)}`
		);
	}
});

suite.check('attachmentVariantsToText writes kind, size and format', () => {
	assertEqual(m.attachmentVariantsToText(undefined), '', 'undefined');
	assertEqual(m.attachmentVariantsToText('x'), '', 'string');
	assertEqual(
		m.attachmentVariantsToText([
			{ kind: 'thumb', max_dimension: 200, format: 'jpg' },
			{ kind: ' preview ' },
			{ kind: '' },
			{ kind: 3 },
			'x',
			null,
			{ kind: 'nan', max_dimension: Number.NaN },
			{ kind: 'w', max_dimension: 100, format: 5 }
		]),
		'thumb:200:jpeg\npreview:png\nnan:png\nw:100:png',
		'valid and invalid entries'
	);
});

suite.check('parseAttachmentVariantsText parses and validates variant lines', () => {
	assertDeepEqual(
		m.parseAttachmentVariantsText(
			[
				'thumb:200:jpeg',
				'Preview Large.v2:800:webp',
				'bad!:10',
				'plain',
				'zero:0:png',
				'neg:-5',
				'big:12.7px',
				'jpg:50:jpg',
				':100',
				'',
				`${'x'.repeat(33)}:10`,
				`${'y'.repeat(32)}`
			].join('\n')
		),
		[
			{ kind: 'thumb', max_dimension: 200, format: 'jpeg' },
			{ kind: 'preview-large-v2', max_dimension: 800, format: 'webp' },
			{ kind: 'plain' },
			{ kind: 'zero' },
			{ kind: 'neg' },
			{ kind: 'big', max_dimension: 12 },
			{ kind: 'jpg', max_dimension: 50, format: 'jpeg' },
			{ kind: 'y'.repeat(32) }
		],
		'variants'
	);
	assertDeepEqual(m.parseAttachmentVariantsText(''), [], 'empty');
	assertEqual(
		m.attachmentVariantsToText(m.parseAttachmentVariantsText('thumb:200:jpeg\nbanner:1600:webp')),
		'thumb:200:jpeg\nbanner:1600:webp',
		'round trip'
	);
	assertEqual(
		m.attachmentVariantsToText(m.parseAttachmentVariantsText('plain')),
		'plain:png',
		'png is written explicitly'
	);
});

suite.check('formulaArgsToText writes strings, field references and JSON', () => {
	assertEqual(m.formulaArgsToText(undefined), '', 'undefined');
	// Deliberately malformed input: a non-array passed through a type assertion.
	assertEqual(m.formulaArgsToText('a' as unknown as unknown[]), '', 'non-array');
	assertEqual(
		m.formulaArgsToText(['a', { field: 'b' }, { value: 'c' }, 3, { x: 1 }, null, { field: 5 }]),
		'a\nb\nc\n3\n{"x":1}\nnull\n{"field":5}',
		'mixed args'
	);
	assertEqual(m.formulaArgsToText([]), '', 'empty array');
});

suite.check('parseFormulaArgsText splits and decodes JSON arguments', () => {
	assertDeepEqual(
		m.parseFormulaArgsText('a, b\n{"field":"c"}\n 3 \n\n{bad'),
		['a', 'b', { field: 'c' }, '3', '{bad'],
		'mixed arguments'
	);
	assertDeepEqual(m.parseFormulaArgsText(''), [], 'empty');
	assertDeepEqual(
		m.parseFormulaArgsText(m.formulaArgsToText(['qty', { field: 'price' }])),
		['qty', 'price'],
		'object references are written as plain field names'
	);
	assertDeepEqual(
		m.parseFormulaArgsText('{"a":1,"b":2}'),
		[{ a: 1, b: 2 }],
		'comma inside a JSON object'
	);
	assertDeepEqual(
		m.parseFormulaArgsText('qty, {"field":"a,b","list":[1,2]}, price'),
		['qty', { field: 'a,b', list: [1, 2] }, 'price'],
		'commas in nested JSON and in a JSON string, plain arguments still split'
	);
	assertDeepEqual(
		m.parseFormulaArgsText('{"s":"a\\",b"},x'),
		[{ s: 'a",b' }, 'x'],
		'escaped quote inside a JSON string'
	);
	assertDeepEqual(
		m.parseFormulaArgsText('{"a":1,\n"b":2}\nz'),
		['{"a":1', '"b":2}', 'z'],
		'a newline always ends an argument'
	);
	assertDeepEqual(
		m.parseFormulaArgsText('{bad,x'),
		['{bad', 'x'],
		'an unterminated object falls back to a plain comma split'
	);
});

suite.check('parseValidationBoundary returns numbers when the text is numeric', () => {
	const table: Array<[string, string | number]> = [
		['5', 5],
		['0', 0],
		['1.5', 1.5],
		['-3', -3],
		[' 7 ', 7],
		['1e3', 1000],
		['', ''],
		['  ', '  '],
		['abc', 'abc'],
		['2026-01-01', '2026-01-01'],
		['Infinity', 'Infinity']
	];
	for (const [input, expected] of table) {
		assertEqual(m.parseValidationBoundary(input), expected, `boundary ${JSON.stringify(input)}`);
	}
});

suite.check('parseFieldDefaultValue converts by field type', () => {
	assertEqual(m.parseFieldDefaultValue('', 'text'), undefined, 'empty');
	assertEqual(m.parseFieldDefaultValue('   ', 'number'), undefined, 'blank');
	for (const text of ['true', 'yes', '1', ' true ']) {
		assertEqual(m.parseFieldDefaultValue(text, 'boolean'), true, `boolean ${JSON.stringify(text)}`);
	}
	for (const text of ['false', 'no', '0', 'TRUE', 'x']) {
		assertEqual(
			m.parseFieldDefaultValue(text, 'boolean'),
			false,
			`boolean ${JSON.stringify(text)}`
		);
	}
	assertEqual(m.parseFieldDefaultValue('12', 'integer'), 12, 'integer');
	assertEqual(m.parseFieldDefaultValue('12.9', 'integer'), 12, 'integer truncates');
	assertEqual(m.parseFieldDefaultValue(' 7 ', 'integer'), 7, 'integer trimmed');
	assertEqual(m.parseFieldDefaultValue('1e3', 'integer'), 1, 'integer stops at the exponent');
	assertEqual(m.parseFieldDefaultValue('abc', 'integer'), 'abc', 'integer fallback');
	assertEqual(m.parseFieldDefaultValue('1.5', 'number'), 1.5, 'number');
	assertEqual(m.parseFieldDefaultValue('1e3', 'number'), 1000, 'number exponent');
	assertEqual(m.parseFieldDefaultValue('x', 'number'), 'x', 'number fallback');
	assertEqual(
		m.parseFieldDefaultValue('Infinity', 'number'),
		'Infinity',
		'non finite number stays text'
	);
	assertDeepEqual(m.parseFieldDefaultValue('{"a":1}', 'text'), { a: 1 }, 'JSON object');
	assertDeepEqual(m.parseFieldDefaultValue('[1,2]', 'text'), [1, 2], 'JSON array');
	assertEqual(m.parseFieldDefaultValue('{bad', 'text'), '{bad', 'invalid JSON stays text');
	assertEqual(m.parseFieldDefaultValue('  hello ', 'text'), 'hello', 'text is trimmed');
	assertEqual(m.parseFieldDefaultValue('12', 'text'), '12', 'text does not convert numbers');
});

suite.check('parseSelectDefaultValue validates a single select default', () => {
	const options = ['open', 'done', 'old'];
	assertEqual(m.parseSelectDefaultValue('', 'single_select', options, [], []), undefined, 'empty');
	assertEqual(
		m.parseSelectDefaultValue('  ', 'single_select', options, [], []),
		undefined,
		'blank'
	);
	assertEqual(
		m.parseSelectDefaultValue(' open ', 'single_select', options, [], []),
		'open',
		'valid'
	);
	assertEqual(
		throwsMessage(() => m.parseSelectDefaultValue('nope', 'single_select', options, [], [])),
		'forms.defaultOptionInvalid',
		'unknown option'
	);
	assertEqual(
		throwsMessage(() => m.parseSelectDefaultValue('done', 'single_select', options, ['done'], [])),
		'forms.defaultOptionDisabled',
		'disabled option'
	);
	assertEqual(
		throwsMessage(() => m.parseSelectDefaultValue('old', 'single_select', options, [], ['old'])),
		'forms.defaultOptionArchived',
		'archived option'
	);
	assertEqual(
		throwsMessage(() => m.parseSelectDefaultValue('a,b', 'single_select', ['a', 'b'], [], [])),
		'forms.defaultOptionInvalid',
		'a single select does not split on commas'
	);
});

suite.check('parseSelectDefaultValue validates every item of a multi select default', () => {
	const options = ['a', 'b', 'c'];
	assertDeepEqual(
		m.parseSelectDefaultValue('a, b', 'multi_select', options, [], []),
		['a', 'b'],
		'comma list'
	);
	assertDeepEqual(
		m.parseSelectDefaultValue('["c","a"]', 'multi_select', options, [], []),
		['c', 'a'],
		'JSON list'
	);
	assertDeepEqual(
		m.parseSelectDefaultValue('a', 'multi_select', options, [], []),
		['a'],
		'single item stays an array'
	);
	assertDeepEqual(
		m.parseSelectDefaultValue('[]', 'multi_select', options, [], []),
		[],
		'empty JSON list'
	);
	assertEqual(m.parseSelectDefaultValue('', 'multi_select', options, [], []), undefined, 'empty');
	assertEqual(
		throwsMessage(() => m.parseSelectDefaultValue('a,z', 'multi_select', options, [], [])),
		'forms.defaultOptionInvalid',
		'invalid item'
	);
	assertEqual(
		throwsMessage(() => m.parseSelectDefaultValue('a,b', 'multi_select', options, ['b'], [])),
		'forms.defaultOptionDisabled',
		'disabled item'
	);
	assertEqual(
		throwsMessage(() => m.parseSelectDefaultValue('a,c', 'multi_select', options, [], ['c'])),
		'forms.defaultOptionArchived',
		'archived item'
	);
	assertEqual(
		throwsMessage(() => m.parseSelectDefaultValue('z,b', 'multi_select', options, ['b'], [])),
		'forms.defaultOptionInvalid',
		'items are checked in order'
	);
});

suite.check('parseDefaultMultiSelectValue reads JSON arrays and delimited lists', () => {
	assertDeepEqual(
		m.parseDefaultMultiSelectValue('["a"," ",1,"b"]'),
		['a', 'b'],
		'JSON keeps non-blank strings'
	);
	assertDeepEqual(m.parseDefaultMultiSelectValue('[1,2]'), [], 'JSON without strings');
	assertDeepEqual(
		m.parseDefaultMultiSelectValue('[a, b'),
		['[a', 'b'],
		'partial JSON falls back to delimiters'
	);
	assertDeepEqual(m.parseDefaultMultiSelectValue('x,\ny , '), ['x', 'y'], 'comma and newline');
	assertDeepEqual(
		m.parseDefaultMultiSelectValue('{"a":1}'),
		['{"a":1}'],
		'object text is not JSON-decoded'
	);
	assertDeepEqual(m.parseDefaultMultiSelectValue(''), [], 'empty');
	// CURRENT BEHAVIOUR: strings inside a JSON array are not trimmed, so " a " keeps its spaces while
	// the delimiter path trims every item.
	assertDeepEqual(m.parseDefaultMultiSelectValue('[" a "]'), [' a '], 'JSON items are not trimmed');
});

suite.check('fieldIdentity prefers field_id over key', () => {
	assertEqual(m.fieldIdentity({ field_id: 'f1', key: 'k' }), 'f1', 'field id');
	assertEqual(m.fieldIdentity({ key: 'k' }), 'k', 'no field id');
	assertEqual(m.fieldIdentity({ field_id: '', key: 'k' }), 'k', 'empty field id');
});

finish(suite.result());
