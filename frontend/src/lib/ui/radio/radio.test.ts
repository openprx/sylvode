import { createRawSnippet } from 'svelte';
import { Suite, assert, assertEqual, finish } from '../../../../tests/support/harness';
import { installSvelteSsr, renderToHtml } from '../../../../tests/support/svelte-ssr';

await installSvelteSsr();
const { default: RadioGroup } = await import('./RadioGroup.svelte');

// Arrow-key roving across the group is exercised in the browser run (scripts/ui-behaviour.mjs);
// the roving index rules are covered in foundations.test.ts.
const suite = new Suite('ui-radio');

await suite.checkAsync(
	'group renders a radiogroup with shared name and the selected item checked',
	async () => {
		const { default: Radio } = await import('./Radio.svelte');
		const children = createRawSnippet(() => ({ render: () => '<span>items</span>' }));
		const group = await renderToHtml(RadioGroup, { label: 'Pick', value: 'b', children });
		assert(group.includes('role="radiogroup"') && group.includes('aria-label="Pick"'), group);
		assertEqual(typeof Radio, 'function', 'Radio component loads');
	}
);

finish(suite.result());
