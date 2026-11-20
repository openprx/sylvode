import {
	Suite,
	assert,
	assertDeepEqual,
	assertEqual,
	finish
} from '../../../../tests/support/harness';
import {
	filterOptions,
	initialActive,
	initialTypeahead,
	keyAction,
	moveActive,
	shouldSearch,
	typeahead
} from './listbox';
import { installSvelteSsr, renderToHtml } from '../../../../tests/support/svelte-ssr';

await installSvelteSsr();
const { default: Select } = await import('./Select.svelte');

const suite = new Suite('ui-select');
const options = ['Backlog', 'Blocked', 'Done', 'In progress', 'Review'].map((label) => ({
	value: label.toLowerCase(),
	label
}));

suite.check('search shows above eight options with auto, always or never otherwise', () => {
	assertEqual(shouldSearch('auto', 8), false, '8');
	assertEqual(shouldSearch('auto', 9), true, '9');
	assertEqual(shouldSearch(true, 2), true, 'forced on');
	assertEqual(shouldSearch(false, 50), false, 'forced off');
});
suite.check('filterOptions: case-insensitive substring, empty query keeps all', () => {
	assertEqual(filterOptions(options, '').length, 5, 'all');
	assertDeepEqual(
		filterOptions(options, ' PRO ').map((o) => o.label),
		['In progress'],
		'substring'
	);
	assertEqual(filterOptions(options, 'zzz').length, 0, 'none');
});
suite.check('initialActive: the selected option, else the first enabled one', () => {
	assertEqual(
		initialActive(options, (i) => i === 3),
		3,
		'selected'
	);
	assertEqual(
		initialActive([{ disabled: true }, {}], () => false),
		1,
		'first enabled'
	);
	assertEqual(
		initialActive([], () => false),
		-1,
		'empty'
	);
});
suite.check('moveActive clamps at both ends and pages by eight', () => {
	assertEqual(moveActive('ArrowDown', 4, 5), 4, 'stays on the last');
	assertEqual(moveActive('ArrowUp', 0, 5), 0, 'stays on the first');
	assertEqual(moveActive('End', 0, 20), 19, 'end');
	assertEqual(moveActive('Home', 7, 20), 0, 'home');
	assertEqual(moveActive('PageDown', 2, 20), 10, 'page down');
	assertEqual(moveActive('PageUp', 2, 20), 0, 'page up clamps');
	assertEqual(moveActive('a', 2, 20), null, 'other keys');
	assertEqual(moveActive('ArrowDown', -1, 0), null, 'no options');
});
suite.check('type-ahead: a prefix grows, a repeated letter cycles, a pause resets', () => {
	const labels = options.map((o) => o.label);
	let r = typeahead(initialTypeahead, 'b', 0, labels, 2);
	assertEqual(r.index, 0, 'first B after wrap');
	r = typeahead(r.state, 'l', 100, labels, 0);
	assertEqual(r.index, 1, 'bl is Blocked, not Backlog');
	r = typeahead(r.state, 'o', 200, labels, 0);
	assertEqual(r.index, 1, 'blo keeps Blocked');
	r = typeahead(initialTypeahead, 'b', 0, labels, 0);
	r = typeahead(r.state, 'b', 100, labels, 0);
	assertEqual(r.index, 1, 'repeated b moves to the next B');
	r = typeahead(r.state, 'b', 200, labels, 1);
	assertEqual(r.index, 0, 'and cycles back');
	r = typeahead(r.state, 'd', 5000, labels, 0);
	assertEqual(r.state.buffer, 'd', 'buffer reset after the pause');
	assertEqual(r.index, 2, 'Done');
	assertEqual(
		typeahead(initialTypeahead, 'x', 0, labels, 0).index,
		null,
		'no match keeps the active option'
	);
});
suite.check('keyAction on the closed trigger', () => {
	const base = { open: false, multiple: false, searching: false };
	for (const key of ['Enter', ' ', 'ArrowDown'])
		assertDeepEqual(keyAction({ ...base, key }), { type: 'open' }, key);
	assertDeepEqual(
		keyAction({ ...base, key: 'ArrowDown', altKey: true }),
		{ type: 'open' },
		'Alt+Down'
	);
	assertDeepEqual(keyAction({ ...base, key: 'r' }), { type: 'typeahead', char: 'r' }, 'typing');
	assertEqual(keyAction({ ...base, key: 'Escape' }), null, 'escape does nothing when closed');
	assertEqual(
		keyAction({ ...base, key: 'k', ctrlKey: true }),
		null,
		'Ctrl combos are left to the browser'
	);
});
suite.check('keyAction on the open menu: single and multiple', () => {
	const single = { open: true, multiple: false, searching: false };
	assertDeepEqual(
		keyAction({ ...single, key: 'ArrowDown' }),
		{ type: 'move', key: 'ArrowDown' },
		'move'
	);
	assertDeepEqual(keyAction({ ...single, key: 'Enter' }), { type: 'commit' }, 'enter commits');
	assertDeepEqual(keyAction({ ...single, key: 'Escape' }), { type: 'close' }, 'escape');
	assertDeepEqual(keyAction({ ...single, key: 'Tab' }), { type: 'tab' }, 'tab');
	const multi = { ...single, multiple: true };
	assertDeepEqual(keyAction({ ...multi, key: ' ' }), { type: 'toggle' }, 'space toggles');
	assertDeepEqual(
		keyAction({ ...multi, key: 'Enter' }),
		{ type: 'toggle-close' },
		'enter toggles and closes'
	);
});
suite.check('keyAction in the search field leaves text keys to the input', () => {
	const search = { open: true, multiple: false, searching: true };
	assertEqual(keyAction({ ...search, key: 'a' }), null, 'letter');
	assertEqual(keyAction({ ...search, key: ' ' }), null, 'space');
	assertEqual(keyAction({ ...search, key: 'Home' }), null, 'caret keys');
	assertDeepEqual(
		keyAction({ ...search, key: 'ArrowDown' }),
		{ type: 'move', key: 'ArrowDown' },
		'arrows still move'
	);
	assertDeepEqual(keyAction({ ...search, key: 'Enter' }), { type: 'commit' }, 'enter');
});

