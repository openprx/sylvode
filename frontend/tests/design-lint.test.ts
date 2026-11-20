import { describe, test } from 'node:test';
import assert from 'node:assert/strict';
import { join } from 'node:path';
import { countSource, lintTree, parseAllow, parseRules } from '../scripts/design-lint.mjs';

const cwd = join(import.meta.dirname, 'fixtures', 'design-lint');
const root = join(cwd, 'src');
const brand = { file: 'src/brand.ts', rules: ['hex'], reason: 'brand colours' };

describe('design-lint rules', () => {
	test('dark counts variants, not object keys', () => {
		assert.equal(
			countSource('<p class="dark:bg-x md:dark:hover:text-y dark:[&>p]:m-1 dark:!p-1">').dark,
			4
		);
		assert.equal(countSource("const t = { dark: ['#fff'], dark: 1 };", 'a.ts').dark, 0);
	});
});

describe('design-lint --rules', () => {
	test('counts every category by default', async () => {
		const { totals, rows } = await lintTree(root, { cwd });
		assert.deepEqual(
			{ palette: totals.palette, dark: totals.dark, hex: totals.hex, rounded: totals.rounded },
			{ palette: 2, dark: 1, hex: 3, rounded: 1 }
		);
		assert.deepEqual(rows.map((row) => row.file).sort(), ['a.svelte', 'brand.ts']);
	});

	test('counts only the selected categories', async () => {
		const { totals, rows, rules } = await lintTree(root, {
			cwd,
			rules: parseRules('dark,rounded')
		});
		assert.deepEqual(rules, ['dark', 'rounded']);
		assert.equal(totals.total, 2);
		assert.equal(totals.palette, undefined);
		assert.deepEqual(
			rows.map((row) => row.file),
			['a.svelte']
		);
	});

	test('keeps the canonical order and rejects unknown names', () => {
		assert.deepEqual(parseRules('rounded,palette'), ['palette', 'rounded']);
		assert.throws(() => parseRules('palette,colour'), /unknown category colour/);
		assert.throws(() => parseRules(''), /empty/);
	});
});

describe('design-lint --allow', () => {
	test('an allowed file and category is not counted', async () => {
		const { totals, rows, stale } = await lintTree(root, { cwd, allow: parseAllow([brand]) });
		assert.equal(totals.hex, 1);
		assert.deepEqual(
			rows.map((row) => row.file),
			['a.svelte']
		);
		assert.deepEqual(stale, []);
	});

	test('only the listed category is excused', async () => {
		const { rows } = await lintTree(root, {
			cwd,
			allow: parseAllow([{ file: 'src/a.svelte', rules: ['hex'], reason: 'x' }])
		});
		const a = rows.find((row) => row.file === 'a.svelte');
		assert.equal(a?.hex, 0);
		assert.equal(a?.palette, 2);
	});

	test('an entry that matches nothing is reported as stale', async () => {
		const allow = parseAllow([
			brand,
			{ file: 'src/clean.svelte', rules: ['hex'], reason: 'no hex here any more' },
			{ file: 'src/gone.svelte', rules: ['dark'], reason: 'file was deleted' }
		]);
		const { stale } = await lintTree(root, { cwd, allow });
		assert.deepEqual(
			stale.map((entry) => `${entry.file} ${entry.rule}`),
			['src/clean.svelte hex', 'src/gone.svelte dark']
		);
	});

	test('entries outside the root, or for categories not selected, are not stale', async () => {
		const allow = parseAllow([brand, { file: 'elsewhere/x.ts', rules: ['hex'], reason: 'x' }]);
		const narrow = await lintTree(join(cwd, 'src', 'a.svelte'), { cwd, allow });
		assert.deepEqual(narrow.stale, []);
		const other = await lintTree(root, { cwd, allow, rules: parseRules('palette') });
		assert.deepEqual(other.stale, []);
	});

	test('a reason, a file and valid categories are required', () => {
		assert.throws(() => parseAllow([{ file: 'a', rules: ['hex'] }]), /"reason" is required/);
		assert.throws(() => parseAllow([{ rules: ['hex'], reason: 'x' }]), /"file" is required/);
		assert.throws(
			() => parseAllow([{ file: 'a', rules: ['rgb'], reason: 'x' }]),
			/unknown category rgb/
		);
		assert.throws(() => parseAllow([{ file: 'a', rules: [], reason: 'x' }]), /at least one/);
		assert.throws(() => parseAllow({}), /JSON array/);
	});
});
