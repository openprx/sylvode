import { test } from 'node:test';
import assert from 'node:assert/strict';
import { pageInlineRuns, pageVisibleBlocks } from '../src/lib/flow/inline-runs';
test('UTF8 spans preserve Unicode overlap boundaries and literal script text', () => {
	const runs = pageInlineRuns('中😀<script>', [
		{ start: 0, end: 7, mark: 'bold' },
		{ start: 3, end: 7, mark: 'italic' }
	]);
	assert.deepEqual(
		runs.map((r) => [r.text, r.marks.map((m) => m.mark)]),
		[
			['中', ['bold']],
			['😀', ['bold', 'italic']],
			['<script>', []]
		]
	);
});
test('unsafe href and invalid UTF8 boundaries cannot become public markup', () => {
	const runs = pageInlineRuns('中x', [
		{ start: 1, end: 3, mark: 'italic' },
		{ start: 0, end: 3, mark: 'link', href: 'javascript:alert(1)' }
	]);
	assert.deepEqual(runs, [{ text: '中x', marks: [] }]);
});

test('actual Svelte server component renders safe rich tags and escapes script text', async () => {
	const { readFile } = await import('node:fs/promises');
	const { compile } = await import('svelte/compiler');
	const { render } = await import('svelte/server');
	const source = await readFile(
		new URL('../src/lib/components/flow/FlowInlineText.svelte', import.meta.url),
		'utf8'
	);
	let code = compile(source, { generate: 'server', filename: 'FlowInlineText.svelte' }).js.code;
	code = code.replace(
		/from (["'])([^"']+)\1/g,
		(_all, quote, path) =>
			'from ' +
			quote +
			(path === '#lib/flow/inline-runs.js'
				? new URL('../src/lib/flow/inline-runs.ts', import.meta.url).href
				: import.meta.resolve(path)) +
			quote
	);
	const component = (
		await import('data:text/javascript;base64,' + Buffer.from(code).toString('base64'))
	).default;
	const html = render(component, {
		props: {
			text: '中😀<script>x</script>',
			spans: [
				{ start: 0, end: 7, mark: 'bold' },
				{ start: 3, end: 7, mark: 'italic' },
				{ start: 0, end: 3, mark: 'link', href: 'javascript:alert(1)' }
			]
		}
	}).body;
	assert.match(html, /<strong>/);
	assert.match(html, /<em>/);
	assert.match(html, /&lt;script(?:&gt;|>)/);
	assert.doesNotMatch(html, /<script>|href="javascript:/);
});

test('viewer folding follows active preorder subtree and sibling ordered numbering', () => {
	const blocks = [
		{ kind: 'toggle', indent: 0 },
		{ kind: 'orderedItem', indent: 1, order: 1 },
		{ kind: 'orderedItem', indent: 1, order: 1 },
		{ kind: 'orderedItem', indent: 0, order: 3 },
		{ kind: 'orderedItem', indent: 0, order: 1 }
	];
	assert.deepEqual(
		pageVisibleBlocks(blocks, new Set([0])).map((item) => [item.index, item.order]),
		[
			[0, 1],
			[3, 3],
			[4, 4]
		]
	);
	assert.deepEqual(
		pageVisibleBlocks(blocks, new Set()).map((item) => item.order),
		[1, 1, 2, 3, 4]
	);
});
