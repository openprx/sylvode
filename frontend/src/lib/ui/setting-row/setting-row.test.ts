import { createRawSnippet } from 'svelte';
import { Suite, assert, finish } from '../../../../tests/support/harness';
import { installSvelteSsr, renderToHtml } from '../../../../tests/support/svelte-ssr';

await installSvelteSsr();
const { default: SettingRow } = await import('./SettingRow.svelte');

const action = createRawSnippet(() => ({ render: () => '<button type="button">Change</button>' }));
const chip = createRawSnippet(() => ({ render: () => '<span>Verified</span>' }));
const suite = new Suite('ui-setting-row');

await suite.checkAsync('label, value and one action are rendered in reading order', async () => {
	const html = await renderToHtml(SettingRow, { label: 'Language', value: 'English', action });
	const label = html.indexOf('Language');
	const value = html.indexOf('English');
	const button = html.indexOf('Change');
	assert(label >= 0 && value > label && button > value, 'label, value, action order');
	assert(html.includes('class="value'), 'value element');
});

await suite.checkAsync(
	'an optional chip sits with the value; absent parts are not rendered',
	async () => {
		const html = await renderToHtml(SettingRow, { label: 'Language', chip });
		assert(html.includes('Verified'), 'chip rendered');
		assert(!html.includes('class="value'), 'no value when none given');
		assert(!html.includes('class="action'), 'no action when none given');
	}
);

finish(suite.result());
