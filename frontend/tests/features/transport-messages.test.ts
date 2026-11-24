import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative, sep } from 'node:path';
import { transportFailure, type TransportKey } from '../../src/lib/api/transport';
import { describeResult } from '../../src/lib/i18n/describe';

// Modules that produce a failure themselves return a locale key and no text, so they run (and are
// tested) without an initialised locale; `describeResult` is the one place that makes words of it.

const LIB = join(new URL('../..', import.meta.url).pathname, 'src/lib');

function keysOf(file: string): string[] {
	const out: string[] = [];
	const walk = (node: unknown, prefix: string) => {
		if (typeof node !== 'object' || node === null) return void out.push(prefix);
		for (const [k, v] of Object.entries(node)) walk(v, prefix ? `${prefix}.${k}` : k);
	};
	walk(JSON.parse(readFileSync(file, 'utf8')), '');
	return out.sort();
}

function sources(dir: string): string[] {
	const out: string[] = [];
	for (const name of readdirSync(dir)) {
		const path = join(dir, name);
		if (path.split(sep).includes('vendor')) continue;
		if (statSync(path).isDirectory()) out.push(...sources(path));
		else if (/\.(ts|svelte)$/.test(name)) out.push(path);
	}
	return out;
}

test('a transport failure carries a key and no text', () => {
	const failure = transportFailure(500, 'transport.networkError');
	assert.equal(failure.code, 500);
	assert.equal(failure.message, '');
	assert.equal(failure.messageKey, 'transport.networkError');
	assert.equal(failure.data, null);
});

test('describeResult: the key wins, then the server text, then the fallback', () => {
	const translate = (key: string) => `<${key}>`;
	assert.equal(
		describeResult({ message: '', messageKey: 'transport.downloadFailed' }, 'x', translate),
		'<transport.downloadFailed>'
	);
	assert.equal(describeResult({ message: 'Server says no' }, 'x', translate), 'Server says no');
	assert.equal(describeResult({ message: '' }, 'fallback', translate), 'fallback');
	assert.equal(describeResult({}, '', translate), '');
});

test('every transport key exists in en and zh with the same keys', () => {
	const en = keysOf(join(LIB, 'surface/transport/locale.en.json'));
	const zh = keysOf(join(LIB, 'surface/transport/locale.zh.json'));
	assert.deepEqual(zh, en);
	const used = new Set<string>();
	const source = readFileSync(join(LIB, 'api/transport.ts'), 'utf8');
	for (const match of source.matchAll(/'(transport\.[A-Za-z]+)'/g)) used.add(match[1]);
	assert.ok(used.size >= 10);
	for (const key of used) assert.ok(en.includes(key), `${key} is defined`);
	const probe: TransportKey = 'transport.invalidResponse';
	assert.ok(en.includes(probe));
});

test('the client and the Flow modules hold no English message text', () => {
	const offenders: string[] = [];
	for (const dir of ['api', 'flow']) {
		for (const file of sources(join(LIB, dir))) {
			const text = readFileSync(file, 'utf8');
			for (const [index, line] of text.split('\n').entries()) {
				// `message: '<text>'` of an API result; `new Error(...)` is an internal programmer error.
				if (/\bmessage:\s*['"`][^'"`]+['"`]/.test(line) && !/messageKey/.test(line))
					offenders.push(`${relative(LIB, file)}:${index + 1}: ${line.trim()}`);
			}
		}
	}
	assert.deepEqual(offenders, []);
});
