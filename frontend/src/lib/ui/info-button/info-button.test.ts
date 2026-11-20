import { createRawSnippet } from 'svelte';
import { Suite, assert, finish } from '../../../../tests/support/harness';
import { installSvelteSsr, renderToHtml } from '../../../../tests/support/svelte-ssr';

await installSvelteSsr();
const { default: InfoButton } = await import('./InfoButton.svelte');

const body = createRawSnippet(() => ({ render: () => '<span>Full explanation</span>' }));
const suite = new Suite('ui-info-button');

await suite.checkAsync(
	'the info button is named, collapsed and marks that it opens a dialog',
	async () => {
		const html = await renderToHtml(InfoButton, { label: 'About approvals', children: body });
		assert(/<button[^>]*aria-label="About approvals"/.test(html), 'accessible name');
		assert(html.includes('aria-expanded="false"'), 'collapsed');
		assert(html.includes('aria-haspopup="dialog"'), 'popup kind');
		assert(html.includes('type="button"'), 'type button');
	}
);

await suite.checkAsync('the explanation is not rendered until the button opens it', async () => {
	const html = await renderToHtml(InfoButton, { label: 'About approvals', children: body });
	assert(!html.includes('Full explanation'), 'closed popover renders nothing');
});

finish(suite.result());
