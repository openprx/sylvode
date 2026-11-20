import { Suite, assert, finish } from '../../../../tests/support/harness';
import { installSvelteSsr, renderToHtml } from '../../../../tests/support/svelte-ssr';

await installSvelteSsr();
const { default: TextField } = await import('./TextField.svelte');
const { default: Textarea } = await import('../textarea/Textarea.svelte');

const suite = new Suite('ui-text-field');

for (const [name, component] of [
	['TextField', TextField],
	['Textarea', Textarea]
] as const) {
	await suite.checkAsync(`${name}: the marking word follows the form marking`, async () => {
		const required = await renderToHtml(component, {
			label: 'Title',
			requirement: 'required',
			marking: 'required'
		});
		assert(required.includes('ui.form.required'), required);
		assert(/<(input|textarea)[^>]* required/.test(required), 'required attribute');
		const optional = await renderToHtml(component, {
			label: 'Title',
			requirement: 'optional',
			marking: 'required'
		});
		assert(!optional.includes('ui.form.'), optional);
		const flipped = await renderToHtml(component, {
			label: 'Title',
			requirement: 'optional',
			marking: 'optional'
		});
		assert(flipped.includes('ui.form.optional'), flipped);
	});
	await suite.checkAsync(`${name}: no marking without a form marking`, async () => {
		const html = await renderToHtml(component, { label: 'Title', requirement: 'required' });
		assert(!html.includes('ui.form.'), 'no word');
		assert(/<(input|textarea)[^>]* required/.test(html), 'still required');
	});
}

finish(suite.result());
