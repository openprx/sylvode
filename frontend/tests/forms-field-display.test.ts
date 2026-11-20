/**
 * Read-only presentation of every field type: value, empty state and copy text, plus the rules
 * that hold across types (no HTML, no data-URI text, exact decimals, no timezone shift).
 *
 * Run standalone: `bun tests/forms-field-display.test.ts`
 */

import type { FormAttachment, FormField, FormFieldType } from '../src/lib/api/forms';
import {
	DISPLAY_FIELD_TYPES,
	copyText,
	fieldDisplay,
	formatDecimal,
	isEmptyValue,
	type FieldDisplay,
	type FieldDisplayContext
} from '../src/lib/forms/field-display';
import { FORM_FIELD_TYPES } from '../src/lib/forms/record-values';
import {
	parseHtmlToTree,
	richTextToPlainText,
	textFromTree,
	type TextTreeNode
} from '../src/lib/forms/rich-text';
import { Suite, assert, assertDeepEqual, assertEqual, finish } from './support/harness';

const suite = new Suite('forms-field-display');

const attachment: FormAttachment = {
	id: 'att-1',
	workspace_id: 'w',
	project_id: 'p',
	form_id: 'f',
	record_id: 'r',
	field_id: 'fld',
	field_key: 'file',
	file_name: 'invoice.pdf',
	content_type: 'application/pdf',
	byte_size: 2048,
	storage_key: 'k',
	url: 'https://cdn.test/invoice.pdf',
	created_at: '2026-01-01T00:00:00Z',
	updated_at: '2026-01-01T00:00:00Z'
};

const ctx: FieldDisplayContext = {
	locale: 'en',
	formKey: 'orders',
	members: new Map([['user-2', { name: 'Wei Zhang', email: 'wei@example.test' }]]),
	attachmentsByField: { file: [attachment] },
	signedUrls: { 'att-1': 'https://cdn.test/signed/invoice.pdf?sig=1' },
	relationTitle: (_field, id) => (id === 'rec-9' ? 'Order #9' : undefined),
	timeZone: 'UTC'
};

function field(key: string, type: FormFieldType, extra: Partial<FormField> = {}): FormField {
	return { key, type, label: key, ...extra };
}

interface Case {
	field: FormField;
	value: unknown;
	kind: FieldDisplay['kind'];
	copy: string | null;
	/** Text the rendered value must contain, when it has text. */
	text?: string;
}

