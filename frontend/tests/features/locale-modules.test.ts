import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative, sep } from 'node:path';
import { mergeMessages, type Messages } from '../../src/lib/ui/foundations/messages';

// Feature texts live in `locale.{en,zh}.json` modules next to the code and are merged by the globs in
// `src/lib/i18n/index.ts`. This suite keeps the two globs equal, keeps every module inside a merged
// folder, and keeps `en` and `zh` modules key-for-key equal.

const ROOT = new URL('../..', import.meta.url).pathname;
const LIB = join(ROOT, 'src/lib');
const MERGED_ROOTS = [
	'ui',
	'shell',
	'surface',
	'project',
	'oauth',
	'milestones',
	'notifications',
	'governance',
	'admin',
	'setup',
	'flow',
	'webhooks',
	'people'
];

function globRoots(language: 'en' | 'zh'): string[] {
	const source = readFileSync(join(LIB, 'i18n/index.ts'), 'utf8');
	const match = source.match(
		new RegExp(`import\\.meta\\.glob<Messages>\\('\\.\\./\\{([^}]*)\\}/\\*\\*/locale\\.${language}\\.json'`)
	);
	assert.ok(match, `no ${language} locale glob in src/lib/i18n/index.ts`);
	return match[1].split(',');
}

function localeModules(dir: string, found: string[] = []): string[] {
	for (const entry of readdirSync(dir)) {
		const path = join(dir, entry);
		if (statSync(path).isDirectory()) localeModules(path, found);
		else if (/^locale\.(en|zh)\.json$/.test(entry)) found.push(path);
	}
	return found;
}

function leafKeys(node: unknown, prefix = ''): string[] {
	if (!node || typeof node !== 'object' || Array.isArray(node)) return [prefix];
	return Object.entries(node as Record<string, unknown>).flatMap(([key, value]) =>
		leafKeys(value, prefix ? `${prefix}.${key}` : key)
	);
}

// A plural message has one leaf per plural category and the languages differ in their categories
// (`zh` has no `one`), so the twin check compares the messages, not the categories.
function messageKeys(node: unknown): string[] {
	return [...new Set(leafKeys(node).map((key) => key.replace(/\.(zero|one|two|few|many|other)$/, '')))].sort();
}

function read(path: string): Messages {
	return JSON.parse(readFileSync(path, 'utf8')) as Messages;
}

test('the en and zh globs name the same feature folders, and the list here', () => {
	assert.deepEqual(globRoots('en'), globRoots('zh'));
	assert.deepEqual(globRoots('en'), MERGED_ROOTS);
});

test('every locale module lives in a merged folder and has its twin with the same keys', () => {
	const modules = localeModules(LIB);
	assert.ok(modules.length > 0, 'no locale modules found');
	for (const path of modules) {
		const folder = relative(LIB, path).split(sep)[0];
		assert.ok(MERGED_ROOTS.includes(folder), `${relative(ROOT, path)} is outside the merged folders`);
		const twin = path.endsWith('locale.en.json')
			? path.replace(/locale\.en\.json$/, 'locale.zh.json')
			: path.replace(/locale\.zh\.json$/, 'locale.en.json');
		assert.deepEqual(
			messageKeys(read(path)),
			messageKeys(read(twin)),
			`${relative(ROOT, path)} and its twin differ in keys`
		);
	}
});

test('merging the modules over the shared catalogues has no key conflict, in both languages', () => {
	for (const language of ['en', 'zh'] as const) {
		const base = read(join(LIB, `i18n/${language}.json`));
		const modules = localeModules(LIB)
			.filter((path) => path.endsWith(`locale.${language}.json`))
			.sort()
			.map(read);
		const merged = mergeMessages(base, ...modules);
		assert.ok(leafKeys(merged).length >= leafKeys(base).length);
	}
});
