/** Every component locale file exists in every shipped locale with the same keys, and no key is claimed twice. */
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { Suite, assert, assertDeepEqual, finish } from '../../../../tests/support/harness';
import { mergeMessages, type Messages } from './messages';
import { isPluralCategoryKey } from './plural';

const ROOT = new URL('..', import.meta.url).pathname;

function files(dir: string, found: string[] = []): string[] {
	for (const name of readdirSync(dir)) {
		const path = join(dir, name);
		if (statSync(path).isDirectory()) files(path, found);
		else if (/^locale\.[a-z-]+\.json$/.test(name)) found.push(path);
	}
	return found;
}
function leaves(node: unknown, prefix = ''): string[] {
	if (typeof node !== 'object' || node === null) return [prefix];
	return Object.entries(node).flatMap(([k, v]) => leaves(v, prefix ? `${prefix}.${k}` : k));
}

/**
 * Keys without the plural categories a language does not use (`en` has `.one`, `zh` only
 * `.other`): a category key counts only next to its `.other` sibling, which every language needs.
 */
function comparable(keys: string[]): string[] {
	const set = new Set(keys);
	return keys.filter((key) => {
		if (!isPluralCategoryKey(key)) return true;
		return !set.has(`${key.slice(0, key.lastIndexOf('.'))}.other`);
	});
}

const suite = new Suite('ui-locale-parity');
const all = files(ROOT);

suite.check('there are locale files', () => assert(all.length >= 2, `found ${all.length}`));

suite.check('each locale.en.json has a locale.zh.json with the same keys', () => {
	for (const en of all.filter((f) => f.endsWith('locale.en.json'))) {
		const zh = en.replace('locale.en.json', 'locale.zh.json');
		assertDeepEqual(
			comparable(leaves(JSON.parse(readFileSync(zh, 'utf8')))).sort(),
			comparable(leaves(JSON.parse(readFileSync(en, 'utf8')))).sort(),
			zh
		);
	}
});

suite.check('every plural group has an other form in every language', () => {
	for (const file of all) {
		const keys = leaves(JSON.parse(readFileSync(file, 'utf8')));
		for (const key of keys.filter(isPluralCategoryKey)) {
			const base = key.slice(0, key.lastIndexOf('.'));
			assert(keys.includes(`${base}.other`), `${file}: ${base} has no .other`);
		}
	}
});

suite.check('keys live under ui. or states. and no two files define the same key', () => {
	for (const lang of ['en', 'zh']) {
		const parts = all
			.filter((f) => f.endsWith(`locale.${lang}.json`))
			.map((f) => JSON.parse(readFileSync(f, 'utf8')) as Messages);
		for (const part of parts)
			assert(
				Object.keys(part).every((k) => k === 'ui' || k === 'states'),
				`${lang}: top-level key`
			);
		mergeMessages({}, ...parts);
	}
});

suite.check('the merged ui keys do not collide with the app dictionaries', () => {
	for (const lang of ['en', 'zh']) {
		const base = JSON.parse(
			readFileSync(join(ROOT, '..', 'i18n', `${lang}.json`), 'utf8')
		) as Messages;
		const parts = all
			.filter((f) => f.endsWith(`locale.${lang}.json`))
			.map((f) => JSON.parse(readFileSync(f, 'utf8')) as Messages);
		mergeMessages(base, ...parts);
	}
});

finish(suite.result());
