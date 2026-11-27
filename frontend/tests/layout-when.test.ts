/**
 * Layout display rules: `when`, placement `condition` and computed `{cel}` text in the
 * `layout_when` environment, and the pure parts of the "Create here" editor.
 *
 * Run standalone: `bun tests/layout-when.test.ts`
 */

import { Suite, assert, assertDeepEqual, assertEqual, finish } from './support/harness';
import { evaluateText, evaluateWhen } from '../src/lib/layout/when';
import { resolveText } from '../src/lib/layout/refs';
import {
	offeredScopes,
	parseSpec,
	placeable,
	refusalLines,
	starterSpec
} from '../src/lib/layout/editor/logic';

const suite = new Suite('layout-when');

const scope = {
	params: { record_id: 'r1' },
	data: { req: { values: { status: 'open', count: 3 } } },
	slot: { project_id: 'p1' },
	viewer: { id: 'u1', role: 'admin', locale: 'en', theme: 'light' }
};

suite.check('no expression shows', () => {
	assert(evaluateWhen(undefined, scope), 'undefined');
	assert(evaluateWhen(null, scope), 'null');
	assert(evaluateWhen('  ', scope), 'blank');
});

suite.check('a real expression shows and hides', () => {
	assert(evaluateWhen("data.req.values.status != 'closed'", scope), 'open is shown');
	assert(!evaluateWhen("data.req.values.status == 'closed'", scope), 'open is not closed');
	assert(evaluateWhen("viewer.role == 'admin' && data.req.values.count > 2", scope), 'and');
	assert(!evaluateWhen('false', scope), 'literal false hides');
});

suite.check('errors hide', () => {
	assert(!evaluateWhen('nope.x == 1', scope), 'unknown variable');
	assert(!evaluateWhen('1 +', scope), 'syntax');
	assert(!evaluateWhen('data.missing.values.status == 1', scope), 'missing key at runtime');
	assert(!evaluateWhen("'text'", scope), 'non-boolean result');
});

suite.check('computed text', () => {
	assertEqual(evaluateText("'Status: ' + data.req.values.status", scope), 'Status: open', 'string');
	assertEqual(evaluateText('data.req.values.count * 2', scope), '6', 'number');
	assertEqual(evaluateText('nope', scope), '', 'compile error is empty');
	assertEqual(evaluateText('data.x.y', scope), '', 'runtime error is empty');
	const full = { ...scope, actions: {} };
	assertEqual(resolveText({ cel: 'params.record_id' }, full, 'en'), 'r1', 'resolveText cel');
	assertEqual(resolveText({ en: 'Hi' }, full, 'en'), 'Hi', 'i18n still resolves');
});

suite.check('offered scopes follow slot, context and role', () => {
	const slot = { scope_kinds: ['workspace', 'project', 'form'] };
	const ctx = { project_id: 'p', form_id: 'f' };
	assertDeepEqual(
		offeredScopes(slot, ctx, 'w', 'member').map((s) => s.kind),
		['form', 'project'],
		'member'
	);
	assertDeepEqual(
		offeredScopes(slot, ctx, 'w', 'owner').map((s) => s.kind),
		['form', 'project', 'workspace'],
		'owner'
	);
	assertDeepEqual(
		offeredScopes(slot, { project_id: 'p' }, 'w', 'member').map((s) => s.kind),
		['project'],
		'no form'
	);
	assertDeepEqual(offeredScopes(slot, ctx, 'w', undefined), [], 'signed out');
	assertDeepEqual(
		offeredScopes({ scope_kinds: ['workspace'] }, ctx, 'w', 'member'),
		[],
		'workspace-only slot, member'
	);
});

suite.check('starter specs match slot roots', () => {
	const card = starterSpec({ allowed_roots: ['card', 'stat'] }, 'a') as { root: { type: string } };
	assertEqual(card.root.type, 'card', 'card');
	const bar = starterSpec({ allowed_roots: ['button', 'stack'] }, 'a') as {
		root: { type: string };
	};
	assertEqual(bar.root.type, 'stack', 'stack of buttons');
	assert(
		/^custom\.layout_[a-z0-9]+$/.test((card as unknown as { key: string }).key),
		'key grammar'
	);
});

suite.check('parse, refusal lines, placeable', () => {
	assert(parseSpec('{"a":1}').ok, 'object');
	assert(!parseSpec('[1]').ok, 'array');
	assert(!parseSpec('{').ok, 'syntax');
	assertDeepEqual(
		refusalLines({
			error_code: 'layout_invalid',
			details: { errors: [{ path: '/root/children/0/when', code: 'cel_syntax' }] }
		}),
		['/root/children/0/when  cel_syntax'],
		'node path and code'
	);
	assertDeepEqual(
		refusalLines({ error_code: 'slot_occupied', message: 'full' }),
		['slot_occupied  full'],
		'plain'
	);
	assertEqual(
		placeable([
			{ status: 'active', installation_id: null },
			{ status: 'archived', installation_id: null },
			{ status: 'active', installation_id: 'i' }
		]).length,
		1,
		'only active user layouts'
	);
});

finish(suite.result());
