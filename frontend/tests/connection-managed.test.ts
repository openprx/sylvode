/**
 * Rows a project bot connection made: how the Bots and Webhooks answers are read, what a managed
 * row locks, which bots a webhook may name, the notice a managed row shows, and the shared
 * clipboard helper.
 *
 * Run standalone: `bun tests/connection-managed.test.ts`
 */
import { Suite, assert, assertEqual, finish } from './support/harness';
import { installSvelteSsr, renderToHtml } from './support/svelte-ssr';
import { copyText } from '../src/lib/clipboard';
import {
	MANAGED_REASON_KEY,
	executionHref,
	managedBotIds,
	managedLock,
	parseManagedBy,
	pickableBots
} from '../src/lib/project/connection/managed';

await installSvelteSsr();
const { default: ManagedNotice } =
	await import('../src/lib/project/connection/ManagedNotice.svelte');

const suite = new Suite('connection-managed');
const managed = {
	kind: 'project_connection',
	projects: [
		{ id: 'p1', name: 'Core' },
		{ id: 'p 2', name: 'Orders' }
	]
};

suite.check('managed_by is read at the boundary: only the documented shape counts', () => {
	assertEqual(parseManagedBy(managed)?.projects.length, 2, 'two projects');
	for (const bad of [
		null,
		undefined,
		'project_connection',
		[],
		{ kind: 'other', projects: [] },
		{ kind: 'project_connection' },
		{ kind: 'project_connection', projects: 'x' }
	])
		assertEqual(parseManagedBy(bad), null, JSON.stringify(bad));
	const partial = parseManagedBy({
		kind: 'project_connection',
		projects: [{ id: 'p1', name: 'Core' }, { id: 3, name: 'x' }, null]
	});
	assertEqual(partial?.projects.length, 1, 'malformed project entries are dropped');
});

suite.check('a managed row locks Edit, Delete, Rotate secret and Revoke with one reason', () => {
	const locked = managedLock(parseManagedBy(managed));
	assertEqual(locked.disabled, true, 'locked');
	assertEqual(locked.reasonKey, MANAGED_REASON_KEY, 'the reason');
	assertEqual(MANAGED_REASON_KEY, 'execution.connection.managed.reason', 'the key');
	const free = managedLock(parseManagedBy(null));
	assertEqual(free.disabled, false, 'an ordinary row is not locked');
	assertEqual(free.reasonKey, undefined, 'and has no reason');
});

suite.check('managed bots are absent from the webhook bot picker', () => {
	const bots = [
		{ id: 'b1', name: 'Managed', managed_by: managed },
		{ id: 'b2', name: 'Free', managed_by: null },
		{ id: 'b3', name: 'No field' },
		{ id: 'b4', name: 'Bogus', managed_by: { kind: 'x' } }
	];
	const ids = managedBotIds(bots);
	assertEqual([...ids].join(','), 'b1', 'only the connection-managed bot');
	const members = ['b1', 'b2', 'b3', 'b4'].map((user_id) => ({ user_id }));
	assertEqual(
		pickableBots(members, ids)
			.map((bot) => bot.user_id)
			.join(','),
		'b2,b3,b4',
		'the picker'
	);
});

suite.check('the link goes to the Execution section, ids encoded', () => {
	assertEqual(
		executionHref('w 1', 'p/2'),
		'/workspace/w%201/projects/p%2F2/settings/execution',
		'href'
	);
});

await suite.checkAsync('the notice names the chip and links each project', async () => {
	const parsed = parseManagedBy(managed);
	assert(parsed !== null, 'parsed');
	const html = await renderToHtml(ManagedNotice, { workspaceId: 'w1', managedBy: parsed });
	assert(html.includes('execution.connection.managed.chip'), 'the chip text');
	assert(html.includes('href="/workspace/w1/projects/p1/settings/execution"'), 'first link');
	assert(html.includes('href="/workspace/w1/projects/p%202/settings/execution"'), 'second link');
	assert(html.includes('Core') && html.includes('Orders'), 'project names');
});

await suite.checkAsync('the clipboard helper copies, falls back, and reports failure', async () => {
	const written: string[] = [];
	assertEqual(
		await copyText('abc', { clipboard: { writeText: async (text) => void written.push(text) } }),
		true,
		'clipboard path'
	);
	assertEqual(written.join('|'), 'abc', 'exactly the text, once');
	let legacyText = '';
	assertEqual(
		await copyText('def', {
			legacy: {
				copy: (text) => {
					legacyText = text;
					return true;
				}
			}
		}),
		true,
		'legacy path'
	);
	assertEqual(legacyText, 'def', 'legacy text');
	assertEqual(await copyText('x', { legacy: { copy: () => false } }), false, 'legacy refused');
	assertEqual(
		await copyText('x', {
			clipboard: {
				writeText: async () => {
					throw new Error('denied');
				}
			}
		}),
		false,
		'clipboard denied'
	);
	assertEqual(
		await copyText('x', {
			legacy: {
				copy: () => {
					throw new Error('boom');
				}
			}
		}),
		false,
		'legacy throws'
	);
	assertEqual(await copyText('x', {}), false, 'nothing available');
});

finish(suite.result());
