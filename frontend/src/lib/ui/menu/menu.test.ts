import { createRawSnippet } from 'svelte';
import { Suite, assert, finish } from '../../../../tests/support/harness';
import { installSvelteSsr, renderToHtml } from '../../../../tests/support/svelte-ssr';

await installSvelteSsr();
const { default: MenuItem } = await import('./MenuItem.svelte');

const suite = new Suite('ui-menu');
const children = createRawSnippet(() => ({ render: () => '<span>Label</span>' }));
const root = (html: string) => /<(button|a)[^>]*>/.exec(html)?.[0] ?? '';

await suite.checkAsync('a plain item is a menuitem without aria-checked', async () => {
	const tag = root(await renderToHtml(MenuItem, { children }));
	assert(tag.includes('role="menuitem"') && !tag.includes('aria-checked'), tag);
});
await suite.checkAsync('a radio item carries role and aria-checked in both states', async () => {
	const on = root(await renderToHtml(MenuItem, { children, role: 'menuitemradio', checked: true }));
	const off = root(await renderToHtml(MenuItem, { children, role: 'menuitemradio' }));
	assert(on.includes('role="menuitemradio"') && on.includes('aria-checked="true"'), on);
	assert(off.includes('role="menuitemradio"') && off.includes('aria-checked="false"'), off);
});
await suite.checkAsync('a checkbox item carries role and aria-checked', async () => {
	const tag = root(
		await renderToHtml(MenuItem, { children, role: 'menuitemcheckbox', checked: true })
	);
	assert(tag.includes('role="menuitemcheckbox"') && tag.includes('aria-checked="true"'), tag);
});
await suite.checkAsync(
	'a checked radio item shows the check glyph, an unchecked one does not',
	async () => {
		const on = await renderToHtml(MenuItem, { children, role: 'menuitemradio', checked: true });
		const off = await renderToHtml(MenuItem, { children, role: 'menuitemradio' });
		assert(on.includes('<svg') && !off.includes('<svg'), 'glyph only when checked');
	}
);
await suite.checkAsync('data attributes reach the item element', async () => {
	const tag = root(
		await renderToHtml(MenuItem, { children, 'data-testid': 'x', 'data-tab-key': 'k' })
	);
	assert(tag.includes('data-testid="x"') && tag.includes('data-tab-key="k"'), tag);
});
await suite.checkAsync(
	'an item with href is a real link; disabled or without href is a button',
	async () => {
		const link = root(await renderToHtml(MenuItem, { children, href: '/a/b' }));
		assert(
			link.startsWith('<a') && link.includes('href="/a/b"') && link.includes('role="menuitem"'),
			link
		);
		const disabled = root(await renderToHtml(MenuItem, { children, href: '/a/b', disabled: true }));
		assert(disabled.startsWith('<button') && disabled.includes('aria-disabled="true"'), disabled);
		assert(root(await renderToHtml(MenuItem, { children })).startsWith('<button'), 'plain button');
	}
);

finish(suite.result());
