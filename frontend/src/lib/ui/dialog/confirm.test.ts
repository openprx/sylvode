import { readFileSync } from 'node:fs';
import { createRawSnippet } from 'svelte';
import {
	Suite,
	assert,
	assertDeepEqual,
	assertEqual,
	finish
} from '../../../../tests/support/harness';
import { installSvelteSsr, renderToHtml } from '../../../../tests/support/svelte-ssr';
import {
	CONFIRM_VERBS,
	ConfirmQueue,
	confirmLabelOf,
	type ConfirmOptions,
	type Translate
} from './confirm';

await installSvelteSsr();
const { default: Dialog } = await import('./Dialog.svelte');
const { default: DestructiveDialog } = await import('./DestructiveDialog.svelte');

const suite = new Suite('ui-confirm');

function messages(lang: 'en' | 'zh'): Record<string, unknown> {
	return JSON.parse(readFileSync(new URL(`./locale.${lang}.json`, import.meta.url), 'utf8'));
}
function translator(lang: 'en' | 'zh'): Translate {
	const tree = messages(lang);
	return (key, values = {}) => {
		let node: unknown = tree;
		for (const part of key.split('.')) node = (node as Record<string, unknown>)[part];
		if (typeof node !== 'string') throw new Error(`missing message ${key} in ${lang}`);
		return node.replace(/\{(\w+)\}/g, (_, name: string) => values[name] ?? '');
	};
}

function queue() {
	const warnings: string[] = [];
	let scope = '/a';
	const q = new ConfirmQueue(
		() => scope,
		(message) => warnings.push(message)
	);
	return { q, warnings, setScope: (value: string) => (scope = value) };
}
const opts = (title: string): ConfirmOptions => ({ title, confirmLabel: `Do ${title}` });

await suite.checkAsync('resolves true on confirm and false on decline', async () => {
	const { q } = queue();
	const host = q.attachHost();
	const yes = q.request(opts('a'));
	const first = q.current();
	assert(first !== null, 'request is current');
	q.settle(first, true);
	assertEqual(await yes, true, 'confirmed');
	const no = q.request(opts('b'));
	const second = q.current();
	assert(second !== null, 'second is current');
	q.settle(second, false);
	assertEqual(await no, false, 'declined');
	assertEqual(q.current(), null, 'queue empty');
	host.detach();
});

await suite.checkAsync(
	'two calls in a row are shown one after the other, in call order',
	async () => {
		const { q } = queue();
		q.attachHost();
		const order: string[] = [];
		const a = q.request(opts('a')).then((v) => order.push(`a:${v}`));
		const b = q.request(opts('b')).then((v) => order.push(`b:${v}`));
		assertEqual(q.pending(), 2, 'both queued');
		const head = q.current();
		assert(head?.options.title === 'a', 'a first');
		q.settle(head, true);
		assert(q.current()?.options.title === 'b', 'b second');
		const next = q.current();
		assert(next !== null, 'b current');
		q.settle(next, false);
		await Promise.all([a, b]);
		assertDeepEqual(order, ['a:true', 'b:false'], 'answers matched to calls');
	}
);

await suite.checkAsync('answering twice is ignored', async () => {
	const { q } = queue();
	q.attachHost();
	const p = q.request(opts('a'));
	const head = q.current();
	assert(head !== null, 'current');
	q.settle(head, false);
	q.settle(head, true);
	assertEqual(await p, false, 'first answer wins');
});

await suite.checkAsync('without a host it resolves false and warns once', async () => {
	const { q, warnings } = queue();
	assertEqual(await q.request(opts('a')), false, 'first');
	assertEqual(await q.request(opts('b')), false, 'second');
	assertEqual(warnings.length, 1, 'one warning');
	assertEqual(q.pending(), 0, 'nothing queued');
});

await suite.checkAsync('detaching the last host declines everything pending', async () => {
	const { q } = queue();
	const host = q.attachHost();
	const a = q.request(opts('a'));
	const b = q.request(opts('b'));
	host.detach();
	assertEqual(await a, false, 'current declined');
	assertEqual(await b, false, 'queued declined');
	assertEqual(q.current(), null, 'empty');
});

await suite.checkAsync(
	'only the first host is active; the second takes over when it leaves',
	async () => {
		const { q } = queue();
		const one = q.attachHost();
		const two = q.attachHost();
		assert(one.isActive() && !two.isActive(), 'first is active');
		const p = q.request(opts('a'));
		one.detach();
		assert(two.isActive(), 'second is active now');
		assert(q.current() !== null, 'request survives a host change');
		two.detach();
		assertEqual(await p, false, 'declined when the last host left');
	}
);

