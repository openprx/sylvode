import { createRawSnippet } from 'svelte';
import { Suite, assert, assertEqual, finish } from '../../../../tests/support/harness';
import { installSvelteSsr, renderToHtml } from '../../../../tests/support/svelte-ssr';
import { isFilled, isInert } from './button-state';

await installSvelteSsr();
const { default: Button } = await import('./Button.svelte');

const label = createRawSnippet(() => ({ render: () => '<span>Save</span>' }));
const suite = new Suite('ui-button');

suite.check('filled variants get the outside focus ring marker', () => {
	assert(isFilled('primary') && isFilled('danger'), 'primary and danger');
	assert(!isFilled('secondary') && !isFilled('text') && !isFilled('danger-quiet'), 'others');
});
suite.check('busy and disabled buttons ignore activation', () => {
	assert(isInert(true, false) && isInert(false, true) && !isInert(false, false), 'inert');
});

await suite.checkAsync('enabled primary button carries the filled marker', async () => {
	const html = await renderToHtml(Button, { variant: 'primary', children: label });
	assert(html.includes('data-filled'), html);
	assert(!html.includes('disabled'), html);
});

await suite.checkAsync(
	'disabled with a reason: disabled attribute, visible reason, described by it',
	async () => {
		const html = await renderToHtml(Button, {
			variant: 'primary',
			disabled: true,
			disabledReason: 'Fill in the title',
			children: label
		});
		assert(/<button[^>]*\sdisabled/.test(html), 'button is disabled');
		assert(html.includes('Fill in the title'), 'reason is visible text');
		const id = /aria-describedby="([^"]+)"/.exec(html)?.[1];
		assert(id && html.includes(`id="${id}"`), 'aria-describedby points at the reason');
		assert(!html.includes('data-filled'), 'a disabled button is not a filled control');
	}
);

await suite.checkAsync('disabled without a reason renders none', async () => {
	const html = await renderToHtml(Button, { disabled: true, children: label });
	assert(!html.includes('class="reason'), html);
	assert(!html.includes('aria-describedby'), html);
});

await suite.checkAsync('loading keeps the label for width and marks the button busy', async () => {
	const html = await renderToHtml(Button, {
		loading: true,
		loadingLabel: 'Saving...',
		children: label
	});
	assert(html.includes('aria-busy="true"'), 'busy');
	assert(html.includes('Save') && html.includes('Saving...'), 'label and verb share the cell');
	assert(html.includes('content hidden') || /class="content[^"]*hidden/.test(html), 'label hidden');
});

await suite.checkAsync(
	'type defaults to button so it never submits a form by accident',
	async () => {
		const html = await renderToHtml(Button, { children: label });
		assertEqual(/type="(\w+)"/.exec(html)?.[1], 'button', 'type');
	}
);

finish(suite.result());
