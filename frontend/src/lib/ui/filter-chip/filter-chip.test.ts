import { createRawSnippet } from 'svelte';
import { Suite, assert, finish } from '../../../../tests/support/harness';
import { installSvelteSsr, renderToHtml } from '../../../../tests/support/svelte-ssr';

await installSvelteSsr();
const { default: FilterChip } = await import('./FilterChip.svelte');

const label = createRawSnippet(() => ({ render: () => '<span>Open</span>' }));
const suite = new Suite('ui-filter-chip');

await suite.checkAsync(
	'an unselected chip is a toggle button, not pressed, without a check',
	async () => {
		const html = await renderToHtml(FilterChip, { children: label });
		assert(/<button[^>]*type="button"/.test(html), 'native button with type button');
		assert(html.includes('aria-pressed="false"'), 'aria-pressed false');
		assert(
			!html.includes('lucide-check') && !html.includes('lucide lucide-check'),
			'no check glyph'
		);
		assert(!html.includes('data-selected'), 'no selected marker');
	}
);

await suite.checkAsync('a selected chip is pressed and shows the leading check glyph', async () => {
	const html = await renderToHtml(FilterChip, { selected: true, children: label });
	assert(html.includes('aria-pressed="true"'), 'aria-pressed true');
	assert(html.includes('data-selected'), 'selected marker');
	assert(/<svg[^>]*check/.test(html) || html.includes('lucide-check'), 'check glyph rendered');
	assert(html.includes('selected'), 'selected class');
});

finish(suite.result());
