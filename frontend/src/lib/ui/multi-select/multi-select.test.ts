import {
	Suite,
	assert,
	assertDeepEqual,
	assertEqual,
	finish
} from '../../../../tests/support/harness';
import { installSvelteSsr, renderToHtml } from '../../../../tests/support/svelte-ssr';
import { backspace, chipView, summaryName, toggleValue, type Translate } from './multi-state';

await installSvelteSsr();
const { default: MultiSelect } = await import('./MultiSelect.svelte');

const suite = new Suite('ui-multi-select');

suite.check('toggleValue adds at the end and removes in place', () => {
	assertDeepEqual(toggleValue(['a'], 'b'), ['a', 'b'], 'add');
	assertDeepEqual(toggleValue(['a', 'b', 'c'], 'b'), ['a', 'c'], 'remove');
});
suite.check('Backspace twice removes the last chip, the first press only marks it', () => {
	const first = backspace(['bug', 'urgent'], null);
	assertDeepEqual(first, { values: ['bug', 'urgent'], marked: 'urgent', removed: null }, 'mark');
	const second = backspace(first.values, first.marked);
	assertDeepEqual(second, { values: ['bug'], marked: null, removed: 'urgent' }, 'remove');
	assertDeepEqual(backspace([], null), { values: [], marked: null, removed: null }, 'empty');
	assertEqual(backspace(['a', 'b'], 'a').marked, 'b', 'a stale mark moves to the last chip');
});
suite.check('chipView shows at most the limit and hides the rest', () => {
	const items = ['a', 'b', 'c', 'd', 'e'].map((v) => ({ value: v, label: v.toUpperCase() }));
	const view = chipView(items, 2);
	assertEqual(view.chips.length, 2, 'chips');
	assertDeepEqual(
		view.hidden.map((h) => h.label),
		['C', 'D', 'E'],
		'hidden labels for the +n name'
	);
	assertEqual(chipView(items, 10).hidden.length, 0, 'nothing hidden');
});
suite.check('summaryName lists the labels and counts the rest', () => {
	const dict: Record<string, string> = {
		'ui.multiSelect.summary': '{label}: {labels}',
		'ui.multiSelect.summaryMore': '{label}: {labels} and {n} more'
	};
	const t: Translate = (key, values) =>
		dict[key].replace(/\{(\w+)\}/g, (_, k: string) => String(values?.[k]));
	assertEqual(summaryName('Labels', [], 2, t), 'Labels', 'empty');
	assertEqual(summaryName('Labels', ['Bug', 'Urgent'], 2, t), 'Labels: Bug, Urgent', 'fits');
	assertEqual(
		summaryName('Labels', ['Bug', 'Urgent', 'a', 'b', 'c'], 2, t),
		'Labels: Bug, Urgent and 3 more',
		'more'
	);
});

const options = ['Bug', 'Urgent', 'Docs', 'Infra'].map((label) => ({
	value: label.toLowerCase(),
	label
}));

await suite.checkAsync(
	'renders chips up to the limit, "+n", and Clear all outside the combobox',
	async () => {
		const html = await renderToHtml(MultiSelect, {
			value: ['bug', 'urgent', 'docs', 'infra'],
			options,
			onchange: () => {},
			label: 'Labels',
			maxChips: 2
		});
		assert(html.includes('role="combobox"') && html.includes('aria-haspopup="listbox"'), html);
		assert(html.includes('Bug') && html.includes('Urgent'), 'first two chips');
		assert(!html.includes('>Docs<'), 'third chip hidden');
		assert(html.includes('ui.multiSelect.more'), 'overflow count');
		assert(html.includes('ui.multiSelect.clearAll'), 'clear all');
		const combobox = /<button[^>]*role="combobox"[\s\S]*?<\/button>/.exec(html)?.[0] ?? '';
		assert(!combobox.includes('clearAll'), 'no button inside the combobox');
	}
);
await suite.checkAsync('empty value shows the placeholder and no Clear all', async () => {
	const html = await renderToHtml(MultiSelect, {
		value: [],
		options,
		onchange: () => {},
		label: 'Labels',
		placeholder: 'Choose labels'
	});
	assert(html.includes('Choose labels') && !html.includes('clearAll'), html);
});

finish(suite.result());