await suite.checkAsync(
	'renders a combobox button with the label, value and no popup while closed',
	async () => {
		const html = await renderToHtml(Select, {
			value: 'done',
			options,
			onchange: () => {},
			label: 'Status'
		});
		assert(html.includes('role="combobox"') && html.includes('aria-haspopup="listbox"'), html);
		assert(html.includes('aria-expanded="false"'), 'collapsed');
		assert(html.includes('Status') && html.includes('Done'), 'label and selected label');
		assert(!html.includes('role="listbox"'), 'no listbox while closed');
	}
);
await suite.checkAsync(
	'shows the placeholder when nothing is selected, and marks required and invalid',
	async () => {
		const html = await renderToHtml(Select, {
			value: null,
			options,
			onchange: () => {},
			label: 'Status',
			placeholder: 'Choose a status',
			required: true,
			error: 'Choose a status.'
		});
		assert(html.includes('Choose a status'), 'placeholder');
		assert(
			html.includes('aria-required="true"') && html.includes('aria-invalid="true"'),
			'aria states'
		);
	}
);
await suite.checkAsync(
	'disabled shows its reason and read-only has no chevron button',
	async () => {
		const off = await renderToHtml(Select, {
			value: 'done',
			options,
			onchange: () => {},
			label: 'Status',
			disabled: true,
			disabledReason: 'Locked by the workflow'
		});
		assert(/<button[^>]*\sdisabled/.test(off) && off.includes('Locked by the workflow'), off);
		const ro = await renderToHtml(Select, {
			value: 'done',
			options,
			onchange: () => {},
			label: 'Status',
			readonly: true
		});
		assert(!ro.includes('role="combobox"') && ro.includes('Done'), ro);
	}
);

finish(suite.result());
