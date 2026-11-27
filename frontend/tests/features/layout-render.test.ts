import { test } from 'node:test';
import assert from 'node:assert/strict';
import { evaluateWhen } from '../../src/lib/layout/when';
import { formatValue } from '../../src/lib/layout/format';
import { resolveDeep, resolveRef, resolveText } from '../../src/lib/layout/refs';
import type { RefScope } from '../../src/lib/layout/types';

const scope: RefScope = {
	params: { record_id: 'r1' },
	data: { req: { values: { status: 'open' }, rows: [{ id: 'a' }, { id: 'b' }] } },
	slot: { project_id: 'p1' },
	viewer: { id: 'u1' },
	actions: {}
};

test('when: literals are evaluated, anything else stays visible', () => {
	assert.equal(evaluateWhen('false'), false);
	assert.equal(evaluateWhen(' FALSE '), false);
	assert.equal(evaluateWhen('true'), true);
	assert.equal(evaluateWhen(undefined), true);
	assert.equal(evaluateWhen("data.req.values.status != 'closed'"), true);
});

test('refs: paths, indexes and missing values', () => {
	assert.equal(resolveRef('$params.record_id', scope), 'r1');
	assert.equal(resolveRef('$data.req.values.status', scope), 'open');
	assert.equal(resolveRef('$data.req.rows[1].id', scope), 'b');
	assert.equal(resolveRef('$data.nope.values.status', scope), undefined);
	assert.equal(resolveRef('not a ref', scope), undefined);
	assert.deepEqual(resolveDeep({ id: '$params.record_id', n: [1, '$slot.project_id'] }, scope), {
		id: 'r1',
		n: [1, 'p1']
	});
});

test('text: literal, i18n map with fallback, reference', () => {
	assert.equal(resolveText('Hello', scope, 'en'), 'Hello');
	assert.equal(resolveText({ en: 'Status', zh: '状态' }, scope, 'zh'), '状态');
	assert.equal(resolveText({ en: 'Status' }, scope, 'zh'), 'Status');
	assert.equal(resolveText('$data.req.values.status', scope, 'en'), 'open');
	assert.equal(resolveText({ cel: 'x' }, scope, 'en'), '');
});

test('format: numbers and unparsable values', () => {
	assert.equal(formatValue(1234.5, 'amount', 'en'), '1,234.50');
	assert.equal(formatValue(5, 'percent', 'en'), '5%');
	assert.equal(formatValue('abc', 'number', 'en'), 'abc');
	assert.equal(formatValue('x', undefined, 'en'), 'x');
});
