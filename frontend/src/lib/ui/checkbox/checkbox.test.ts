import { createRawSnippet } from 'svelte';
import { Suite, assert, finish } from '../../../../tests/support/harness';
import { installSvelteSsr, renderToHtml } from '../../../../tests/support/svelte-ssr';

await installSvelteSsr();
const { default: Checkbox } = await import('./Checkbox.svelte');
const { default: Switch } = await import('../switch/Switch.svelte');

const suite = new Suite('ui-checkbox-switch');
const input = (html: string) => /<input[^>]*>/.exec(html)?.[0] ?? '';

// Keyboard toggling (Space) is native to a checkbox input; these checks pin the markup that
// makes it work, and the browser run (scripts/ui-behaviour.mjs) presses the keys.
await suite.checkAsync(
	'checkbox is a real checkbox input with the filled marker when checked',
	async () => {
		const off = input(await renderToHtml(Checkbox, { label: 'A' }));
		const on = input(await renderToHtml(Checkbox, { label: 'A', checked: true }));
		assert(off.includes('type="checkbox"') && !off.includes('checked'), off);
		assert(on.includes('checked') && on.includes('data-filled'), on);
	}
);
await suite.checkAsync('indeterminate exposes aria-checked="mixed"', async () => {
	const html = input(await renderToHtml(Checkbox, { label: 'A', indeterminate: true }));
	assert(html.includes('aria-checked="mixed"') && html.includes('data-filled'), html);
});
await suite.checkAsync('disabled checkbox is disabled and not a filled control', async () => {
	const html = input(await renderToHtml(Checkbox, { label: 'A', checked: true, disabled: true }));
	assert(html.includes('disabled') && !html.includes('data-filled'), html);
});
await suite.checkAsync(
	'switch is role=switch on a checkbox input; on state is filled',
	async () => {
		const off = input(await renderToHtml(Switch, { label: 'A' }));
		const on = input(await renderToHtml(Switch, { label: 'A', checked: true }));
		assert(off.includes('role="switch"') && off.includes('type="checkbox"'), off);
		assert(
			!off.includes('data-filled') && on.includes('data-filled') && on.includes('checked'),
			on
		);
	}
);
await suite.checkAsync('children snippet replaces the label text', async () => {
	const children = createRawSnippet(() => ({ render: () => '<b>Rich</b>' }));
	const html = await renderToHtml(Checkbox, { children });
	assert(html.includes('<b>Rich</b>'), html);
});

finish(suite.result());
