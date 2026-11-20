import { createRawSnippet } from 'svelte';
import { Suite, assert, assertEqual, finish } from '../../../../tests/support/harness';
import { installSvelteSsr, renderToHtml } from '../../../../tests/support/svelte-ssr';

await installSvelteSsr();
const { default: StatusChip } = await import('./StatusChip.svelte');

const label = createRawSnippet(() => ({ render: () => '<span>Saved</span>' }));
const suite = new Suite('ui-status-chip');

await suite.checkAsync('neutral is the default tone and carries its words', async () => {
	const html = await renderToHtml(StatusChip, { children: label });
	assertEqual(/data-tone="([^"]+)"/.exec(html)?.[1], 'neutral', 'tone');
	assert(html.includes('Saved'), 'label text is rendered');
});

await suite.checkAsync('each tone is reflected in its class and data attribute', async () => {
	for (const tone of ['success', 'warning', 'danger'] as const) {
		const html = await renderToHtml(StatusChip, { tone, children: label });
		assert(html.includes(`tone-${tone}`), `class for ${tone}`);
		assertEqual(/data-tone="([^"]+)"/.exec(html)?.[1], tone, `data-tone for ${tone}`);
	}
});

finish(suite.result());
