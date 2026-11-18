import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as pm from 'prosemirror-model';
import { createPageSchema } from '../src/lib/flow/editor-schema';
import { outlineTree, pageTextMatches, visiblePageBlocks } from '../src/lib/flow/editor-page-tools';

const schema = createPageSchema(pm);

test('find maps literal and case-insensitive matches to original Unicode positions across marks', () => {
	const doc = schema.node('doc', null, [
		schema.node('paragraph', null, [
			schema.text('İ🙂A[文', [schema.marks.bold.create()]),
			schema.text('字] a[文字]')
		]),
		schema.node('codeBlock', null, schema.text('read(\nnext)'))
	]);
	const text = (query: string) =>
		pageTextMatches(doc, query).map(({ from, to }) => ({
			from,
			text: doc.textBetween(from, to)
		}));
	assert.deepEqual(text('a[文字]'), [
		{ from: 4, text: 'A[文字]' },
		{ from: 10, text: 'a[文字]' }
	]);
	assert.equal(text('next)')[0].text, 'next)');
	assert.deepEqual(text('  '), []);
	assert.deepEqual(text('nomatch'), []);
});

test('find and outline traversal skip folded descendants and resume at the next visible sibling', () => {
	const doc = schema.node('doc', null, [
		schema.node('toggle', { collapsed: true }, schema.text('Find parent')),
		schema.node('heading', { indent: 1, level: 2 }, schema.text('Find hidden heading')),
		schema.node('paragraph', { indent: 2 }, schema.text('Find hidden text')),
		schema.node('heading', { level: 3 }, schema.text('Find visible heading')),
		schema.node('paragraph', null, schema.text('Find visible text'))
	]);
	assert.equal(pageTextMatches(doc, 'find').length, 3);
	const headings: string[] = [];
	visiblePageBlocks(doc, (node) => {
		if (node.type.name === 'heading') headings.push(node.textContent);
	});
	assert.deepEqual(headings, ['Find visible heading']);
});

test('outline preserves actual heading levels and document order with skipped levels', () => {
	const order = [3, 1, 3, 2, 3, 1, 2].map((level, pos) => ({ pos, level, text: String(pos) }));
	const outline = outlineTree(order);
	assert.deepEqual(
		outline.map(({ pos }) => pos),
		[0, 1, 5]
	);
	assert.deepEqual(
		outline[1].children.map(({ pos }) => pos),
		[2, 3]
	);
	assert.equal(outline[1].children[1].children[0].level, 3);
	assert.equal(outline[2].children[0].pos, 6);
});
