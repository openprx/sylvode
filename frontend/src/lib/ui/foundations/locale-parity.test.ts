/** Every component locale file exists in every shipped locale with the same keys, and no key is claimed twice. */
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { Suite, assert, assertDeepEqual, finish } from '../../../../tests/support/harness';
import { mergeMessages, type Messages } from './messages';

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

const suite = new Suite('ui-locale-parity');
const all = files(ROOT);

suite.check('there are locale files', () => assert(all.length >= 2, `found ${all.length}`));

suite.check('each locale.en.json has a locale.zh.json with the same keys', () => {
	for (const en of all.filter((f) => f.endsWith('locale.en.json'))) {
		const zh = en.replace('locale.en.json', 'locale.zh.json');
		assertDeepEqual(
			leaves(JSON.parse(readFileSync(zh, 'utf8'))).sort(),
			leaves(JSON.parse(readFileSync(en, 'utf8'))).sort(),
			zh
		);
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
