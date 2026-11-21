import { createRawSnippet } from 'svelte';
import { Suite, assert, assertEqual, finish } from '../../../../tests/support/harness';
import { installSvelteSsr, renderToHtml } from '../../../../tests/support/svelte-ssr';

await installSvelteSsr();
const { default: TipBar } = await import('./TipBar.svelte');

const text = createRawSnippet(() => ({ render: () => '<span>Saved</span>' }));
const suite = new Suite('ui-tip-bar');

await suite.checkAsync('neutral is a polite status with the info glyph and no action', async () => {
	const html = await renderToHtml(TipBar, { children: text });
	assertEqual(/data-tone="([^"]+)"/.exec(html)?.[1], 'neutral', 'tone');
	assert(html.includes('role="status"'), 'status role');
	assert(!html.includes('<button'), 'no action button');
});

await suite.checkAsync('a danger bar is an alert and carries its text action', async () => {
	const html = await renderToHtml(TipBar, {
		tone: 'danger',
		children: text,
		action: { label: 'Retry', run: () => {} }
	});
	assert(html.includes('role="alert"'), 'alert role');
	assert(html.includes('tone-danger'), 'danger class');
	assert(html.includes('Retry'), 'action label');
});

await suite.checkAsync('every tone renders its own class', async () => {
	for (const tone of ['success', 'warning'] as const) {
		const html = await renderToHtml(TipBar, { tone, children: text });
		assert(html.includes(`tone-${tone}`), tone);
	}
});

finish(suite.result());