const CASES: Record<FormFieldType, Case> = {
	text: {
		field: field('note', 'text'),
		value: ' hello ',
		kind: 'text',
		copy: 'hello',
		text: 'hello'
	},
	phone: {
		field: field('phone', 'phone'),
		value: '+1 555 0100',
		kind: 'link',
		copy: '+1 555 0100',
		text: '+1 555 0100'
	},
	email: { field: field('email', 'email'), value: 'a@b.test', kind: 'link', copy: 'a@b.test' },
	address: {
		field: field('addr', 'address'),
		value: '1 Test Ave\nSpringfield',
		kind: 'text',
		copy: '1 Test Ave\nSpringfield'
	},
	location: {
		field: field('where', 'location'),
		value: { lat: 40.7128, lng: -74.006, label: 'Store' },
		kind: 'location',
		copy: '40.7128,-74.006'
	},
	scan: { field: field('sku', 'scan'), value: 'SKU-1', kind: 'text', copy: 'SKU-1' },
	signature: {
		field: field('sig', 'signature'),
		value: 'data:image/png;base64,abc',
		kind: 'signature',
		copy: null
	},
	autonumber: {
		field: field('no', 'autonumber'),
		value: 'AUTO-000001',
		kind: 'text',
		copy: 'AUTO-000001'
	},
	member: {
		field: field('owner', 'member'),
		value: { user_id: 'user-1', name: 'Ada', email: 'ada@example.test' },
		kind: 'member',
		copy: 'ada@example.test'
	},
	textarea: {
		field: field('body', 'textarea'),
		value: 'multi\nline',
		kind: 'text',
		copy: 'multi\nline'
	},
	rich_text: {
		field: field('rich', 'rich_text'),
		value: '<p>Hello &amp; <b>welcome</b></p>',
		kind: 'text',
		copy: 'Hello & welcome'
	},
	number: {
		field: field('qty', 'number'),
		value: { type: 'decimal', decimal: '1234.50' },
		kind: 'number',
		copy: '1234.50',
		text: '1,234.50'
	},
	integer: {
		field: field('count', 'integer'),
		value: { type: 'integer', value: 12345 },
		kind: 'number',
		copy: '12345',
		text: '12,345'
	},
	amount: {
		field: field('total', 'amount', { amount: { currency: 'USD', scale: 2 } }),
		value: { type: 'amount', decimal: '99.9', currency: 'USD', scale: 2 },
		kind: 'number',
		copy: '99.9 USD',
		text: '$99.90'
	},
	rating: {
		field: field('stars', 'rating'),
		value: { type: 'rating', value: 4 },
		kind: 'rating',
		copy: '4'
	},
	progress: {
		field: field('done', 'progress'),
		value: { type: 'progress', value: 75 },
		kind: 'progress',
		copy: '75%'
	},
	date: {
		field: field('day', 'date'),
		value: '2026-08-11',
		kind: 'date',
		copy: '2026-08-11',
		text: 'Aug 11, 2026'
	},
	datetime: {
		field: field('at', 'datetime'),
		value: '2026-08-11T10:00',
		kind: 'date',
		copy: '2026-08-11T10:00',
		text: '10:00'
	},
	single_select: {
		field: field('status', 'single_select', { options: ['open'] }),
		value: 'open',
		kind: 'options',
		copy: 'open'
	},
	multi_select: {
		field: field('tags', 'multi_select', { options: ['a', 'b'] }),
		value: ['a', 'b'],
		kind: 'options',
		copy: 'a, b'
	},
	boolean: { field: field('flag', 'boolean'), value: true, kind: 'boolean', copy: null },
	attachment: {
		field: field('file', 'attachment'),
		value: 'https://cdn.test/invoice.pdf',
		kind: 'files',
		copy: 'https://cdn.test/signed/invoice.pdf?sig=1'
	},
	image: {
		field: field('cover', 'image'),
		value: 'https://cdn.test/a.png',
		kind: 'images',
		copy: 'https://cdn.test/a.png'
	},
	relation: {
		field: field('order', 'relation', { relation: { form_key: 'orders' } }),
		value: 'rec-9',
		kind: 'relation',
		copy: 'Order #9'
	},
	child_table: {
		field: field('lines', 'child_table'),
		value: [{ record_id: 'c1' }, { record_id: 'c2' }],
		kind: 'children',
		copy: null
	},
	formula: {
		field: field('calc', 'formula'),
		value: { type: 'decimal', decimal: '3.00' },
		kind: 'number',
		copy: '3.00',
		text: '3.00'
	},
	ai_summary: {
		field: field('summary', 'ai_summary'),
		value: 'A summary',
		kind: 'text',
		copy: 'A summary'
	}
};

function visibleText(display: FieldDisplay): string {
	return 'text' in display ? display.text : JSON.stringify(display);
}

suite.check('every field type the product defines has a presentation', () => {
	assertDeepEqual(
		[...DISPLAY_FIELD_TYPES].sort(),
		[...FORM_FIELD_TYPES].sort(),
		'renderer table matches FORM_FIELD_TYPES'
	);
	assertEqual(FORM_FIELD_TYPES.length, 27, 'the union has 27 members');
});

for (const type of FORM_FIELD_TYPES) {
	const testCase = CASES[type];
	suite.check(`${type}: value`, () => {
		const display = fieldDisplay(testCase.field, testCase.value, ctx);
		assertEqual(display.kind, testCase.kind, 'kind');
		if (testCase.text)
			assert(
				visibleText(display).includes(testCase.text),
				`${visibleText(display)} should contain ${testCase.text}`
			);
		if ('text' in display) {
			assert(!/<[a-z/]/i.test(display.text), `no markup: ${display.text}`);
			assert(!display.text.includes('data:'), `no data URI: ${display.text}`);
			assert(!display.text.startsWith('{'), `no JSON: ${display.text}`);
		}
	});
	suite.check(`${type}: copy`, () => {
		assertEqual(
			copyText(fieldDisplay(testCase.field, testCase.value, ctx)),
			testCase.copy,
			'copy text'
		);
	});
	suite.check(`${type}: empty`, () => {
		for (const empty of [null, undefined, '', []]) {
			// Attachments and images also read stored attachment rows; use a field without any.
			const emptyField = { ...testCase.field, key: `${testCase.field.key}_empty` };
			assertEqual(
				fieldDisplay(emptyField, empty, ctx).kind,
				'empty',
				`${JSON.stringify(empty)} is empty`
			);
			assertEqual(isEmptyValue(emptyField, empty, ctx), true, 'isEmptyValue');
		}
	});
}

suite.check('decimal strings keep every digit', () => {
	assertEqual(
		formatDecimal('12345678901234567890.12', 'en'),
		'12,345,678,901,234,567,890.12',
		'en'
	);
	assertEqual(formatDecimal('-0.50', 'en'), '-0.50', 'negative zero integer part');
	assertEqual(formatDecimal('-1234.5', 'en'), '-1,234.5', 'negative');
	assertEqual(formatDecimal('1234.5', 'zh'), '1,234.5', 'zh');
	assertEqual(formatDecimal('abc', 'en'), null, 'not a number');
});

