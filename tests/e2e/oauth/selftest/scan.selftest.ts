import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';
import { scanDirectory } from '../lib/scan.ts';
import { scratch } from './util.ts';

const needles = [
	{ label: 'token-prefix:access', value: 'sva_' },
	{ label: 'token-prefix:refresh', value: 'svr_' },
	{ label: 'token-prefix:code', value: 'svc_' },
	{ label: 'password:userA', value: 'Sup3r-secret-pw' }
];

test('a clean directory has no findings and counts its files', () => {
	const dir = scratch('scan-clean-');
	mkdirSync(join(dir, 'sub'));
	writeFileSync(join(dir, 'a.json'), '{"status":"ok","token":"<redacted>"}');
	writeFileSync(join(dir, 'sub', 'b.md'), 'nothing here');
	const report = scanDirectory(dir, needles);
	assert.deepEqual(report, { files: 2, findings: [] });
});

test('each prefix and the password are found in nested files, reported by label only', () => {
	const dir = scratch('scan-dirty-');
	mkdirSync(join(dir, 'deep', 'er'), { recursive: true });
	writeFileSync(join(dir, 'one.txt'), 'x sva_abc y');
	writeFileSync(join(dir, 'deep', 'two.txt'), 'svr_123');
	writeFileSync(join(dir, 'deep', 'er', 'three.bin'), Buffer.from('..svc_..'));
	writeFileSync(join(dir, 'deep', 'er', 'four.md'), 'pw Sup3r-secret-pw end');
	const report = scanDirectory(dir, needles);
	assert.deepEqual(report.findings.map((f) => f.needle).sort(), [
		'password:userA',
		'token-prefix:access',
		'token-prefix:code',
		'token-prefix:refresh'
	]);
	assert.ok(report.findings.every((f) => !JSON.stringify(f).includes('Sup3r-secret-pw')));
});

test('empty needles are ignored and skipped paths are not read', () => {
	const dir = scratch('scan-skip-');
	writeFileSync(join(dir, 'keep.txt'), 'sva_x');
	writeFileSync(join(dir, 'skip.txt'), 'sva_y');
	const report = scanDirectory(
		dir,
		[{ label: 'empty', value: '' }, needles[0] ?? { label: 'x', value: 'x' }],
		(path) => path.endsWith('skip.txt')
	);
	assert.equal(report.findings.length, 1);
	assert.ok(report.findings[0]?.file.endsWith('keep.txt'));
});
