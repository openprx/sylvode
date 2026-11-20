import { readFileSync } from 'node:fs';
import { Suite, assert, assertEqual, finish } from '../../../../tests/support/harness';
import { installSvelteSsr, renderToHtml } from '../../../../tests/support/svelte-ssr';
import { STATE_KINDS, capitalised, stateCopy } from './state-block-state';

await installSvelteSsr();
const { default: StateBlock } = await import('./StateBlock.svelte');

const suite = new Suite('ui-state-block');

function keysOf(file: string): Set<string> {
	const out = new Set<string>();
	const walk = (node: unknown, prefix: string) => {
		if (typeof node !== 'object' || node === null) return out.add(prefix);
		for (const [k, v] of Object.entries(node)) walk(v, prefix ? `${prefix}.${k}` : k);
	};
	walk(JSON.parse(readFileSync(new URL(file, import.meta.url), 'utf8')), '');
	return out;
}

suite.check(
	'every key a state uses exists in English and Chinese, with or without parameters',
	() => {
		const en = keysOf('locale.en.json');
		const zh = keysOf('locale.zh.json');
		const some = { thing: true, object: true, access: true, count: true, place: true };
		const none = {};
		for (const kind of STATE_KINDS) {
			for (const given of [some, none]) {
				const copy = stateCopy(kind, given);
				for (const key of [copy.titleKey, copy.bodyKey, copy.defaultActionKey]) {
					if (!key) continue;
					// A counted body is a plural group: English needs .other, Chinese only .other.
					const leaf = copy.bodyCounted && key === copy.bodyKey ? `${key}.other` : key;
					assert(en.has(leaf), `en has ${leaf}`);
					assert(zh.has(leaf), `zh has ${leaf}`);
				}
			}
		}
	}
);

suite.check('the catalogue has every kind of the design', () => {
	assertEqual(
		[...STATE_KINDS].sort().join(','),
		'empty,expired,forbidden,identityChanged,loadFailed,loading,noResults,noSearchResults,notFound,offline',
		'kinds'
	);
});

suite.check('the generic wording is used when the object parameters are missing', () => {
	assertEqual(stateCopy('empty', {}).titleKey, 'states.empty.titleGeneric', 'empty');
	assertEqual(stateCopy('loadFailed', {}).titleKey, 'states.loadFailed.titleGeneric', 'failed');
	assertEqual(stateCopy('notFound', {}).titleKey, 'states.notFound.titleGeneric', 'not found');
	assertEqual(stateCopy('forbidden', {}).bodyKey, 'states.forbidden.bodyGeneric', 'forbidden');
	assertEqual(stateCopy('noResults', {}).bodyKey, null, 'no count, no body');
	assertEqual(stateCopy('notFound', {}).defaultActionKey, null, 'no place, no action');
});

suite.check('the wording mapping: empty and forbidden are status, a failure is an alert', () => {
	assertEqual(stateCopy('empty').titleKey, 'states.empty.title', 'empty title');
	assertEqual(stateCopy('empty').bodyKey, 'states.empty.body', 'empty body');
	assertEqual(stateCopy('loadFailed').role, 'alert', 'failure role');
	assertEqual(stateCopy('forbidden').role, 'status', 'forbidden role');
	assertEqual(stateCopy('loading').bodyKey, null, 'loading has no body');
});

suite.check('the default action is Retry for a failure and none otherwise', () => {
	assertEqual(stateCopy('loadFailed').defaultActionKey, 'states.loadFailed.action', 'retry');
	assertEqual(stateCopy('empty').defaultActionKey, null, 'empty');
	assertEqual(stateCopy('forbidden').defaultActionKey, null, 'forbidden');
});

suite.check('the thing is capitalised for the sentence start', () => {
	assertEqual(capitalised('projects'), 'Projects', 'latin');
	assertEqual(capitalised(''), '', 'empty');
});

await suite.checkAsync(
	'an empty state renders heading, sentence and the caller action',
	async () => {
		const html = await renderToHtml(StateBlock, {
			kind: 'empty',
			thing: 'projects',
			action: { label: 'New project', run: () => {}, primary: true }
		});
		assert(html.includes('role="status"'), 'status role');
		assert(html.includes('states.empty.title'), 'heading key (no locale loaded in SSR)');
		assert(html.includes('New project'), 'action label');
		assert(html.includes('data-ui="button"') || html.includes('ui-button'), 'action is a button');
	}
);

await suite.checkAsync('a failed load is an alert with a Retry action by default', async () => {
	const html = await renderToHtml(StateBlock, { kind: 'loadFailed', action: { run: () => {} } });
	assert(html.includes('role="alert"'), 'alert role');
	assert(html.includes('states.loadFailed.action'), 'default Retry label key');
});

await suite.checkAsync('loading renders only a visually hidden status, no block', async () => {
	const html = await renderToHtml(StateBlock, { kind: 'loading', thing: 'projects' });
	assert(html.includes('role="status"'), 'status');
	assert(html.includes('ui-sr-only'), 'hidden');
	assert(!html.includes('class="title'), 'no visible heading');
});

const rendered = async (props: Record<string, unknown>) =>
	renderToHtml(StateBlock, { action: { run: () => {} }, ...props });

await suite.checkAsync(
	'every new kind renders a status block with its default action',
	async () => {
		const cases: [string, Record<string, unknown>, string][] = [
			['noResults', { count: 2 }, 'states.noResults.action'],
			['noSearchResults', { query: 'x' }, 'states.noSearchResults.action'],
			['offline', {}, 'states.offline.action'],
			['notFound', { object: 'project', place: 'Projects' }, 'states.notFound.action'],
			['expired', {}, 'states.expired.action'],
			['identityChanged', {}, 'states.identityChanged.action']
		];
		for (const [kind, props, action] of cases) {
			const html = await rendered({ kind, ...props });
			assert(html.includes(`data-kind="${kind}"`), `${kind} kind`);
			assert(html.includes('role="status"') && !html.includes('role="alert"'), `${kind} role`);
			assert(html.includes(action), `${kind} default action ${action}`);
		}
	}
);

await suite.checkAsync('the count selects the plural form of the filter sentence', async () => {
	const html = await rendered({ kind: 'noResults', count: 1 });
	// No locale is loaded in SSR, so the key shows: English one for 1 falls back to the key read.
	assert(html.includes('states.noResults.body'), html);
	assert(!(await rendered({ kind: 'noResults' })).includes('states.noResults.body'), 'no count');
});

await suite.checkAsync(
	'a message replaces the body and data attributes reach the block',
	async () => {
		const html = await rendered({ kind: 'offline', message: 'Custom sentence', 'data-x': 'y' });
		assert(html.includes('Custom sentence') && !html.includes('states.offline.body'), 'message');
		assert(html.includes('data-x="y"'), 'data attribute');
	}
);

finish(suite.result());
