/**
 * Every message key exists in every shipped locale.
 *
 * `flow-i18n-parity.test.ts` holds the `flow.*` namespace to its contract; this suite holds the
 * whole of each locale file to the same key set, so copy added for one page in one language is
 * caught before a user of the other language sees a raw key. The locale list is read from
 * `src/lib/i18n/index.ts`'s `register(...)` calls rather than written down here, so adding a
 * third locale extends the check instead of escaping it.
 *
 * Run standalone: `bun tests/i18n-parity.test.ts`
 */

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { Suite, assert, assertDeepEqual, finish } from './support/harness';

const ROOT = new URL('..', import.meta.url).pathname;
const I18N = join(ROOT, 'src/lib/i18n');

const suite = new Suite('i18n-parity');

function registeredLocales(): string[] {
	const source = readFileSync(join(I18N, 'index.ts'), 'utf8');
	return [...source.matchAll(/register\('([a-z-]+)',/g)].map((match) => match[1]);
}

function leafKeys(node: unknown, prefix = ''): string[] {
	if (!node || typeof node !== 'object' || Array.isArray(node)) return [prefix];
	return Object.entries(node as Record<string, unknown>).flatMap(([key, value]) =>
		leafKeys(value, prefix ? `${prefix}.${key}` : key)
	);
}

const locales = registeredLocales();
const keys = new Map<string, Set<string>>();

suite.check('at least two locales are registered', () => {
	assert(locales.length >= 2, `found ${JSON.stringify(locales)} in src/lib/i18n/index.ts`);
	for (const locale of locales) {
		keys.set(locale, new Set(leafKeys(JSON.parse(readFileSync(join(I18N, `${locale}.json`), 'utf8')))));
	}
});

for (const locale of locales) {
	for (const other of locales) {
		if (locale === other) continue;
		suite.check(`every ${other} key exists in ${locale}`, () => {
			const mine = keys.get(locale) ?? new Set<string>();
			const theirs = keys.get(other) ?? new Set<string>();
			const missing = [...theirs].filter((key) => !mine.has(key)).sort();
			assertDeepEqual(missing, [], `${locale}.json lacks keys that ${other}.json has`);
		});
	}
}

export const result = suite.result();

if (import.meta.main) finish(result);