await suite.checkAsync('navigation declines requests of the old path only', async () => {
	const { q, setScope } = queue();
	q.attachHost();
	const old = q.request(opts('old'));
	setScope('/b');
	const fresh = q.request(opts('fresh'));
	q.cancelOutside('/b');
	assertEqual(await old, false, 'old path declined');
	assertEqual(q.current()?.options.title, 'fresh', 'new path request stays');
	q.cancelAll();
	assertEqual(await fresh, false, 'cancelAll declines');
});

await suite.checkAsync(
	'a request made from inside another dialog queues like any other',
	async () => {
		const { q } = queue();
		q.attachHost();
		const outer = q.request(opts('outer'));
		// The outer confirmation's handler asks again while it is still open.
		const inner = q.request(opts('inner'));
		const head = q.current();
		assert(head?.options.title === 'outer', 'outer shown first');
		q.settle(head, true);
		assertEqual(await outer, true, 'outer');
		const next = q.current();
		assert(next?.options.title === 'inner', 'inner follows');
		q.settle(next, true);
		assertEqual(await inner, true, 'inner');
	}
);

suite.check('confirm label names verb and object in English and Chinese', () => {
	const en = translator('en');
	const zh = translator('zh');
	assertEqual(
		confirmLabelOf({ title: 'q', verb: 'delete', object: 'webhook hooks.example' }, en),
		'Delete webhook hooks.example',
		'en delete'
	);
	assertEqual(
		confirmLabelOf({ title: 'q', verb: 'remove', object: 'member Ana' }, en),
		'Remove member Ana',
		'en remove'
	);
	assertEqual(
		confirmLabelOf({ title: 'q', verb: 'delete', object: '工作流“审批”' }, zh),
		'删除工作流“审批”',
		'zh delete puts the object after the verb with no space'
	);
	assertEqual(
		confirmLabelOf({ title: 'q', confirmLabel: 'Discard and sign out' }, en),
		'Discard and sign out',
		'caller label is used as given'
	);
	for (const verb of CONFIRM_VERBS) {
		for (const translate of [en, zh]) {
			const label = confirmLabelOf({ title: 'q', verb, object: 'X' }, translate);
			assert(label.includes('X') && label !== 'X', `verb ${verb} composes: ${label}`);
		}
	}
});

const body = createRawSnippet(() => ({ render: () => '<p>This cannot be undone.</p>' }));
const attr = (html: string, name: string) => new RegExp(`${name}="([^"]*)"`).exec(html)?.[1];

await suite.checkAsync(
	'DestructiveDialog renders an alertdialog with name and description',
	async () => {
		const html = await renderToHtml(DestructiveDialog, {
			open: true,
			title: 'Delete project Alpha?',
			confirmLabel: 'Delete project Alpha',
			onconfirm: () => {},
			onclose: () => {},
			children: body
		});
		assert(html.includes('role="alertdialog"'), 'alertdialog role');
		assert(!html.includes('role="dialog"'), 'no plain dialog role');
		const labelledby = attr(html, 'aria-labelledby');
		const describedby = attr(html, 'aria-describedby');
		assert(labelledby && describedby, 'name and description wired');
		assert(
			new RegExp(`<h2[^>]*id="${labelledby}"[^>]*>Delete project Alpha\\?</h2>`).test(html),
			`name is the title: ${html}`
		);
		assert(
			new RegExp(`id="${describedby}"[^>]*><p>This cannot be undone`).test(html),
			'description is the body'
		);
		assert(html.includes('data-autofocus'), 'Cancel carries the initial focus mark');
		assert(/Delete project Alpha(?!\?)/.test(html), 'confirm button text');
	}
);

await suite.checkAsync(
	'a plain Dialog still renders role="dialog" without a description',
	async () => {
		const html = await renderToHtml(Dialog, {
			open: true,
			title: 'Edit',
			onclose: () => {},
			children: body
		});
		assert(html.includes('role="dialog"'), 'dialog role');
		assert(!html.includes('alertdialog'), 'not an alertdialog');
		assertEqual(attr(html, 'aria-describedby'), undefined, 'no description');
		const named = await renderToHtml(Dialog, {
			open: true,
			title: 'Sure?',
			role: 'alertdialog',
			onclose: () => {},
			children: body
		});
		assert(named.includes('role="alertdialog"'), 'role prop is honoured');
	}
);

finish(suite.result());
