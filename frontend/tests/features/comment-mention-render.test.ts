/**
 * Mentions in comments: only the people the saved comment mentions are highlighted, a plain
 * `@word` is not, and no rendered text carries an id. The composer helpers keep a picked list.
 *
 * Run standalone: `bun test tests/features/comment-mention-render.test.ts`
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import type { Person } from '../../src/lib/people/display';
import {
	highlightMentions,
	insertMentionText,
	mentionSegments,
	mentionTrigger,
	pickedIds,
	withPicked
} from '../../src/lib/people/mentions';
import { renderMarkdown } from '../../src/lib/utils/markdown';
import { randomUuid } from '../../src/lib/utils/uuid';

function translator(language: 'en' | 'zh') {
	const raw = JSON.parse(
		readFileSync(new URL(`../../src/lib/people/locale.${language}.json`, import.meta.url), 'utf8')
	) as Record<string, unknown>;
	return (key: string, options?: { values?: Record<string, unknown> }): string => {
		let node: unknown = raw;
		for (const part of key.split('.')) node = (node as Record<string, unknown> | undefined)?.[part];
		assert.equal(typeof node, 'string', `missing text ${key}`);
		return (node as string).replace(/\{(\w+)\}/g, (_, name) => String(options?.values?.[name]));
	};
}

const t = translator('en');
const person = (name: string | null, over: Partial<Person> = {}): Person => ({
	id: randomUuid(),
	name,
	is_bot: false,
	state: 'active',
	...over
});
const marks = (html: string) => [
	...html.matchAll(/<span class="comment-mention"[^>]*>(.*?)<\/span>/g)
];

test('only the mentioned people are highlighted; a plain @word stays plain', () => {
	const ada = person('Ada');
	const html = highlightMentions(renderMarkdown('Hi @Ada and @nobody, see @ada.'), [ada], t);
	assert.deepEqual(
		marks(html).map((m) => m[1]),
		['@Ada', '@ada']
	);
	assert.ok(html.includes('@nobody'));
	assert.ok(!html.includes('<span class="comment-mention" title="Someone">'));
});

test('the longest name wins and a name inside a longer word is not a mention', () => {
	const ann = person('Ann');
	const annMarie = person('Ann Marie');
	const html = highlightMentions(
		renderMarkdown('@Ann Marie and @Annabel and @Ann'),
		[ann, annMarie],
		t
	);
	assert.deepEqual(
		marks(html).map((m) => m[1]),
		['@Ann Marie', '@Ann']
	);
	assert.deepEqual(
		mentionSegments('mail ada@Ann.com', [ann]).filter((s) => s.mention),
		[]
	);
});

test('Chinese names need no trailing space', () => {
	const wang = person('王小明');
	const segments = mentionSegments('请 @王小明看一下', [wang]);
	assert.deepEqual(
		segments.map((s) => [s.text, Boolean(s.mention)]),
		[
			['请 ', false],
			['@王小明', true],
			['看一下', false]
		]
	);
});

test('links, code and tags are left alone; the tooltip is the person label', () => {
	const bot = person('Builder', { is_bot: true });
	const html = highlightMentions(
		renderMarkdown('`@Builder` [@Builder](https://example.com) @Builder'),
		[bot],
		t
	);
	assert.equal(marks(html).length, 1);
	assert.ok(html.includes('title="Builder (Bot)"'));
});

test('a name with markup characters is matched on the escaped text', () => {
	const odd = person('A&B <x>');
	const html = highlightMentions(renderMarkdown('hello @A&B <x>'), [odd], t);
	assert.equal(marks(html).length, 1);
	assert.ok(!html.includes('<x>'));
});

test('mentions without a usable name are not highlighted and no id appears in the output', () => {
	const gone = person(null, { state: 'removed' });
	const idName = person(randomUuid());
	const unknownId = randomUuid();
	const body = `Hello @${unknownId} @${unknownId.slice(0, 8)} @Someone @${gone.id}`;
	for (const language of ['en', 'zh'] as const) {
		const html = highlightMentions(renderMarkdown(body), [gone, idName], translator(language));
		assert.equal(marks(html).length, 0);
		const stripped = html.replace(new RegExp(unknownId, 'g'), '').replace(gone.id, '');
		assert.ok(!/[0-9a-f]{8}-/.test(stripped), `id in output: ${stripped}`);
	}
	// A saved mention of a removed user with a stored label is shown by that label.
	const label = person('Grace', { state: 'removed' });
	assert.equal(marks(highlightMentions(renderMarkdown('@Grace'), [label], t)).length, 1);
	// Nobody mentioned: the text is returned untouched.
	const plain = renderMarkdown('@Ada');
	assert.equal(highlightMentions(plain, [], t), plain);
});

test('the picker trigger, the inserted text and the picked list', () => {
	assert.deepEqual(mentionTrigger('hello @ad', 9), { start: 6, filter: 'ad' });
	assert.deepEqual(mentionTrigger('mail a@b', 8), { start: null, filter: '' });
	assert.deepEqual(mentionTrigger('hi @Ada there', 13), { start: null, filter: '' });
	const inserted = insertMentionText('hello @ad', 6, 9, 'Ada Lovelace');
	assert.equal(inserted.text, 'hello @Ada Lovelace ');
	assert.equal(inserted.cursor, inserted.text.length);
	assert.equal(insertMentionText('no at', null, 5, 'Ada').text, 'no at');

	const ada = randomUuid();
	const bob = randomUuid();
	let picked = withPicked([], { id: ada, text: 'Ada' });
	picked = withPicked(picked, { id: bob, text: 'Bob' });
	picked = withPicked(picked, { id: ada, text: 'Ada' });
	assert.equal(picked.length, 2);
	// Only people whose `@Name` is still in the text are sent.
	assert.deepEqual(pickedIds('@ada hello', picked), [ada]);
	assert.deepEqual(new Set(pickedIds('@Ada @Bob', picked)), new Set([ada, bob]));
	assert.deepEqual(pickedIds('no one', picked), []);
});
