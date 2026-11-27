/**
 * The client field engine: the descriptor table and schema parser (`src/lib/forms/types`), rule
 * drafts (`rule-drafts.ts`), rule hints (`record-checks.ts`) and error texts (`field-errors.ts`).
 *
 * Run standalone: `bun tests/forms-page/field-engine.test.ts`
 */
import { Suite, assert, assertDeepEqual, assertEqual, finish } from '../support/harness';
import type { FieldTypeDocument, FormField } from '../../src/lib/api/forms';

const types = await import('../../src/lib/forms/types/index');
const drafts = await import('../../src/lib/forms/rule-drafts');
const checks = await import('../../src/lib/forms/record-checks');
const errors = await import('../../src/lib/forms/field-errors');

const suite = new Suite('forms-page/field-engine');
const ctx = { now: 1_800_000_000, user: { id: 'u1', role: 'member' } };

function field(key: string, extra: Partial<FormField> = {}): FormField {
	return { key, type: 'text', ...extra };
}

suite.check('parseSchema keeps unknown members in extra and reads v2 members', () => {
	const schema = types.parseSchema({
		version: 'sv.form.schema.v2',
		x_designer: { a: 1 },
		fields: [
			{
				key: 'a',
				type: 'select',
				read_only: true,
				default: { cel: 'now' },
				mask: { keep_last: 4 },
				x: 1
			},
			{ key: '', type: 'text' },
			'raw'
		]
	});
	assertDeepEqual(schema.extra, { x_designer: { a: 1 } }, 'schema extra');
	assertEqual(schema.fields.length, 1, 'keyless and non-object fields dropped');
	assertEqual(schema.fields[0]?.type, 'single_select', 'select becomes single_select');
	assertEqual(schema.fields[0]?.read_only, true, 'read_only');
	assertDeepEqual(schema.fields[0]?.default, { cel: 'now' }, 'CEL default');
	assertDeepEqual(schema.fields[0]?.extra, { mask: { keep_last: 4 }, x: 1 }, 'field extra');
	assertEqual(types.parseSchema(null).version, types.FORM_SCHEMA_VERSION, 'default version');
});

suite.check('descriptorDrift reports rule and CEL type differences only', () => {
	const document: FieldTypeDocument = {
		version: 1,
		types: Object.values(types.FIELD_TYPES).map((descriptor) => ({
			name: descriptor.name,
			storage: descriptor.storage,
			cel_type: descriptor.celType,
			allowed_rules: [...descriptor.allowedRules],
			normalize_ops: [...descriptor.normalizeOps],
			transform_ops: [...descriptor.transformOps],
			outlets: {},
			maskable: descriptor.maskable
		}))
	};
	assertDeepEqual(types.descriptorDrift(document), [], 'same table');
	const phone = document.types.find((entry) => entry.name === 'phone');
	assert(phone, 'phone listed');
	phone.allowed_rules = ['cel'];
	phone.cel_type = null;
	document.types.push({ ...phone, name: 'secret' });
	assertDeepEqual(
		types.descriptorDrift(document),
		['phone: allowed rules differ', 'phone: CEL type differs', 'secret: missing in the client'],
		'drift lines'
	);
});

