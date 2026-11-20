import { createRawSnippet } from 'svelte';
import { Suite, assert, finish } from '../../../../tests/support/harness';
import { installSvelteSsr, renderToHtml } from '../../../../tests/support/svelte-ssr';

await installSvelteSsr();
const { default: Popover } = await import('./Popover.svelte');

const suite = new Suite('ui-popover');
const children = createRawSnippet(() => ({ render: () => '<p>body</p>' }));
const surface = (html: string) => /<div[^>]*data-ui-layer="popover"[^>]*>/.exec(html)?.[0] ?? '';
const base = { open: true, anchor: null, onclose: () => {}, label: 'Pick', children };

await suite.checkAsync('the surface is a dialog without aria-modal by default', async () => {
	const tag = surface(await renderToHtml(Popover, base));
	assert(tag.includes('role="dialog"') && !tag.includes('aria-modal'), tag);
});
await suite.checkAsync('modal sets aria-modal on the surface', async () => {
	const tag = surface(await renderToHtml(Popover, { ...base, modal: true }));
	assert(tag.includes('aria-modal="true"'), tag);
});
await suite.checkAsync('data attributes and describedby reach the surface element', async () => {
	const tag = surface(
		await renderToHtml(Popover, {
			...base,
			'data-testid': 'task-close-confirm',
			'data-x': 'y',
			describedby: 'why'
		})
	);
	assert(tag.includes('data-testid="task-close-confirm"') && tag.includes('data-x="y"'), tag);
	assert(tag.includes('aria-describedby="why"'), tag);
});
await suite.checkAsync('a closed popover renders nothing', async () => {
	const html = await renderToHtml(Popover, { ...base, open: false });
	assert(!html.includes('data-ui-layer'), html);
});

finish(suite.result());