suite.check('amount with an unknown currency keeps the code as text', () => {
	const display = fieldDisplay(
		field('total', 'amount'),
		{ decimal: '10.5', currency: 'NOT A CODE', scale: 2 },
		ctx
	);
	assertEqual(display.kind, 'number', 'kind');
	assertEqual(visibleText(display), '10.50 NOT A CODE', 'fallback');
	const yuan = fieldDisplay(
		field('total', 'amount'),
		{ decimal: '99.90', currency: 'CNY', scale: 2 },
		{ ...ctx, locale: 'zh' }
	);
	assertEqual(visibleText(yuan), '¥99.90', 'CNY in zh');
});

suite.check('dates do not shift with the viewer timezone', () => {
	for (const timeZone of ['Pacific/Honolulu', 'Asia/Tokyo', 'UTC']) {
		const display = fieldDisplay(field('day', 'date'), '2026-01-01', { ...ctx, timeZone });
		assertEqual(visibleText(display), 'Jan 1, 2026', timeZone);
	}
	assertEqual(
		fieldDisplay(field('day', 'date'), '2026-02-31', ctx).kind,
		'text',
		'impossible date shown as text'
	);
	assertEqual(
		fieldDisplay(field('at', 'datetime'), 'not a date', ctx).kind,
		'text',
		'malformed datetime'
	);
});

suite.check('unsafe URLs are blocked, never linked', () => {
	const files = fieldDisplay(field('doc', 'attachment'), 'javascript:alert(1)', ctx);
	assert(
		files.kind === 'files' && files.items[0].blocked === true && !files.items[0].href,
		'attachment blocked'
	);
	const signature = fieldDisplay(
		field('sig', 'signature'),
		'data:image/svg+xml;base64,PHN2Zz4=',
		ctx
	);
	assert(
		signature.kind === 'signature' && signature.blocked && !signature.src,
		'svg signature blocked'
	);
	const images = fieldDisplay(
		field('cover', 'image'),
		['javascript:alert(1)', 'https://cdn.test/b.png'],
		ctx
	);
	assert(
		images.kind === 'images' && images.items.length === 1 && images.blocked === 1,
		'image blocked'
	);
	const email = fieldDisplay(field('email', 'email'), 'not an email', ctx);
	assertEqual(email.kind, 'text', 'non-address is plain text');
	const phone = fieldDisplay(field('phone', 'phone'), '+1 (555) 0100 x"><script>', ctx);
	assert(
		phone.kind === 'link' && phone.href === 'tel:+15550100',
		`tel built from digits: ${JSON.stringify(phone)}`
	);
});

suite.check('attachments come with name, type and size', () => {
	const display = fieldDisplay(field('file', 'attachment'), null, ctx);
	assert(display.kind === 'files', 'files');
	if (display.kind !== 'files') return;
	assertEqual(display.items[0].name, 'invoice.pdf', 'name');
	assertEqual(display.items[0].typeLabel, 'PDF', 'type');
	assertEqual(display.items[0].sizeLabel, '2 kB', 'size');
	assertEqual(display.items[0].href, 'https://cdn.test/signed/invoice.pdf?sig=1', 'signed url');
});

suite.check('members resolve through the workspace list and fall back to unknown', () => {
	const known = fieldDisplay(field('owner', 'member'), { user_id: 'user-2' }, ctx);
	assert(known.kind === 'member' && known.name === 'Wei Zhang' && known.known, 'looked up');
	const unknown = fieldDisplay(field('owner', 'member'), 'user-404', ctx);
	assert(unknown.kind === 'member' && !unknown.known && unknown.userId === 'user-404', 'unknown');
});

suite.check('relations know same-form targets and missing titles', () => {
	const same = fieldDisplay(
		field('parent', 'relation', { relation: { form_key: 'orders' } }),
		{ record_id: 'rec-1', title: 'Order #1', form_key: 'orders' },
		ctx
	);
	assert(same.kind === 'relation' && same.sameForm && same.available, 'same form');
	const other = fieldDisplay(
		field('cust', 'relation'),
		{ record_id: 'rec-2', form_key: 'customers' },
		ctx
	);
	assert(other.kind === 'relation' && !other.sameForm && !other.available, 'unavailable');
});