suite.check('descriptors normalise to the stored form and bind CEL values', () => {
	const integer = field('qty', { type: 'integer' });
	const normalized = types.FIELD_TYPES.integer.normalize(integer, 5);
	assertDeepEqual(normalized, { ok: true, value: { type: 'integer', value: 5 } }, 'integer');
	assertEqual(types.FIELD_TYPES.rating.normalize(integer, 9).ok, false, 'rating range');
	assertEqual(types.FIELD_TYPES.number.normalize(field('n'), 1.5).ok, false, 'JSON number refused');
	assertDeepEqual(
		types.FIELD_TYPES.number.normalize(field('n'), '+012.50'),
		{ ok: true, value: { type: 'decimal', decimal: '12.50' } },
		'decimal text'
	);
	assertEqual(
		types.FIELD_TYPES.number.celValue(field('n'), { decimal: '12.50' }),
		'12.5',
		'decimal CEL value drops trailing zeros'
	);
	assertEqual(
		types.FIELD_TYPES.datetime.celValue(field('d'), '2027-01-15T08:00'),
		Date.UTC(2027, 0, 15, 8) / 1000,
		'zone-less date-time read as UTC seconds'
	);
	const select = field('s', { type: 'single_select', options: ['a', 'b'], option_disabled: ['b'] });
	assertDeepEqual(
		types.FIELD_TYPES.single_select.normalize(select, 'b'),
		{ ok: false, code: 'option_disabled' },
		'disabled option'
	);
	assertEqual(types.FIELD_TYPES.single_select.normalize(select, 'b', 'b').ok, true, 'carried over');
});

suite.check('rule drafts round-trip and report the server codes live', () => {
	const stored = [
		{ rule: 'min', value: '2027-01-01T08:00:00Z' },
		{ rule: 'pattern', value: '^a', case_insensitive: true, code: 'bad_code' },
		{ rule: 'url', schemes: ['http'] },
		{ cel: 'size(value) > 1', message: { zh: '太短' }, run_on_empty: true }
	];
	const parsed = drafts.ruleDraftsFromRules(stored, 'datetime');
	assertEqual(parsed[0]?.value, '2027-01-01T08:00', 'datetime bound for the input');
	assertDeepEqual(
		parsed.map((draft) => drafts.ruleObjectFromDraft(draft, 'datetime')),
		[{ rule: 'min', value: '2027-01-01T08:00:00Z' }, ...stored.slice(1)],
		'round trip'
	);
	const check = (
		draft: Partial<ReturnType<typeof drafts.blankRuleDraft>>,
		type = 'text' as const
	) => drafts.checkRuleDraft({ ...drafts.blankRuleDraft('cel'), ...draft }, type)?.code ?? null;
	assertEqual(check({ value: 'size(value) <= 16' }), null, 'valid CEL');
	assertEqual(check({ value: '[1].map(x, x)' }), 'cel_not_in_subset', 'out of subset');
	assertEqual(check({ value: 'secret == 1' }), 'cel_unknown_variable', 'unknown variable');
	assertEqual(check({ value: 'x'.repeat(1100) }), 'cel_too_long', 'too long');
	assertEqual(check({ kind: 'pattern', value: '(?i)a' }), 'pattern_unsupported', 'inline flag');
	assertEqual(check({ kind: 'min', value: '5' }), 'rule_not_allowed_for_type', 'min on text');
	assertEqual(
		drafts.checkRuleDraft({ ...drafts.blankRuleDraft('min'), value: 'abc' }, 'integer')?.code,
		'rule_param_invalid',
		'bound does not fit'
	);
	assertEqual(check({ value: 'true', code: 'Bad' }), 'rule_param_invalid', 'code format');
	assertEqual(drafts.checkDefaultCel('now + 60'), null, 'default env');
	assertEqual(drafts.checkDefaultCel('value'), 'cel_unknown_variable', 'value not in default env');
	assertDeepEqual(drafts.addableRuleKinds('boolean'), ['cel'], 'plugin rules are not added here');
});

