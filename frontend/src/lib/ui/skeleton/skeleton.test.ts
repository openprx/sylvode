import { Suite, assert, finish } from '../../../../tests/support/harness';
import { installSvelteSsr, renderToHtml } from '../../../../tests/support/svelte-ssr';

await installSvelteSsr();
const { default: Skeleton } = await import('./Skeleton.svelte');

const suite = new Suite('ui-skeleton');

await suite.checkAsync('a plain block is hidden from assistive technology', async () => {
	const html = await renderToHtml(Skeleton, { height: '12px' });
	assert(html.includes('aria-hidden="true"'), 'aria-hidden');
	assert(!html.includes('role="status"'), 'no status role');
	assert(html.includes('height: 12px'), 'height applied');
});

await suite.checkAsync('a circle takes its height as its width', async () => {
	const html = await renderToHtml(Skeleton, { circle: true, height: '40px', width: '10px' });
	assert(/class="[^"]*\bcircle\b/.test(html), 'circle class');
	assert(html.includes('width: 40px'), 'width follows height');
});

await suite.checkAsync('a labelled block is the status of a loading area', async () => {
	const html = await renderToHtml(Skeleton, { label: 'Loading', width: '50%' });
	assert(html.includes('role="status"'), 'status role');
	assert(html.includes('aria-label="Loading"'), 'name');
	assert(!html.includes('aria-hidden'), 'not hidden');
});

finish(suite.result());