suite.check('mismatched shapes fall back to readable text', () => {
	assertEqual(
		visibleText(fieldDisplay(field('qty', 'number'), { display: '12 boxes' }, ctx)),
		'12 boxes',
		'display key'
	);
	const json = fieldDisplay(field('qty', 'number'), { odd: [1, 2] }, ctx);
	assertEqual(json.kind, 'json', 'json');
	assertEqual(
		fieldDisplay({ key: 'x', type: 'future_type' as FormFieldType }, 'value', ctx).kind,
		'text',
		'unknown type'
	);
	assertEqual(
		fieldDisplay(field('stars', 'rating'), { type: 'rating', value: 99 }, ctx).kind,
		'rating',
		'clamped rating'
	);
	const progress = fieldDisplay(field('done', 'progress'), 150, ctx);
	assert(progress.kind === 'progress' && progress.value === 100, 'clamped progress');
});

suite.check('rich text: script and style dropped, entities decoded, structure kept', () => {
	const html =
		'<p>Line &lt;one&gt; &amp; &quot;two&quot;</p><script>alert("x")</script><style>p{color:red}</style>' +
		'<ul><li>First</li><li>Second</li></ul>Tail<br>after &#x4e2d;&#25991;<img src=x onerror="alert(1)">';
	const text = richTextToPlainText(html, () => null);
	assertEqual(text, 'Line <one> & "two"\n\n• First\n\n• Second\n\nTail\nafter 中文', 'converted');
	assert(!text.includes('alert'), 'no script content');
	assert(!text.includes('color:red'), 'no style content');
});

suite.check('rich text: nothing that looks like markup is produced from tags', () => {
	const text = richTextToPlainText(
		'<div onclick="x()"><a href="javascript:x">link</a> <iframe src="//evil">x</iframe></div>',
		() => null
	);
	assertEqual(text, 'link', 'tags dropped');
	assertEqual(
		richTextToPlainText('a < b and c > d', () => null),
		'a < b and c > d',
		'loose angle brackets are text'
	);
	assertEqual(richTextToPlainText(null), '', 'non-string');
});

suite.check('rich text: the DOM walker reads parser-built trees the same way', () => {
	// A tree shaped like DOMParser output (decoded text nodes); the tokenizer builds the same shape.
	const text = (value: string): TextTreeNode => ({
		nodeType: 3,
		nodeName: '#text',
		nodeValue: value,
		childNodes: []
	});
	const el = (name: string, ...children: TextTreeNode[]): TextTreeNode => ({
		nodeType: 1,
		nodeName: name,
		nodeValue: null,
		childNodes: children
	});
	const tree = el(
		'BODY',
		el('P', text('A & B')),
		el('SCRIPT', text('evil()')),
		el('OL', el('LI', text('one')))
	);
	assertEqual(textFromTree(tree), 'A & B\n\n• one', 'walked');
	assertEqual(
		textFromTree(parseHtmlToTree('<p>A &amp; B</p><script>evil()</script><ol><li>one</li></ol>')),
		'A & B\n\n• one',
		'tokenized'
	);
});

suite.check('link-scheme guard blocks every non-http(s)/mailto scheme, however spelled', () => {
	const hostile = [
		'javascript:alert(1)',
		'  JaVaScRiPt:alert(1)',
		'java\tscript:alert(1)',
		'\njavascript:alert(1)',
		'vbscript:msgbox(1)',
		'data:text/html;base64,PHNjcmlwdD4=',
		'file:///etc/passwd',
		'ftp://example.test/x'
	];
	for (const url of hostile) {
		const files = fieldDisplay(field('doc', 'attachment'), url, ctx);
		assert(
			files.kind === 'files' && files.items.every((item) => item.blocked && !item.href),
			`attachment url blocked: ${JSON.stringify(url)}`
		);
		const nested = fieldDisplay(field('doc', 'attachment'), { url }, ctx);
		assert(
			nested.kind === 'files' && nested.items.every((item) => item.blocked && !item.href),
			`attachment object url blocked: ${JSON.stringify(url)}`
		);
		const images = fieldDisplay(field('cover', 'image'), [url], ctx);
		assert(
			images.kind !== 'images' || images.items.length === 0,
			`image url blocked: ${JSON.stringify(url)}`
		);
	}
	const ok = fieldDisplay(field('doc', 'attachment'), 'https://cdn.test/a.pdf', ctx);
	assert(
		ok.kind === 'files' && ok.items[0].href === 'https://cdn.test/a.pdf' && !ok.items[0].blocked,
		'https attachment still links'
	);
	for (const text of ['javascript:alert(1)//a@b.co', 'a@b.co?cc=x" onmouseover="y']) {
		const email = fieldDisplay(field('email', 'email'), text, ctx);
		assert(
			email.kind !== 'link' || email.href.startsWith('mailto:'),
			`email never yields a non-mailto link: ${JSON.stringify(text)}`
		);
	}
});

finish(suite.result());
