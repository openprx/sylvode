import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';
import { buildRecord, renderMarkdown, type Environment } from '../lib/acceptance.ts';
import { CATALOG } from '../lib/catalog.ts';
import type { CaseResult } from '../lib/record.ts';

const environment: Environment = {
	uiOrigin: 'http://127.0.0.1:18097',
	apiOrigin: 'http://127.0.0.1:18083',
	mcpOrigin: 'http://127.0.0.1:8090',
	buildCommit: 'abc123',
	buildCommitSource: 'test'
};
const meta = {
	startedAt: 's',
	finishedAt: 'f',
	environment,
	limitMs: 5000,
	clientActive: true,
	scan: { scannedFiles: 3, findings: [] }
};
const result = (
	id: string,
	status: CaseResult['status'],
	timings: Record<string, number> = {}
): CaseResult => ({ id, title: id, status, durationMs: 1000, timings, details: {} });

test('the record lists all twenty-one cases, marks the missing ones, and carries the three timing figures', () => {
	const record = buildRecord(
		[
			result('A07', 'passed', { notification_ms: 321 }),
			result('A08', 'passed', { task_created_ms: 100, task_status_changed_ms: 1500 }),
			result('A11', 'failed', { revoked_ms: 6000 })
		],
		meta
	);
	assert.equal(record.cases.length, CATALOG.length);
	assert.equal(record.cases.length, 21);
	assert.deepEqual(record.summary, { passed: 2, failed: 1, notRun: 18 });
	assert.deepEqual(record.timings, {
		A07_notification_ms: 321,
		A08_task_created_ms: 100,
		A08_task_status_changed_ms: 1500,
		A11_revoked_ms: 6000
	});
	assert.equal(record.result, 'failed');
});

test('a clean run is passed only with all cases, a clean scan and an active client', () => {
	const all = CATALOG.map((entry) => result(entry.id, 'passed'));
	assert.equal(buildRecord(all, meta).result, 'passed');
	assert.equal(buildRecord(all, { ...meta, clientActive: false }).result, 'failed');
	assert.equal(
		buildRecord(all, {
			...meta,
			scan: { scannedFiles: 1, findings: [{ file: 'x', needle: 'password:userA' }] }
		}).result,
		'failed'
	);
});

test('the template renders with every placeholder filled and the limit applied', () => {
	const template = readFileSync(join(import.meta.dirname, '..', 'RESULTS-TEMPLATE.md'), 'utf8');
	const record = buildRecord(
		[
			result('A07', 'passed', { notification_ms: 321 }),
			{ ...result('A11', 'failed', { revoked_ms: 6000 }), error: 'too slow | really' }
		],
		meta
	);
	const markdown = renderMarkdown(template, record);
	assert.ok(!/\{\{[a-z_]+\}\}/.test(markdown), 'a placeholder was left unfilled');
	assert.ok(
		markdown.includes(
			'| A07 notification.created | end of the response that assigned the item | 321 ms | ok |'
		)
	);
	assert.ok(
		markdown.includes(
			'| A11 revoked control message | DELETE answer as seen by the browser | 6000 ms | FAIL |'
		)
	);
	assert.ok(markdown.includes('A11 (failed): too slow \\| really'));
	assert.ok(markdown.includes('abc123 (test)'));
	assert.ok(markdown.includes('not measured'));
});