suite.check('rule hints follow the server pipeline and codes', () => {
	const phone = field('phone', {
		type: 'phone',
		rules: {
			normalize: [{ op: 'trim' }],
			validate: [
				{ rule: 'max_length', value: 5 },
				{ rule: 'pattern', value: '^\\+?[0-9 ]+$', code: 'phone_digits' },
				{ cel: "!value.startsWith('+0')", code: 'bad_prefix' },
				{ plugin: { package: 'acme/p', rule: 'phone' } }
			]
		}
	});
	const total = field('total', { type: 'number' });
	const note = field('note', {
		rules: {
			validate: [
				{ cel: "decimal_cmp(record.total, '100') < 0 || value != null", run_on_empty: true },
				{ cel: "user.role == 'admin'" }
			]
		}
	});
	const fields = [phone, total, note];
	assertDeepEqual(
		checks.recordHints(fields, { phone: '  +0 12a ', total: '150' }, ctx),
		{
			phone: [
				{ field: 'phone', code: 'max_length', rule: 'validate[0]', params: { bound: 5 } },
				{ field: 'phone', code: 'phone_digits', rule: 'validate[1]', params: null },
				{ field: 'phone', code: 'bad_prefix', rule: 'validate[2]', params: null }
			]
		},
		'trim before rules, overrides, plugin skipped, absent key not checked'
	);
	assertDeepEqual(
		checks.recordHints(fields, { phone: ' +86 ', note: '' }, ctx, { total: { decimal: '150' } }),
		{ note: [{ field: 'note', code: 'cel_failed', rule: 'validate[0]', params: null }] },
		'run_on_empty with the stored record'
	);
	assertDeepEqual(
		checks.recordHints(fields, { note: 'x' }, { ...ctx, user: { id: 'u1', role: 'admin' } }),
		{},
		'user role'
	);
	assertDeepEqual(
		checks.recordHints([field('q', { type: 'integer' })], { q: 'abc' }, ctx),
		{ q: [{ field: 'q', code: 'type_mismatch', rule: null, params: null }] },
		'type check first'
	);
	assertDeepEqual(
		checks.recordHints(
			[field('q', { type: 'integer', rules: { validate: [{ rule: 'foo' }] } })],
			{ q: 1 },
			ctx
		),
		{ q: [{ field: 'q', code: 'rule_invalid', rule: 'validate[0]', params: null }] },
		'an invalid rule fails closed'
	);
});

suite.check('error texts come from the rule message, the plugin or the code', () => {
	const result = {
		error_code: 'validation_failed',
		details: {
			errors: [
				{
					field: 'qty',
					field_id: 'f1',
					stage: 'validate',
					code: 'max',
					rule: 'validate[0]',
					params: { bound: '100' }
				},
				{ bad: true }
			],
			truncated: false
		}
	};
	const items = errors.validationErrorsOf(result);
	assertEqual(items?.length, 1, 'malformed items dropped');
	assertEqual(errors.validationErrorsOf({ error_code: 'other' }), null, 'other refusals');
	assertDeepEqual(Object.keys(errors.errorsByField(items ?? [])), ['qty'], 'grouped');
	const translate = (key: string, options?: { values?: Record<string, string | number> }) =>
		`${key}${options?.values ? JSON.stringify(options.values) : ''}`;
	const qty = field('qty', {
		type: 'integer',
		rules: { validate: [{ rule: 'max', value: 100, message: { zh: '太多', en: 'Too many' } }] }
	});
	assertEqual(errors.errorMessage(qty, items![0]!, translate, 'en'), 'Too many', 'rule message');
	assertEqual(
		errors.errorMessage(field('qty'), items![0]!, translate, 'en'),
		'lowcode.forms.errors.max{"bound":"100"}',
		'code text with bound'
	);
	assertEqual(
		errors.errorMessage(undefined, { code: 'plugin.acme/p.too_short' }, translate, 'zh'),
		'lowcode.forms.errors.plugin{"package":"acme/p","code":"too_short"}',
		'plugin code'
	);
	assertEqual(
		errors.errorMessage(undefined, { code: 'bad_prefix' }, translate, 'zh'),
		'lowcode.forms.errors.custom{"code":"bad_prefix"}',
		'custom code'
	);
	assertEqual(
		errors.schemaIssuesOf({
			error_code: 'schema_invalid',
			details: { errors: [{ path: '/fields/0/rules/validate/0/cel', code: 'cel_syntax' }] }
		})?.[0]?.code,
		'cel_syntax',
		'schema issues'
	);
	assertEqual(errors.carriedSummary([], [], translate, 'en'), null, 'nothing carried');
});

finish(suite.result());
