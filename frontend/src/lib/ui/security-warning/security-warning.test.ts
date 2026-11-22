import { createRawSnippet } from 'svelte';
import { Suite, assert, finish } from '../../../../tests/support/harness';
import { installSvelteSsr, renderToHtml } from '../../../../tests/support/svelte-ssr';

await installSvelteSsr();
const { default: SecurityWarning } = await import('./SecurityWarning.svelte');

const body = createRawSnippet(() => ({
	render: () => '<span>Anyone with the key can act as you.</span>'
}));
const suite = new Suite('ui-security-warning');

await suite.checkAsync('a warning shows its title and body and is decorated', async () => {
	const html = await renderToHtml(SecurityWarning, { title: 'Read every project', children: body });
	assert(html.includes('Read every project'), 'title');
	assert(html.includes('Anyone with the key'), 'body');
	assert(html.includes('data-tone="warning"'), 'default tone');
	assert(html.includes('aria-hidden="true"'), 'glyph is decorative');
});

await suite.checkAsync('a danger warning uses the danger tone', async () => {
	const html = await renderToHtml(SecurityWarning, {
		tone: 'danger',
		title: 'Revoke',
		children: body
	});
	assert(html.includes('data-tone="danger"'), 'danger tone');
	assert(html.includes('tone-danger'), 'danger class');
});

finish(suite.result());
