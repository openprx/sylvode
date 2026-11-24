import { test } from 'node:test';
import assert from 'node:assert/strict';
import { installSvelteSsr, renderToHtml } from '../support/svelte-ssr';

await installSvelteSsr();
const { default: StateBlock } = await import('../../src/lib/ui/state-block/StateBlock.svelte');

test('a state block title is a paragraph unless a heading level is given', async () => {
	const plain = await renderToHtml(StateBlock, { kind: 'notFound', object: 'page' });
	assert.match(plain, /<p class="[^"]*title/);
	assert.doesNotMatch(plain, /<h[1-3]/);
});

test('headingLevel renders the title as that heading element', async () => {
	for (const level of [1, 2, 3] as const) {
		const html = await renderToHtml(StateBlock, {
			kind: 'notFound',
			object: 'page',
			headingLevel: level
		});
		assert.match(html, new RegExp(`<h${level} class="[^"]*title[^"]*"[^>]*>`));
		assert.match(html, new RegExp(`</h${level}>`));
	}
});
