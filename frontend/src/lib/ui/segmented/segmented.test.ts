import { Suite, assert, assertEqual, finish } from '../../../../tests/support/harness';
import { installSvelteSsr, renderToHtml } from '../../../../tests/support/svelte-ssr';
import { nextRovingIndex } from '../foundations/roving';
import { tabStopIndex } from './segmented-state';

await installSvelteSsr();
const { default: Segmented } = await import('./Segmented.svelte');

const suite = new Suite('ui-segmented');
const options = [
	{ value: 'a', label: 'A' },
	{ value: 'b', label: 'B', disabled: true, disabledReason: 'Not yet' },
	{ value: 'c', label: 'C' }
];

suite.check('the tab stop is the checked segment, else the first enabled one', () => {
	assertEqual(tabStopIndex(options, 'c'), 2, 'checked');
	assertEqual(tabStopIndex(options, null), 0, 'none checked');
	assertEqual(tabStopIndex(options, 'b'), 0, 'a disabled value never takes the stop');
});
suite.check('arrow keys skip a disabled segment and wrap', () => {
	const enabled = options.map((o) => !o.disabled);
	const opts = { orientation: 'horizontal' } as const;
	assertEqual(nextRovingIndex('ArrowRight', 0, enabled, opts), 2, 'right skips');
	assertEqual(nextRovingIndex('ArrowRight', 2, enabled, opts), 0, 'right wraps');
	assertEqual(nextRovingIndex('ArrowLeft', 0, enabled, opts), 2, 'left wraps');
	assertEqual(nextRovingIndex('ArrowDown', 0, enabled, opts), null, 'vertical keys do nothing');
});

await suite.checkAsync(
	'renders a radio group with one tab stop and the checked segment marked',
	async () => {
		const html = await renderToHtml(Segmented, { options, value: 'c', label: 'View' });
		assert(html.includes('role="radiogroup"') && html.includes('aria-label="View"'), html);
		assertEqual((html.match(/role="radio"/g) ?? []).length, 3, 'segments');
		assertEqual((html.match(/tabindex="0"/g) ?? []).length, 1, 'one tab stop');
		assertEqual((html.match(/aria-checked="true"/g) ?? []).length, 1, 'one checked');
		assert(/aria-checked="true"[^>]*>|tabindex="0"[^>]*data-value="c"/.test(html), 'checked is C');
	}
);
await suite.checkAsync('a disabled segment is aria-disabled and carries its reason', async () => {
	const html = await renderToHtml(Segmented, { options, value: 'a', label: 'View' });
	assert(html.includes('aria-disabled="true"'), 'aria-disabled');
	assert(html.includes('Not yet'), 'reason is available as text');
});

finish(suite.result());
