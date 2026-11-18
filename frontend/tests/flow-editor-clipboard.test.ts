import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as pm from 'prosemirror-model';
import { createPageSchema, safeEditorHref } from '../src/lib/flow/editor-schema';
import { markdownBlocks, plainPasteBlocks } from '../src/lib/flow/editor-clipboard';
const schema = createPageSchema(pm);
test('Markdown paste maps bounded prose, rich runs and list attrs without HTML evaluation', () => {
	const blocks = markdownBlocks(
		'# Title\n1. **Bold**\n[ ] Item\n> Quote\n<script>alert(1)</script>\n[bad](javascript:alert)',
		schema
	)!;
	assert.equal(blocks[0].type.name, 'heading');
	assert.equal(blocks[1].type.name, 'orderedItem');
	assert.equal(blocks[1].firstChild?.marks[0].type.name, 'bold');
	assert.equal(blocks[2].type.name, 'todoItem');
	assert.equal(blocks[4].textContent, '<script>alert(1)</script>');
	assert.equal(blocks[5].firstChild?.marks.length, 0);
});
test('link safety refuses active protocols and protocol-relative addresses', () => {
	for (const href of ['javascript:alert(1)', 'data:text/html,x', '//host/path', 'https://x\n.evil'])
		assert.equal(safeEditorHref(href), false);
	for (const href of ['https://example.test/a', 'mailto:a@example.test', '/flow/uuid', '#section'])
		assert.equal(safeEditorHref(href), true);
});

test('exported balanced overlap HTML parses as native marks without accepting arbitrary tags', () => {
	const blocks = markdownBlocks(
		'<strong>前<em>😀</em></strong><a href="https://example.test?a=1&amp;b=2">link</a><script>x</script>',
		schema
	)!;
	assert.equal(blocks[0].child(1).textContent, '😀');
	assert.deepEqual(
		blocks[0].child(1).marks.map((m) => m.type.name),
		['bold', 'italic']
	);
	assert.equal(blocks[0].child(2).marks[0].attrs.href, 'https://example.test?a=1&b=2');
	assert.match(blocks[0].textContent, /<script>x<\/script>/);
});

test('literal punctuation entities decode after Markdown tokenization without turning into marks', () => {
	const blocks = markdownBlocks(
		'&#42; &#95; &#96; &#91; &#93; &lt; &amp; &#42;&#42;literal&#42;&#42;',
		schema
	)!;
	assert.equal(blocks[0].textContent, '* _ ` [ ] < & **literal**');
	assert.equal(blocks[0].firstChild!.marks.length, 0);
});
test('long code fences retain shorter backtick runs and entities literally', () => {
	const text = '`````\n<script>&amp;_*\n```\n````\n`````';
	const blocks = markdownBlocks(text, schema)!;
	assert.equal(blocks.length, 1);
	assert.equal(blocks[0].type.name, 'codeBlock');
	assert.equal(blocks[0].textContent, '<script>&amp;_*\n```\n````');
	const inline = markdownBlocks('**bold** `&amp;`', schema)!;
	assert.equal(inline[0].lastChild!.textContent, '&amp;');
	assert.equal(inline[0].lastChild!.marks[0].type.name, 'code');
});

test('explicit plain paste preserves literal entities, Markdown and script-looking text', () => {
	const text = '&#42; **literal** <script>alert(1)</script> &amp;';
	const blocks = plainPasteBlocks(text, schema);
	assert.equal(blocks[0].textContent, text);
	assert.equal(blocks[0].firstChild!.marks.length, 0);
	const markdown = markdownBlocks(
		'&#42;&#42;literal&#42;&#42; &lt;script&gt;x&lt;/script&gt;',
		schema
	)!;
	assert.equal(markdown[0].textContent, '**literal** <script>x</script>');
	assert.equal(markdown[0].firstChild!.marks.length, 0);
});
