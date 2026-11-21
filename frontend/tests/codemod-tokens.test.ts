import { describe, test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { transformSource } from '../scripts/codemod-tokens.mjs';

const dir = join(import.meta.dirname, 'fixtures', 'codemod-tokens');
const inputs = readdirSync(dir).filter((f) => f.endsWith('.in'));

describe('codemod-tokens fixtures', () => {
	for (const file of inputs) {
		const name = file.replace(/\.in$/, '');
		test(name, () => {
			const input = readFileSync(join(dir, file), 'utf8');
			const expected = readFileSync(join(dir, `${name}.out`), 'utf8');
			assert.equal(transformSource(input).text, expected);
		});
	}
});

describe('codemod-tokens properties', () => {
	test('is idempotent', () => {
		for (const file of inputs) {
			const once = transformSource(readFileSync(join(dir, file), 'utf8')).text;
			assert.equal(transformSource(once).text, once);
		}
	});

	test('counts what it changes', () => {
		const { counts } = transformSource(
			'<p class="text-slate-900 dark:text-slate-100 shadow-sm font-bold focus:ring-2 text-slate-600"></p>'
		);
		assert.deepEqual(counts, { pairs: 1, focus: 1, shadow: 1, bold: 1, unpaired: 1 });
	});
});
